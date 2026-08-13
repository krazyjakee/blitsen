//! The worker pool the host-side network tasks run on.
//!
//! One pool for the process, shared by `fetch` and `WebSocket`: both are a
//! socket being waited on, and a second runtime would only add threads that
//! spend their lives parked.

use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use blitsen_js::JsError;
use tokio::runtime::Runtime;

/// Locks without propagating poisoning: a panicked worker must not disable
/// networking for the rest of the process.
pub(super) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Returns the process-wide worker pool, starting it on first use.
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
        .map_err(|error| JsError::new(format!("could not start the network worker pool: {error}")))
}
