//! The host half of `postMessage`, for whichever context is asking.
//!
//! Installed into the document's context and into every worker's, because both
//! own ports and both send the same messages through them. Nothing here knows
//! which it is in: a context is an identifier in [`crate::ports`], and the only
//! asymmetry is that a context with no application files behind it cannot start
//! a worker, because there would be no script to run.
//!
//! Binary payloads are staged rather than serialized. The bootstrap hands whole
//! buffers over as typed arrays and refers to them by position, so a megabyte of
//! `ArrayBuffer` crosses the engine boundary once instead of becoming a JSON
//! array of a million numbers.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use blitsen_js::{JsEngine, JsError, TypedArray, TypedArrayKind};
use serde_json::{Value, json};

use crate::app::AppReader;
use crate::dom_bridge::{argument, json_value};
use crate::modules::AppSource;
use crate::ports::{ContextId, Delivery, Envelope, PortId, Waker, registry};

/// The application a context can start a worker out of.
///
/// A worker loads its script through the same resolver the document's modules
/// go through, so `new Worker(new URL("./work.js", import.meta.url))` names the
/// same file whether the application is a directory being run or a section
/// inside the executable.
#[derive(Clone)]
pub struct WorkerFiles {
    /// The application's own files, as the module resolver sees them.
    pub source: Arc<dyn AppSource>,
    /// How a URL naming a shipped file is read, for a worker's `fetch`.
    pub reader: Option<AppReader>,
}

/// A running worker, from the side that started it.
struct WorkerHandle {
    context: ContextId,
    stop: Arc<AtomicBool>,
    waker: Arc<Waker>,
}

/// Everything one JavaScript context needs to send and receive messages.
pub struct MessagingHost {
    context: ContextId,
    /// Buffers staged by the bootstrap for the next message it posts.
    outbound: RefCell<Vec<Vec<u8>>>,
    /// Buffers delivered and not yet read back, keyed by the token the record
    /// referring to them carries.
    inbound: RefCell<HashMap<u64, Vec<u8>>>,
    next_token: Cell<u64>,
    workers: RefCell<HashMap<u64, WorkerHandle>>,
    next_worker: Cell<u64>,
    files: Option<WorkerFiles>,
}

impl MessagingHost {
    /// Creates the messaging state for a context of its own.
    pub fn new(context: ContextId, files: Option<WorkerFiles>) -> Self {
        Self {
            context,
            outbound: RefCell::default(),
            inbound: RefCell::default(),
            next_token: Cell::new(1),
            workers: RefCell::default(),
            next_worker: Cell::new(1),
            files,
        }
    }

    /// The context these ports belong to.
    pub fn context(&self) -> ContextId {
        self.context
    }

    fn stage(&self, bytes: Vec<u8>) {
        self.outbound.borrow_mut().push(bytes);
    }

    /// Takes what was staged, leaving the area empty for the next message.
    fn take_staged(&self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.outbound.borrow_mut())
    }

    fn hold(&self, buffers: Vec<Vec<u8>>) -> Vec<u64> {
        let mut inbound = self.inbound.borrow_mut();
        buffers
            .into_iter()
            .map(|bytes| {
                let token = self.next_token.get();
                self.next_token.set(token + 1);
                inbound.insert(token, bytes);
                token
            })
            .collect()
    }

    /// Hands a delivered payload over, which happens exactly once.
    fn take_held(&self, token: u64) -> Result<Vec<u8>, JsError> {
        self.inbound
            .borrow_mut()
            .remove(&token)
            .ok_or_else(|| JsError::new("this message payload has already been read"))
    }

    /// Moves staged buffers straight into the delivered set, for a clone that
    /// never leaves this context.
    fn adopt_staged(&self) -> Vec<u64> {
        let staged = self.take_staged();
        self.hold(staged)
    }

    fn post(&self, port: PortId, graph: String, ports: Vec<PortId>) {
        registry().post(
            port,
            Delivery::Message(Envelope {
                data: graph,
                buffers: self.take_staged(),
                ports,
            }),
        );
    }

    /// Everything queued for this context's started ports, as the bootstrap
    /// reads it.
    fn poll(&self) -> Value {
        let drained = registry().drain(self.context);
        let mut records = Vec::with_capacity(drained.len());
        for (port, delivery) in drained {
            let record = match delivery {
                Delivery::Message(envelope) => json!({
                    "port": port.0,
                    "type": "message",
                    "data": envelope.data,
                    "ports": envelope.ports.iter().map(|port| port.0).collect::<Vec<_>>(),
                    "buffers": self.hold(envelope.buffers),
                }),
                Delivery::Error(message) => json!({
                    "port": port.0, "type": "error", "message": message,
                }),
                Delivery::Closed => json!({ "port": port.0, "type": "close" }),
            };
            records.push(record);
        }
        Value::Array(records)
    }

    /// Starts a worker, returning its identifier and this side's port.
    fn start_worker(&self, url: &str, module: bool, name: &str) -> Result<Value, JsError> {
        let files = self.files.as_ref().ok_or_else(|| {
            JsError::new("there is no application behind this context to load a worker script from")
        })?;
        let launcher = crate::worker::launcher().ok_or_else(|| {
            JsError::new("this build of Blitsen has no JavaScript engine registered for workers")
        })?;
        // Refused here rather than on the worker's thread: a script the
        // application does not ship is a mistake the constructor should report,
        // not an `error` event a turn later.
        let entry = crate::modules::ModuleRegistry::new(Arc::clone(&files.source))
            .resolve(crate::modules::APP_ORIGIN, url)?;

        let context = registry().new_context();
        let (near, far) = registry().entangle(self.context, context);
        let stop = Arc::new(AtomicBool::new(false));
        let waker = Arc::new(Waker::default());
        registry().attach_waker(context, Arc::clone(&waker));

        let boot = crate::worker::WorkerBoot {
            name: name.to_owned(),
            entry,
            module,
            files: files.clone(),
            context,
            port: far,
            stop: Arc::clone(&stop),
            waker: Arc::clone(&waker),
        };
        if let Err(error) = launcher.launch(boot) {
            registry().release(context);
            registry().close(near);
            return Err(error);
        }
        let id = self.next_worker.get();
        self.next_worker.set(id + 1);
        self.workers.borrow_mut().insert(
            id,
            WorkerHandle {
                context,
                stop,
                waker,
            },
        );
        Ok(json!({ "worker": id, "port": near.0 }))
    }

    /// Stops a worker and drops everything it had queued.
    ///
    /// The flag is what its own loop checks between turns, and the engine's
    /// interrupt handler checks inside one — so a worker in a loop that never
    /// yields still stops, on the hosts whose engine can be interrupted.
    fn terminate_worker(&self, id: u64) {
        let Some(handle) = self.workers.borrow_mut().remove(&id) else {
            return;
        };
        handle.stop.store(true, Ordering::Relaxed);
        handle.waker.wake();
        registry().release(handle.context);
    }

    /// Ends every worker this context started and closes every port it owns.
    pub fn dispose(&self) {
        let ids = self.workers.borrow().keys().copied().collect::<Vec<_>>();
        for id in ids {
            self.terminate_worker(id);
        }
        registry().release(self.context);
        self.inbound.borrow_mut().clear();
        self.outbound.borrow_mut().clear();
    }
}

fn port_argument<E: JsEngine>(
    engine: &mut E,
    call: &blitsen_js::NativeCall<E::Value>,
    index: usize,
) -> Result<PortId, JsError> {
    argument(engine, call, index, "port id")?
        .parse::<u64>()
        .map(PortId)
        .map_err(|_| JsError::new("invalid port id"))
}

/// Installs the messaging entry points the bootstrap calls through.
pub fn install<E: JsEngine + 'static>(
    engine: &mut E,
    host: &Rc<MessagingHost>,
) -> Result<(), JsError> {
    let channel_host = Rc::clone(host);
    engine.define_global_function(
        "__blitsenPortChannel",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let context = channel_host.context;
            let (first, second) = registry().entangle(context, context);
            json_value(&mut engine, &json!([first.0, second.0]))
        }),
    )?;

    let post_host = Rc::clone(host);
    engine.define_global_function(
        "__blitsenPortPost",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let port = port_argument(&mut engine, &call, 0)?;
            let graph = argument(&mut engine, &call, 1, "message graph")?;
            let ports: Vec<u64> =
                serde_json::from_str(&argument(&mut engine, &call, 2, "transferred ports")?)
                    .map_err(|error| JsError::new(format!("invalid transfer list: {error}")))?;
            post_host.post(port, graph, ports.into_iter().map(PortId).collect());
            Ok(call.this)
        }),
    )?;

    let start_host = Rc::clone(host);
    engine.define_global_function(
        "__blitsenPortStart",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let port = port_argument(&mut engine, &call, 0)?;
            let _ = &start_host;
            registry().start(port);
            Ok(call.this)
        }),
    )?;

    let close_host = Rc::clone(host);
    engine.define_global_function(
        "__blitsenPortClose",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let port = port_argument(&mut engine, &call, 0)?;
            let _ = &close_host;
            registry().close(port);
            Ok(call.this)
        }),
    )?;

    let poll_host = Rc::clone(host);
    engine.define_global_function(
        "__blitsenPortPoll",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            json_value(&mut engine, &poll_host.poll())
        }),
    )?;

    let pending_host = Rc::clone(host);
    engine.define_global_function(
        "__blitsenPortPending",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let pending = registry().pending(pending_host.context);
            Ok(engine.boolean(pending))
        }),
    )?;

    let stage_host = Rc::clone(host);
    engine.define_global_function(
        "__blitsenCloneStage",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let bytes = call
                .arguments
                .first()
                .ok_or_else(|| JsError::new("missing message payload"))
                .and_then(|value| engine.to_typed_array(value))?;
            stage_host.stage(bytes.bytes);
            Ok(call.this)
        }),
    )?;

    let take_host = Rc::clone(host);
    engine.define_global_function(
        "__blitsenCloneTake",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let token = argument(&mut engine, &call, 0, "payload token")?
                .parse::<u64>()
                .map_err(|_| JsError::new("invalid payload token"))?;
            let bytes = take_host.take_held(token)?;
            engine.typed_array(&TypedArray::new(TypedArrayKind::Uint8, bytes)?)
        }),
    )?;

    let adopt_host = Rc::clone(host);
    engine.define_global_function(
        "__blitsenCloneAdopt",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let tokens = adopt_host.adopt_staged();
            json_value(&mut engine, &json!(tokens))
        }),
    )?;

    engine.define_global_function(
        "__blitsenDetachBuffer",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let buffer = call
                .arguments
                .first()
                .ok_or_else(|| JsError::new("missing transferred buffer"))?
                .clone();
            engine.detach_array_buffer(&buffer)?;
            Ok(call.this)
        }),
    )?;

    let worker_host = Rc::clone(host);
    engine.define_global_function(
        "__blitsenWorkerStart",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let url = argument(&mut engine, &call, 0, "worker script")?;
            let kind = argument(&mut engine, &call, 1, "worker type")?;
            let name = argument(&mut engine, &call, 2, "worker name")?;
            let started = worker_host.start_worker(&url, kind == "module", &name)?;
            json_value(&mut engine, &started)
        }),
    )?;

    let terminate_host = Rc::clone(host);
    engine.define_global_function(
        "__blitsenWorkerTerminate",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let id = argument(&mut engine, &call, 0, "worker id")?
                .parse::<u64>()
                .map_err(|_| JsError::new("invalid worker id"))?;
            terminate_host.terminate_worker(id);
            Ok(call.this)
        }),
    )?;

    let dispose_host = Rc::clone(host);
    engine.define_global_function(
        "__blitsenMessagingDispose",
        Box::new(move |call| {
            dispose_host.dispose();
            Ok(call.this)
        }),
    )
}
