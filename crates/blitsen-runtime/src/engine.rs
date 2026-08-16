//! Which JavaScript engine this runtime hosts, and the two things that differ.
//!
//! Everything else in this executable — and all of `blitsen-host` beneath it —
//! is generic over [`blitsen_js::JsEngine`]. Only three things are
//! engine-specific — loading the engine, pointing its module loader at the
//! registry, and creating a second engine for a worker thread — so only those
//! live here. Selecting the engine is therefore this file alone, which is the
//! claim `spikes/s8` set out to test and the reason the swap it recommended
//! touched nothing above this line.

use blitsen_js::JsError;

/// The engine this build hosts.
pub type Engine = blitsen_quickjs::QuickJs;

/// What `--engine-report` and the standalone check call this build.
pub const NAME: &str = "QuickJS-ng";
/// How the engine reaches this process, which decides whether anything has to
/// ship beside the executable.
pub const LINKAGE: &str = "static";

/// Loads the engine.
///
/// Nothing to find: the engine is inside the executable.
pub fn load() -> Result<Engine, String> {
    blitsen_quickjs::QuickJs::new().map_err(|error| error.to_string())
}

/// Points the engine's module loader at the resolver.
pub fn install_module_loader(engine: &mut Engine) -> Result<(), String> {
    engine.install_module_loader();
    Ok(())
}

/// How this runtime starts a web worker: a thread, and an engine of its own.
///
/// The third thing that is engine-specific, and it is here for the same reason
/// the other two are — `blitsen-host` cannot create an engine, only use one.
/// Everything after the engine exists is `blitsen_host::worker::run`, which
/// names no engine at all.
pub struct Workers;

impl blitsen_host::worker::WorkerLauncher for Workers {
    fn launch(&self, boot: blitsen_host::worker::WorkerBoot) -> Result<(), JsError> {
        let label = if boot.name.is_empty() {
            boot.entry.clone()
        } else {
            boot.name.clone()
        };
        std::thread::Builder::new()
            // Named for the thread lists a profiler and a debugger show, where
            // "worker" alone would be three threads with one name.
            .name(format!("blitsen-worker {label}"))
            .spawn(move || {
                let engine = load()
                    .and_then(|mut engine| install_module_loader(&mut engine).map(|()| engine));
                match engine {
                    Ok(engine) => blitsen_host::worker::run(engine, boot),
                    Err(error) => {
                        // Reported to the `Worker` object as an `error` event,
                        // because a thread that could not start an engine is
                        // exactly the failure its `onerror` is for.
                        blitsen_host::ports::registry().post(
                            boot.port,
                            blitsen_host::ports::Delivery::Error(format!(
                                "a worker could not start a JavaScript engine: {error}"
                            )),
                        );
                        blitsen_host::ports::registry().release(boot.context);
                    }
                }
            })
            .map(|_| ())
            .map_err(|error| JsError::new(format!("could not start a worker thread: {error}")))
    }
}

/// Whether this build can evaluate module scripts, for `--engine-report`.
///
/// Asked of the engine rather than hard-coded, because the report exists to be
/// checkable against what the compatibility profile claims.
pub fn supports_modules(engine: &Engine) -> bool {
    engine.supports_modules()
}

/// Which of `names` this engine does not define.
///
/// Asked of the engine rather than assumed, because the whole point of the
/// report is to be checkable against what the compatibility profile claims.
pub fn absent_globals(engine: &mut Engine, names: &[&str]) -> Vec<String> {
    use blitsen_js::JsEngine;
    names
        .iter()
        .filter(|name| {
            engine
                .evaluate_script(&format!("typeof {name}"), "blitsen:engine-globals")
                .ok()
                .and_then(|value| engine.to_string(&value).ok())
                .is_none_or(|kind| kind == "undefined")
        })
        .map(|name| (*name).to_owned())
        .collect()
}
