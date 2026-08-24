//! The thread pool the host-side network tasks run on.
//!
//! One pool for the process, shared by `fetch` and `WebSocket`: both are a
//! socket being waited on, and a second runtime would only add threads that
//! spend their lives parked.
//!
//! Nothing to do with a web worker, which is a JavaScript context of its own and
//! lives in [`crate::worker`]. This file was called `worker.rs` until there were
//! both of them.
//!
//! The fetch, socket, event-source and audio queues around this runtime use
//! non-poisoning locks: a panicking task is isolated to its own operation and
//! must not disable networking for every other context in the process.

use std::sync::OnceLock;
use std::time::Duration;

use blitsen_js::JsError;
use reqwest::Client;
use tokio::runtime::Runtime;

/// Returns the process-wide network pool, starting it on first use.
pub(super) fn runtime() -> Result<&'static Runtime, JsError> {
    static RUNTIME: OnceLock<Result<Runtime, String>> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("blitsen-net")
                .enable_all()
                .build()
                .map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|error| JsError::new(format!("could not start the network pool: {error}")))
}

/// Builds an HTTP client inside the runtime that owns its connection pool.
pub(super) fn client(runtime: &Runtime) -> Result<Client, JsError> {
    let guard = runtime.enter();
    // A connect timeout and deliberately no whole-request `.timeout()`: this
    // client also carries `EventSource` streams, which are open for as long as
    // the application listens, and a total-request deadline would cut every one
    // of them off. An address that swallows the connection attempt, though,
    // would otherwise hold a task forever on a two-thread pool — thirty
    // seconds is the figure the dev-server client already settled on.
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| JsError::new(format!("could not start the HTTP client: {error}")))?;
    drop(guard);
    Ok(client)
}
