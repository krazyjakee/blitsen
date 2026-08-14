//! Bun-loadable Node-API addon: the Phase 1 JavaScript host.
//!
//! Two things live here and nowhere else — the [`JsEngine`](blitsen_js::JsEngine)
//! implementation over
//! Node-API, and the `#[napi]` surface Bun calls. Everything between the DOM and
//! the application is in `blitsen-host`, which names no engine at all.

mod engine;
mod exports;
mod workers;

use std::cell::RefCell;

use blitsen_host::app::AppFiles;
use blitsen_host::{OpenDirectoryOptions as HostOptions, WindowSession, native_window};
use blitsen_js::JsError;
use napi::{Env, Status};
use napi_derive::napi;

pub(crate) use engine::napi_error;
pub use engine::{NodeApiEngine, NodeClass, NodeWeakRef};
pub use exports::*;

/// JavaScript-facing engine owner loaded by `new Engine()`.
#[napi]
pub struct Engine {
    runtime: RefCell<NodeApiEngine>,
    session: RefCell<Option<WindowSession<NodeApiEngine>>>,
}

/// Options passed from directory-mode CLI to the native window.
#[napi(object)]
#[derive(Clone)]
pub struct OpenDirectoryOptions {
    /// Canonical application root.
    pub root: String,
    /// Canonical `index.html` path.
    pub entrypoint: String,
    /// Initial logical width.
    pub width: u32,
    /// Initial logical height.
    pub height: u32,
    /// Native title-bar text.
    pub title: String,
    /// Original directory argument, retained for diagnostics.
    pub directory: String,
}

impl From<OpenDirectoryOptions> for HostOptions {
    fn from(options: OpenDirectoryOptions) -> Self {
        Self {
            root: options.root,
            entrypoint: options.entrypoint,
            width: options.width,
            height: options.height,
            title: options.title,
            directory: options.directory,
        }
    }
}

#[napi]
impl Engine {
    /// Creates an engine in the current Bun/Node-API environment.
    #[napi(constructor)]
    pub fn new(env: Env) -> Self {
        Self {
            runtime: RefCell::new(NodeApiEngine::new(env)),
            session: RefCell::new(None),
        }
    }

    /// Parses `index.html` and initializes a native Blitz window session.
    #[napi(js_name = "openDirectory")]
    pub fn open_directory(&self, options: OpenDirectoryOptions) -> napi::Result<()> {
        // Rejected before any observable work: a second call must not build a
        // runtime, create an event loop, or run the document's scripts again.
        if self.session.borrow().is_some() {
            return Err(napi::Error::new(
                Status::GenericFailure,
                "a native window session is already open",
            ));
        }
        let options: HostOptions = options.into();
        // A URL is a dev server to read the application from rather than a
        // directory to read it from (#67). Both hosts take the same branch,
        // because both open the same session over the same `AppFiles`.
        let files = if options.entrypoint.starts_with("http://")
            || options.entrypoint.starts_with("https://")
        {
            AppFiles::server(&options.entrypoint).map_err(napi_error)?
        } else {
            AppFiles::directory(&options.entrypoint).map_err(napi_error)?
        };
        let mut engine = *self.runtime.borrow();
        let session = WindowSession::open(&mut engine, files, options).map_err(napi_error)?;
        *self.session.borrow_mut() = Some(session);
        Ok(())
    }

    /// Re-parses the directory entrypoint and replaces the document in the existing window.
    #[napi(js_name = "reloadDirectory")]
    pub fn reload_directory(&self) -> napi::Result<()> {
        self.with_session(|session, engine| session.reload(engine))
    }

    /// Reloads a linked CSS file into the current document without rerunning JavaScript.
    #[napi(js_name = "reloadCSS")]
    pub fn reload_css(&self, file: String) -> napi::Result<bool> {
        self.with_session(|session, _| session.reload_css(&file))
    }

    /// Advances winit once without blocking Bun's outer event loop.
    #[napi(js_name = "pumpWindow")]
    pub fn pump_window(&self) -> napi::Result<bool> {
        let alive = self.with_session(|session, _| session.pump())?;
        if !alive {
            native_window::release_window();
            self.session.borrow_mut().take();
        }
        Ok(alive)
    }

    fn with_session<T>(
        &self,
        action: impl FnOnce(&mut WindowSession<NodeApiEngine>, &mut NodeApiEngine) -> Result<T, JsError>,
    ) -> napi::Result<T> {
        let mut session = self.session.borrow_mut();
        let session = session.as_mut().ok_or_else(|| {
            napi::Error::new(Status::GenericFailure, "no native window session is open")
        })?;
        let mut engine = *self.runtime.borrow();
        action(session, &mut engine).map_err(napi_error)
    }
}
