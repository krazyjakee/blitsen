//! The Blitsen host, with no JavaScript engine named anywhere in it.
//!
//! Everything a shipped application needs between the DOM and the JavaScript
//! that drives it lives here: the native object graph, the window session, the
//! frame loop, and the headless harness the test suite runs. Each is generic
//! over [`blitsen_js::JsEngine`], which is what makes the Phase 1 → Phase 2
//! host change a swap rather than a rewrite (TECH.md §16.1).
//!
//! Two crates supply that engine. `blitsen-node` implements it over Node-API
//! for the Bun-hosted Phase 1 addon; `blitsen-runtime` implements it over
//! embedded JavaScriptCore for the Phase 2 executable. Neither appears below.

mod alloc;
// An application packaged into an APK's `assets/`, read in place (#144).
pub mod apk;
pub mod app;
// Proxy mode (#67): an application the user's own dev server is serving.
mod assets;
pub mod dev_server;
pub mod dom_bridge;
pub mod frame_loop;
pub mod harness;
pub mod messaging;
pub mod modules;
pub mod native_window;
mod pointer_input;
pub mod ports;
pub mod replay;
pub mod runtime_services;
pub mod standalone;
pub mod worker;

use std::cell::RefCell;
use std::rc::Rc;

use blitsen_blitz::BlitzDom;
use blitsen_dom::{DomBackend, DomError};
use blitsen_js::JsError;
use blitz::dom::NodeId;

pub use assets::validate_local_assets;
pub use native_window::{WindowApplication, WindowSession};

/// What a host needs to open a directory of static output in a native window.
#[derive(Clone, Debug)]
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

/// Wraps a DOM backend failure as a JavaScript-visible error.
pub fn dom_error(error: DomError) -> JsError {
    JsError::new(error.to_string())
}

/// Shared native DOM state addressed by serialized generational Blitz handles.
///
/// Every native bridge entry point parses the opaque handle and resolves it in
/// the authoritative document before performing work.
#[derive(Clone)]
pub struct DomRuntime {
    pub(crate) document: Rc<RefCell<BlitzDom>>,
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
        self.document.borrow().node_kind(node).map_err(dom_error)?;
        Ok(node)
    }

    /// Retains a detached node for one live JavaScript wrapper.
    pub fn retain_handle(&self, handle: &str) -> Result<(), JsError> {
        let node = self.resolve_handle(handle)?;
        self.document
            .borrow_mut()
            .retain_for_js(node)
            .map_err(dom_error)
    }

    /// Releases one wrapper and collects an otherwise-unowned detached subtree.
    pub fn release_handle(&self, handle: &str) -> Result<bool, JsError> {
        let node = self.resolve_handle(handle)?;
        self.document
            .borrow_mut()
            .release_from_js(node)
            .map_err(dom_error)
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
