//! Web workers: a second JavaScript context, on a thread of its own.
//!
//! # What a worker is here
//!
//! A whole engine, not a second context inside the document's. The DOM is not
//! thread-safe and neither host's values are shareable across threads, so the
//! only shape that works is the one the web already specifies: separate heaps,
//! nothing shared, structured-clone message passing through
//! [`crate::ports`]. That constraint is not a limitation this runtime is
//! working around — it is what makes a worker safe to have at all.
//!
//! # Why the engine is not created here
//!
//! Everything in `blitsen-host` is generic over [`JsEngine`] and names no
//! engine, and a worker needs one *created*, which no trait method can do
//! generically — the host that installed the bridge may be a Node-API bridge
//! that does not own its runtime at all. So the crate that chose the engine
//! registers a [`WorkerLauncher`], and this module supplies everything that
//! happens once the engine exists. [`run`] is the worker's whole life, and it is
//! as engine-neutral as the rest of the crate.
//!
//! # The turn
//!
//! A worker's loop is the same shape as the document's frame loop with the
//! painting taken out: deliver what arrived, run the timers that are due, drain
//! microtasks, then sleep until there is a reason not to. It parks on a
//! [`Waker`] rather than polling, so an idle worker costs nothing, and whichever
//! thread queues a message wakes it.

use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use blitsen_js::{JsEngine, JsError};

use crate::messaging::{MessagingHost, WorkerFiles};
use crate::modules::ModuleRegistry;
use crate::ports::{ContextId, Delivery, PortId, Waker, registry};
use crate::runtime_services::RuntimeServices;

/// The worker global scope, spliced from the same fragments the document uses.
///
/// Events are this scope's own rather than the DOM bootstrap's: a worker has no
/// tree for an event to travel through, so the dispatcher is flat, and the
/// twelve hundred lines of node and element wrappers around the document's
/// `EventTarget` have nothing to do here. Everything around that is shared
/// source with the document — `members.js` above, because an interface member
/// has the same shape in either scope, and the clone codec and the messaging
/// classes below, because a message must mean the same thing at both ends.
const BOOTSTRAP: &str = concat!(
    "\n(() => {\n",
    include_str!("dom_bridge/bootstrap/members.js"),
    include_str!("worker/prelude.js"),
    include_str!("dom_bridge/bootstrap/fetch.js"),
    include_str!("dom_bridge/bootstrap/clone.js"),
    include_str!("dom_bridge/bootstrap/messaging.js"),
    include_str!("dom_bridge/bootstrap/intl.js"),
    include_str!("worker/scope.js"),
    "})();\n",
);

/// Everything a worker thread needs to become one.
pub struct WorkerBoot {
    /// `name` as the constructor was given it, reported by `self.name`.
    pub name: String,
    /// The application URL of the script to run.
    pub entry: String,
    /// Whether the script is a module. A classic worker is evaluated as a
    /// classic script, which is what `type: "classic"` means and why it cannot
    /// `import`.
    pub module: bool,
    /// The application the script is loaded out of.
    pub files: WorkerFiles,
    /// This worker's context, already known to the registry.
    pub context: ContextId,
    /// The worker's end of the pair its `Worker` object holds the other of.
    pub port: PortId,
    /// Raised by `terminate()`, and by the worker's own `close()`.
    pub stop: Arc<AtomicBool>,
    /// How the thread is woken when a message is queued for it.
    pub waker: Arc<Waker>,
}

/// Creates the engine a worker runs in, on a thread of its own.
///
/// Implemented by whichever crate selected the engine. The launch is expected to
/// return as soon as the thread is spawned; the worker's own loop runs for as
/// long as the worker lives.
pub trait WorkerLauncher: Send + Sync {
    /// Starts a worker, or reports why this build cannot.
    fn launch(&self, boot: WorkerBoot) -> Result<(), JsError>;
}

/// Starts one engine and worker loop on a named thread.
///
/// The crate that selected the engine supplies only its factory, including any
/// engine-specific module-loader installation. Thread naming and failures after
/// the spawn belong here because [`WorkerBoot`], the port registry and [`run`]
/// all do: keeping them together gives both hosts the same error delivery and
/// context-release order without making this engine-neutral crate depend on an
/// engine implementation.
pub fn launch_on_thread<E>(
    boot: WorkerBoot,
    engine_factory: impl FnOnce() -> Result<E, JsError> + Send + 'static,
) -> Result<(), JsError>
where
    E: JsEngine + 'static,
{
    let label = if boot.name.is_empty() {
        boot.entry.clone()
    } else {
        boot.name.clone()
    };
    std::thread::Builder::new()
        .name(format!("blitsen-worker {label}"))
        .spawn(move || match engine_factory() {
            Ok(engine) => run(engine, boot),
            Err(error) => {
                registry().post(
                    boot.port,
                    Delivery::Error(format!(
                        "a worker could not start a JavaScript engine: {error}"
                    )),
                );
                registry().release(boot.context);
            }
        })
        .map(|_| ())
        .map_err(|error| JsError::new(format!("could not start a worker thread: {error}")))
}

static LAUNCHER: OnceLock<Box<dyn WorkerLauncher>> = OnceLock::new();

/// Registers how this process starts a worker.
///
/// Process-wide rather than passed down through the bridge, because the engine
/// is a property of the executable and not of a document: the same answer is
/// correct for every context in the process, and threading it through six
/// signatures that do not otherwise care would only spread it out. The first
/// registration wins, and a second is ignored rather than refused — two entry
/// points into the same binary register the same launcher.
pub fn register_launcher(launcher: Box<dyn WorkerLauncher>) {
    let _ = LAUNCHER.set(launcher);
}

/// The registered launcher, or `None` in a build that never registered one.
pub fn launcher() -> Option<&'static dyn WorkerLauncher> {
    LAUNCHER.get().map(AsRef::as_ref)
}

/// Runs a worker to its end: boot, then turn until it is stopped.
///
/// Never returns while the worker is alive, so it is called on the worker's own
/// thread. An exception the script did not catch is reported to the port its
/// `Worker` object listens on rather than to this thread's standard error alone,
/// because the application that started the worker is the one that can act on
/// it.
pub fn run<E: JsEngine + 'static>(mut engine: E, boot: WorkerBoot) {
    let port = boot.port;
    match start(&mut engine, &boot) {
        Ok(services) => turn_until_stopped(&mut engine, &services, &boot),
        Err(error) => {
            registry().post(port, Delivery::Error(error.to_string()));
            eprintln!("Uncaught exception in worker {}: {error}", boot.entry);
        }
    }
    registry().release(boot.context);
}

/// Installs the worker's global scope and evaluates its script.
///
/// The services are handed back rather than dropped: the timer queue they own is
/// the one the bootstrap's `setTimeout` writes into, and the loop below is what
/// runs it. Installing a second set would give the loop an empty queue and leave
/// every timer the script armed unfired.
fn start<E: JsEngine + 'static>(
    engine: &mut E,
    boot: &WorkerBoot,
) -> Result<RuntimeServices<E>, JsError> {
    engine.set_interrupt_flag(Arc::clone(&boot.stop))?;
    let services = RuntimeServices::install(engine)?;
    let modules = Rc::new(ModuleRegistry::new(Arc::clone(&boot.files.source)));
    modules.install(engine)?;

    let host = Rc::new(MessagingHost::new(
        boot.context,
        // A worker may start a worker, for the same reason a document may: it
        // has the same application behind it and the same resolver over it.
        Some(boot.files.clone()),
    ));
    crate::messaging::install(engine, &host)?;
    crate::dom_bridge::install_worker_services(engine, boot.files.reader.clone())?;
    install_identity(engine, boot)?;

    engine.evaluate_script(BOOTSTRAP, "blitsen:worker-bootstrap")?;
    let source = modules.source(&boot.entry)?;
    if boot.module {
        // A module's evaluation is a promise on both hosts, so an exception at
        // its top level is a rejection rather than an `Err` here. Left alone it
        // would be an unhandled rejection nobody sees: the worker would sit
        // there having run none of its script, and the application that started
        // it would wait for a message that is never coming. So the result is
        // handed to `reportError`, which is where every other uncaught failure
        // on this thread goes.
        let module = engine.evaluate_module(&source, &boot.entry)?;
        engine.set_global("__blitsenWorkerModule", &module)?;
        engine.evaluate_script(
            "globalThis.__blitsenWorkerModule?.then?.(undefined, globalThis.reportError); \
             delete globalThis.__blitsenWorkerModule;",
            "blitsen:worker-module-result",
        )?;
    } else {
        engine.evaluate_script(&source, &boot.entry)?;
    }
    engine.drain_microtasks()?;
    Ok(services)
}

/// The facts a worker knows about itself, and the switch that ends it.
fn install_identity<E: JsEngine + 'static>(
    engine: &mut E,
    boot: &WorkerBoot,
) -> Result<(), JsError> {
    let port = boot.port;
    engine.define_global_function(
        "__blitsenWorkerSelfPort",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            Ok(engine.number(port.0 as f64))
        }),
    )?;
    let identity = serde_json::json!({ "name": boot.name, "url": boot.entry });
    let identity = crate::dom_bridge::json_value(engine, &identity)?;
    engine.set_global("__blitsenWorkerIdentity", &identity)?;

    let stop = Arc::clone(&boot.stop);
    let waker = Arc::clone(&boot.waker);
    engine.define_global_function(
        "__blitsenWorkerStop",
        Box::new(move |call| {
            stop.store(true, Ordering::Relaxed);
            waker.wake();
            Ok(call.this)
        }),
    )?;

    // Where an uncaught failure on this thread goes. The worker keeps running,
    // as a browser's does: one broken handler is not the end of the worker, and
    // the application decides what to do about it.
    engine.define_global_function(
        "__blitsenWorkerFailed",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let message = crate::dom_bridge::argument(&mut engine, &call, 0, "failure")?;
            registry().post(port, Delivery::Error(message));
            Ok(call.this)
        }),
    )
}

/// The worker's event loop.
fn turn_until_stopped<E: JsEngine + 'static>(
    engine: &mut E,
    services: &RuntimeServices<E>,
    boot: &WorkerBoot,
) {
    while !boot.stop.load(Ordering::Relaxed) {
        let pending = match turn(engine, services) {
            Ok(pending) => pending,
            Err(error) => {
                registry().post(boot.port, Delivery::Error(error.to_string()));
                eprintln!("Uncaught exception in worker {}: {error}", boot.entry);
                0
            }
        };
        if boot.stop.load(Ordering::Relaxed) {
            break;
        }
        // Work that lands off-thread — a `fetch` completion — has no waker of
        // its own, so a worker waiting on one takes short naps rather than
        // parking. With nothing outstanding it parks until the next timer, or
        // until a message wakes it.
        let timer = services.next_timer_delay();
        let wait = if pending > 0 {
            Some(timer.unwrap_or(POLL_INTERVAL).min(POLL_INTERVAL))
        } else {
            timer
        };
        if wait.is_some_and(|wait| wait.is_zero()) {
            continue;
        }
        boot.waker.wait(wait);
    }
}

/// How long a worker with off-thread work outstanding sleeps between checks.
const POLL_INTERVAL: Duration = Duration::from_millis(4);

/// One turn: deliver, run timers, drain microtasks.
fn turn<E: JsEngine + 'static>(
    engine: &mut E,
    services: &RuntimeServices<E>,
) -> Result<u32, JsError> {
    let pending =
        engine.evaluate_script("globalThis.__blitsenWorkerTurn()", "blitsen:worker-turn")?;
    let pending = engine.to_number(&pending)? as u32;
    services.run_expired_timers(engine)?;
    engine.drain_microtasks()?;
    Ok(pending)
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::modules::AppSource;

    struct EmptySource;

    impl AppSource for EmptySource {
        fn read(&self, _path: &str) -> Option<Vec<u8>> {
            None
        }
    }

    fn failed_launch(name: &str, entry: &str) -> (String, Vec<Delivery>) {
        let document = registry().new_context();
        let worker = registry().new_context();
        let (near, far) = registry().entangle(document, worker);
        registry().start(near);
        let waker = Arc::new(Waker::default());
        registry().attach_waker(document, Arc::clone(&waker));
        let boot = WorkerBoot {
            name: name.to_owned(),
            entry: entry.to_owned(),
            module: true,
            files: WorkerFiles {
                source: Arc::new(EmptySource),
                reader: None,
            },
            context: worker,
            port: far,
            stop: Arc::new(AtomicBool::new(false)),
            waker: Arc::new(Waker::default()),
        };
        let (thread_name, named) = mpsc::sync_channel(1);
        launch_on_thread::<blitsen_quickjs::QuickJs>(boot, move || {
            thread_name
                .send(std::thread::current().name().unwrap_or_default().to_owned())
                .expect("the test is still waiting for the worker thread");
            Err(JsError::new("factory refused"))
        })
        .expect("the worker thread can be spawned");

        let named = named
            .recv_timeout(Duration::from_secs(5))
            .expect("the engine factory ran on its worker thread");
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut deliveries = Vec::new();
        while !deliveries
            .iter()
            .any(|delivery| matches!(delivery, Delivery::Closed))
        {
            deliveries.extend(
                registry()
                    .drain(document)
                    .into_iter()
                    .map(|(_, delivery)| delivery),
            );
            if Instant::now() >= deadline {
                panic!("the failed worker did not release its context");
            }
            waker.wait(Some(Duration::from_millis(10)));
        }
        registry().release(document);
        (named, deliveries)
    }

    #[test]
    fn launcher_names_threads_and_reports_factory_failure_before_releasing() {
        for (name, entry, expected_thread) in [
            ("parser", "blitsen://app/work.js", "blitsen-worker parser"),
            (
                "",
                "blitsen://app/fallback.js",
                "blitsen-worker blitsen://app/fallback.js",
            ),
        ] {
            let (thread, deliveries) = failed_launch(name, entry);
            assert_eq!(thread, expected_thread);
            assert!(matches!(
                deliveries.as_slice(),
                [Delivery::Error(message), Delivery::Closed]
                    if message == "a worker could not start a JavaScript engine: factory refused"
            ));
        }
    }
}
