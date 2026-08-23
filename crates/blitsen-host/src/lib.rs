//! The Blitsen host, with no JavaScript engine named anywhere in it.
//!
//! Everything a shipped application needs between the DOM and the JavaScript
//! that drives it lives here: the native object graph, the window session, the
//! frame loop, and the headless harness the test suite runs. Each is generic
//! over [`blitsen_js::JsEngine`], which is what makes the Phase 1 → Phase 2
//! host change a swap rather than a rewrite (TECH.md §16.1).
//!
//! Two crates supply that engine. `blitsen-node` implements it over Node-API
//! for the Bun-hosted Phase 1 addon; `blitsen-quickjs` implements it over the
//! statically linked engine the Phase 2 executable hosts. Neither appears below.

mod alloc;
// An application packaged into an APK's `assets/`, read in place (#144).
pub mod apk;
pub mod app;
// Proxy mode (#67): an application the user's own dev server is serving.
mod assets;
pub mod dev_server;
pub mod dom_bridge;
// Files dragged from the desktop into the window, and the real paths they carry.
mod drag_drop;
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
// Surface loss and recreation: what a window that can be taken away needs (#146).
pub mod surface_lifecycle;
pub mod worker;

use std::cell::RefCell;
use std::rc::Rc;

use blitsen_blitz::BlitzDom;
use blitsen_dom::{DomBackend, DomError};
use blitsen_js::JsError;
use blitz::dom::NodeId;
use serde::{Deserialize, Serialize};

pub use assets::validate_local_assets;
pub use native_window::{WindowApplication, WindowSession};

/// How the application's first native window is presented.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowType {
    /// A decorated, visible window.
    #[default]
    Normal,
    /// A visible window without system decorations.
    Borderless,
    /// Borderless fullscreen on the current monitor.
    Fullscreen,
    /// Created without initially being shown.
    Hidden,
}

/// Options applied while the native window is created.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct NativeWindowOptions {
    /// Initial presentation type.
    #[serde(rename = "type")]
    pub window_type: WindowType,
    /// Whether the user may resize the window.
    pub resizable: bool,
    /// Whether the compositor should preserve surface alpha.
    pub transparent: bool,
    /// Whether the window requests an above-normal stacking level.
    pub always_on_top: bool,
}

impl Default for NativeWindowOptions {
    fn default() -> Self {
        Self {
            window_type: WindowType::Normal,
            resizable: true,
            transparent: false,
            always_on_top: false,
        }
    }
}

/// A built-in operation a declarative tray menu can perform.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TrayAction {
    /// Reveal and focus the application window.
    Show,
    /// Hide the application window.
    Hide,
    /// End the native window session.
    Quit,
    /// Draw a visual separator rather than an actionable entry.
    Separator,
}

impl TrayAction {
    /// Default user-facing label for an actionable entry.
    pub fn default_label(self) -> &'static str {
        match self {
            Self::Show => "Show",
            Self::Hide => "Hide",
            Self::Quit => "Quit",
            Self::Separator => "",
        }
    }
}

/// One configured entry in the system tray context menu.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TrayMenuItem {
    /// Operation selected by this entry.
    pub action: TrayAction,
    /// Optional label overriding the action's default.
    pub label: Option<String>,
    /// Whether the item accepts input.
    pub enabled: bool,
}

/// One declarative menu entry before platform-native objects are created.
///
/// The same shape describes a tray menu and an application menu: the two are
/// one tree installed on two surfaces, and which entries each surface accepts
/// is a decision the parser makes rather than a second type.
///
/// Runtime JavaScript supplies menu icons by index because their bytes travel
/// separately from the JSON tree. Packaged applications use the same shape:
/// the exporter records deterministic asset names, and the host reads those
/// assets into the indexed byte list before validating the menu.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MenuDefinition {
    /// Explicit kind (`action`, `separator`, `checkbox`, `radio`, `role`, or `submenu`).
    #[serde(rename = "type")]
    pub kind: Option<String>,
    /// Application-defined identity for action and checkable entries.
    pub id: Option<String>,
    /// Built-in `show`, `hide`, `quit`, or legacy `separator` tray action.
    pub action: Option<String>,
    /// Platform role carried by a `role` item or by an application submenu.
    pub role: Option<String>,
    /// User-facing item or submenu text.
    pub label: Option<String>,
    /// Whether the item accepts input.
    pub enabled: Option<bool>,
    /// Initial checkbox or radio state.
    pub checked: Option<bool>,
    /// Radio-group identity.
    pub group: Option<String>,
    /// Native keyboard accelerator.
    pub accelerator: Option<String>,
    /// Index into [`TrayMenu::icons`].
    pub icon_index: Option<usize>,
    /// Child entries for a submenu.
    pub menu: Option<Vec<Self>>,
}

/// A rich tray-menu tree and its decoded PNG payloads.
#[derive(Clone, Debug, Default)]
pub struct TrayMenu {
    /// Recursive menu definitions.
    pub entries: Vec<MenuDefinition>,
    /// PNG byte payloads addressed by each definition's `icon_index`.
    pub icons: Vec<Vec<u8>>,
}

impl Default for TrayMenuItem {
    fn default() -> Self {
        Self {
            action: TrayAction::Separator,
            label: None,
            enabled: true,
        }
    }
}

/// Decoded tray configuration ready for the platform implementation.
#[derive(Clone, Debug)]
pub struct TrayOptions {
    /// Encoded PNG bytes.
    pub icon: Vec<u8>,
    /// Optional platform tooltip.
    pub tooltip: Option<String>,
    /// Whether primary activation reveals the application window.
    pub open_on_click: bool,
    /// Whether the native close control hides rather than exits.
    pub close_to_tray: bool,
    /// Ordered context-menu entries.
    pub context_menu: Vec<TrayMenuItem>,
    /// Rich context-menu tree. When present, this replaces `context_menu`.
    pub menu: Option<TrayMenu>,
}

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
    /// Native creation-time window behavior.
    pub window: NativeWindowOptions,
    /// Optional system tray icon and menu.
    pub tray: Option<TrayOptions>,
    /// Optional application menu installed before the first frame.
    ///
    /// Separate from `tray` because the two have different owners. A status
    /// item is one optional piece of desktop furniture; an application menu
    /// belongs to the process itself, and an application that shows no status
    /// item at all must still be able to install one.
    pub menu: Option<Vec<MenuDefinition>>,
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
