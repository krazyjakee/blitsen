//! How the Phase 1 addon starts a web worker.
//!
//! Not over Node-API. A worker needs an engine *created*, on a thread of its
//! own, and a Node-API environment is handed to this addon rather than owned by
//! it — there is no `napi_env` to make a second of, and Bun's own thread pool is
//! not something an addon may put a JavaScript realm on.
//!
//! So a worker here runs the same engine the shipped runtime gives it:
//! QuickJS-ng, statically linked. That is not a compromise for the sake of the
//! test suite — it is the property issue #90 asks for. A worker's global scope
//! is the same source on both hosts, its messages take the same route through
//! `blitsen_host::ports`, and an application that works under the addon works
//! in the exported binary because the thing running its worker is the same
//! thing in both.

use blitsen_js::JsError;
use blitsen_quickjs::QuickJs;

/// The launcher registered when this addon creates its first engine.
pub struct Workers;

impl blitsen_host::worker::WorkerLauncher for Workers {
    fn launch(&self, boot: blitsen_host::worker::WorkerBoot) -> Result<(), JsError> {
        blitsen_host::worker::launch_on_thread(boot, || {
            let mut engine = QuickJs::new()?;
            engine.install_module_loader();
            Ok(engine)
        })
    }
}
