//! Bun-loadable Node-API addon and JavaScript-engine implementation.

mod alloc;
mod dom_bridge;
mod frame_loop;
mod replay;

mod assets;
mod engine;
mod harness;
mod native_window;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use blitsen_blitz::BlitzDom;
use blitsen_core::ScriptDocument;
use blitsen_dom::{DomBackend, DomName};
use blitsen_js::{JsEngine, JsError};
use blitz::dom::{DocumentConfig, NodeId};
use blitz::shell::{BlitzApplication, BlitzShellProxy, WindowConfig, create_default_event_loop};
use blitz::traits::net::NetProvider;
use blitz::traits::shell::{ColorScheme, Viewport};
use napi::{Env, Status};
use napi_derive::napi;
use winit::dpi::LogicalSize;
use winit::event_loop::pump_events::EventLoopExtPumpEvents;
use winit::keyboard::ModifiersState;
use winit::window::WindowAttributes;

#[cfg(target_os = "macos")]
use winit::application::macos::ApplicationHandlerExtMacOS;

// The addon is split by concern; these keep the crate-internal paths the
// submodules already use, and the public surface unchanged.
pub(crate) use assets::validate_local_assets;
pub use engine::{NodeApiEngine, NodeClass, NodeWeakRef};
pub(crate) use engine::{callback_string, check, dom_error, napi_error, raw, unknown};
pub use harness::*;
pub(crate) use harness::{
    encode_png, execute_window_scripts, load_document_harness, render_document,
};
pub(crate) use native_window::{SharedBlitzDocument, WindowApplication, WindowSession};

/// Stable addon name used by packaging and smoke tests.
pub const ADDON_NAME: &str = "blitsen-node";

/// JavaScript-facing engine owner loaded by `new Engine()`.
#[napi]
pub struct Engine {
    runtime: RefCell<NodeApiEngine>,
    session: RefCell<Option<WindowSession>>,
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

/// Shared native DOM state addressed by serialized generational Blitz handles.
///
/// Every native bridge entry point parses the opaque handle and resolves it in
/// the authoritative document before performing work.
#[derive(Clone)]
pub struct DomRuntime {
    document: Rc<RefCell<BlitzDom>>,
}

impl DomRuntime {
    /// Owns a concrete Blitz backend behind single-threaded shared state.
    pub fn new(document: BlitzDom) -> Self {
        Self {
            document: Rc::new(RefCell::new(document)),
        }
    }

    /// Returns the shared backend for a synchronous bridge operation.
    pub fn document(&self) -> Rc<RefCell<BlitzDom>> {
        Rc::clone(&self.document)
    }

    /// Serializes a versioned Blitz handle without losing integer precision in JavaScript.
    pub fn serialize_handle(node: NodeId) -> String {
        node.as_u64().to_string()
    }

    /// Parses an opaque handle and rejects stale or fabricated generations.
    pub fn resolve_handle(&self, handle: &str) -> Result<NodeId, JsError> {
        let raw = handle
            .parse::<u64>()
            .map_err(|_| JsError::new("invalid DOM node handle"))?;
        let node = NodeId::from_u64(raw);
        self.document
            .borrow()
            .node_kind(node)
            .map_err(|error| JsError::new(error.to_string()))?;
        Ok(node)
    }

    /// Retains a detached node for one live JavaScript wrapper.
    pub fn retain_handle(&self, handle: &str) -> Result<(), JsError> {
        let node = self.resolve_handle(handle)?;
        self.document
            .borrow_mut()
            .retain_for_js(node)
            .map_err(|error| JsError::new(error.to_string()))
    }

    /// Releases one wrapper and collects an otherwise-unowned detached subtree.
    pub fn release_handle(&self, handle: &str) -> Result<bool, JsError> {
        let node = self.resolve_handle(handle)?;
        self.document
            .borrow_mut()
            .release_from_js(node)
            .map_err(|error| JsError::new(error.to_string()))
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

    /// Loads an HTML file from disk and returns its source for the document
    /// loader added by the following milestone issues.
    #[napi(js_name = "loadHTML")]
    pub fn load_html(&self, path: String) -> napi::Result<String> {
        let path = Path::new(&path);
        let source = std::fs::read_to_string(path).map_err(|error| {
            napi::Error::new(
                Status::GenericFailure,
                format!("could not read {}: {error}", path.display()),
            )
        })?;
        // Exercise the owned runtime here so constructor state is not merely
        // decorative; document installation follows in issues #23 and #24.
        let _ = self.runtime.borrow_mut().undefined();
        Ok(source)
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
        let session_options = options.clone();
        let started_at = Instant::now();
        let source = std::fs::read_to_string(&options.entrypoint).map_err(|error| {
            napi::Error::new(
                Status::GenericFailure,
                format!("could not read {}: {error}", options.entrypoint),
            )
        })?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|error| napi::Error::new(Status::GenericFailure, error.to_string()))?;
        let guard = runtime.enter();
        let event_loop = create_default_event_loop();
        let (proxy, receiver) = BlitzShellProxy::new(event_loop.create_proxy());
        let net_provider = Arc::new(blitz::net::Provider::new(Some(Arc::new(proxy.clone()))));
        let dom_runtime = DomRuntime::new(BlitzDom::from_html(
            &source,
            DocumentConfig {
                base_url: Some(format!("file://{}/", options.root.replace(' ', "%20"))),
                net_provider: Some(net_provider as Arc<dyn NetProvider>),
                viewport: Some(Viewport::new(
                    options.width,
                    options.height,
                    1.0,
                    ColorScheme::Light,
                )),
                ..Default::default()
            },
        ));
        let document = dom_runtime.document();
        validate_local_assets(
            &document.borrow(),
            Path::new(&options.root),
            Path::new(&options.entrypoint),
        )
        .map_err(napi_error)?;
        let scripts = {
            let document = document.borrow();
            document.document_scripts().map_err(dom_error)?
        };
        let mut engine = self.runtime.borrow_mut();
        let raw_env = engine.raw_env();
        let window_state = execute_window_scripts(
            &mut engine,
            dom_runtime,
            scripts,
            &options.entrypoint,
            options.width,
            options.height,
            false,
        )?;
        drop(engine);
        document.borrow_mut().flush_layout().map_err(dom_error)?;
        let renderer = anyrender_vello::VelloWindowRenderer::new();
        let attributes = WindowAttributes::default()
            .with_title(options.title)
            .with_surface_size(LogicalSize::new(options.width, options.height));
        let window = WindowConfig::with_attributes(
            Box::new(SharedBlitzDocument(Rc::clone(&document))),
            renderer,
            attributes,
        );
        let mut application = BlitzApplication::new(proxy, receiver);
        application.add_window(window);
        let window_error = Rc::new(RefCell::new(None));
        let application = WindowApplication {
            inner: application,
            env: raw_env,
            state: window_state,
            error: Rc::clone(&window_error),
            started_at,
            document,
            pending_mouse_input: Vec::new(),
            pending_keyboard_input: Vec::new(),
            pointer_positions: HashMap::new(),
            mouse_down_targets: HashMap::new(),
            mouse_buttons: 0,
            modifiers: ModifiersState::empty(),
            load_dispatched: false,
        };
        drop(guard);
        *self.session.borrow_mut() = Some(WindowSession {
            runtime,
            event_loop,
            application,
            error: window_error,
            options: session_options,
        });
        Ok(())
    }

    /// Re-parses the directory entrypoint and replaces the document in the existing window.
    #[napi(js_name = "reloadDirectory")]
    pub fn reload_directory(&self) -> napi::Result<()> {
        let mut session_ref = self.session.borrow_mut();
        let session = session_ref.as_mut().ok_or_else(|| {
            napi::Error::new(Status::GenericFailure, "no native window session is open")
        })?;
        let _guard = session.runtime.enter();
        let options = session.options.clone();
        let source = std::fs::read_to_string(&options.entrypoint).map_err(|error| {
            napi::Error::new(
                Status::GenericFailure,
                format!("could not read {}: {error}", options.entrypoint),
            )
        })?;
        let window_id = session
            .application
            .inner
            .windows
            .keys()
            .copied()
            .next()
            .ok_or_else(|| {
                napi::Error::new(Status::GenericFailure, "native window is not ready")
            })?;
        let viewport = session.application.inner.windows[&window_id]
            .doc
            .inner()
            .viewport()
            .clone();
        let scale = f64::from(viewport.hidpi_scale);
        let logical = winit::dpi::PhysicalSize::new(viewport.window_size.0, viewport.window_size.1)
            .to_logical::<u32>(scale);
        let proxy = session.application.inner.proxy.clone();
        let net_provider = Arc::new(blitz::net::Provider::new(Some(Arc::new(proxy))));
        let dom_runtime = DomRuntime::new(BlitzDom::from_html(
            &source,
            DocumentConfig {
                base_url: Some(format!("file://{}/", options.root.replace(' ', "%20"))),
                net_provider: Some(net_provider as Arc<dyn NetProvider>),
                viewport: Some(viewport),
                ..Default::default()
            },
        ));
        let document = dom_runtime.document();
        validate_local_assets(
            &document.borrow(),
            Path::new(&options.root),
            Path::new(&options.entrypoint),
        )
        .map_err(napi_error)?;
        let scripts = document.borrow().document_scripts().map_err(dom_error)?;
        let window_state = execute_window_scripts(
            &mut self.runtime.borrow_mut(),
            dom_runtime,
            scripts,
            &options.entrypoint,
            logical.width,
            logical.height,
            false,
        )?;
        document.borrow_mut().flush_layout().map_err(dom_error)?;

        let view = session
            .application
            .inner
            .windows
            .get_mut(&window_id)
            .expect("window id was read from this map");
        view.replace_document(Box::new(SharedBlitzDocument(Rc::clone(&document))), false);
        let application = &mut session.application;
        application.state = window_state;
        application.document = document;
        application.started_at = Instant::now();
        application.pending_mouse_input.clear();
        application.pending_keyboard_input.clear();
        application.pointer_positions.clear();
        application.mouse_down_targets.clear();
        application.mouse_buttons = 0;
        application.load_dispatched = false;
        Ok(())
    }

    /// Reloads a linked CSS file into the current document without rerunning JavaScript.
    #[napi(js_name = "reloadCSS")]
    pub fn reload_css(&self, file: String) -> napi::Result<bool> {
        let mut session_ref = self.session.borrow_mut();
        let session = session_ref.as_mut().ok_or_else(|| {
            napi::Error::new(Status::GenericFailure, "no native window session is open")
        })?;
        let _guard = session.runtime.enter();
        let root = Path::new(&session.options.root);
        let changed = root.join(&file).canonicalize().map_err(|error| {
            napi::Error::new(
                Status::GenericFailure,
                format!("could not reload CSS file {file}: {error}"),
            )
        })?;
        if !changed.starts_with(root) {
            return Err(napi::Error::new(
                Status::GenericFailure,
                format!("CSS reload escaped application directory: {file}"),
            ));
        }
        let href_name = DomName::attribute("href");
        let rel_name = DomName::attribute("rel");
        let hrefs = {
            let document = session.application.document.borrow();
            document
                .query_selector_all(document.document(), "link[href]")
                .map_err(dom_error)?
                .into_iter()
                .filter_map(|node| {
                    let rel = document.attribute(node, &rel_name).ok().flatten()?;
                    if !rel
                        .split_ascii_whitespace()
                        .any(|value| value.eq_ignore_ascii_case("stylesheet"))
                    {
                        return None;
                    }
                    let href = document.attribute(node, &href_name).ok().flatten()?;
                    let local = href.split(['?', '#']).next().unwrap_or_default();
                    root.join(local)
                        .canonicalize()
                        .ok()
                        .filter(|candidate| *candidate == changed)
                        .map(|_| href)
                })
                .collect::<Vec<_>>()
        };
        for href in &hrefs {
            session
                .application
                .document
                .borrow_mut()
                .document_mut()
                .reload_resource_by_href(href);
        }
        Ok(!hrefs.is_empty())
    }

    /// Advances winit once without blocking Bun's outer event loop.
    #[napi(js_name = "pumpWindow")]
    pub fn pump_window(&self) -> napi::Result<bool> {
        let alive = {
            let mut session = self.session.borrow_mut();
            let session = session.as_mut().ok_or_else(|| {
                napi::Error::new(Status::GenericFailure, "no native window session is open")
            })?;
            let _guard = session.runtime.enter();
            session
                .event_loop
                .pump_app_events(Some(Duration::ZERO), &mut session.application);
            if let Some(error) = session.error.borrow_mut().take() {
                return Err(napi_error(error));
            }
            !session.application.inner.windows.is_empty()
                || !session.application.inner.pending_windows.is_empty()
        };
        if !alive {
            dom_bridge::window::publish(None);
            self.session.borrow_mut().take();
        }
        Ok(alive)
    }
}

#[cfg(test)]
mod tests {
    use blitsen_dom::DomName;

    use super::*;

    #[test]
    fn native_runtime_rejects_stale_generational_handles() {
        let mut dom = BlitzDom::from_html("<body><main id=host></main></body>", Default::default());
        let host = dom.get_element_by_id("host").unwrap().unwrap();
        let node = dom.create_element(&DomName::html("section")).unwrap();
        dom.append_child(host, node).unwrap();
        let runtime = DomRuntime::new(dom);
        let handle = DomRuntime::serialize_handle(node);

        runtime.retain_handle(&handle).unwrap();
        runtime.document().borrow_mut().remove(node).unwrap();
        assert_eq!(runtime.resolve_handle(&handle).unwrap(), node);
        assert!(runtime.release_handle(&handle).unwrap());
        assert!(runtime.resolve_handle(&handle).is_err());

        let replacement = runtime
            .document()
            .borrow_mut()
            .create_element(&DomName::html("aside"))
            .unwrap();
        assert_ne!(DomRuntime::serialize_handle(replacement), handle);
        assert!(runtime.resolve_handle("18446744073709551615").is_err());
    }
}
