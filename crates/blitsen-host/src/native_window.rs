//! The native window: winit application, input translation and frame pumping.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use blitsen_blitz::BlitzDom;
use blitsen_core::WindowState;
use blitsen_js::{JsEngine, JsError};
use blitz::dom::{DocGuard, DocGuardMut, Document as BlitzDocument};
use blitz::shell::BlitzApplication;
use winit::cursor::CursorIcon;
use winit::keyboard::ModifiersState;
use winit::window::WindowId;

use crate::drag_drop::PendingDrag;
use crate::pointer_input::{PendingPointerInput, PointerIds};
use crate::surface_lifecycle::{SurfaceState, SyntheticPhase};

mod borderless_resize;
pub(crate) mod gamepad;
pub(crate) mod hid;
mod input;
mod lifecycle;
pub(crate) mod menu;
pub(crate) mod notify;
mod renderer;
mod session;
mod startup_reveal;
mod state_sync;
pub(crate) mod tray;

use input::{ImeTarget, PendingKeyboardInput};
pub(crate) use input::{InputBootstrap, ModifierFlags, css_pointer_coordinates, take_queued_for};
pub use renderer::NativeWindowRenderer;
pub(crate) use renderer::native_window_renderer;
pub use session::WindowSession;

/// Hands the activity to the event loop, before there is one (issue #142).
///
/// [`WindowSession::open`] builds its loop with
/// `blitz::shell::create_default_event_loop`, whose Android branch reads the
/// `AndroidApp` back out of a `OnceLock` and unwraps it. So this must be called
/// first, and calling it is not something this crate can do for its caller: the
/// activity is handed to `android_main` and reaches no other function.
///
/// Re-exported here rather than reached for through `blitz::shell` directly so
/// that the ordering constraint is stated beside the code that imposes it. The
/// caller is `blitsen-android`, the `cdylib` that exists because Android's entry
/// point is not a `main`.
#[cfg(target_os = "android")]
pub fn set_android_app(app: android_activity::AndroidApp) {
    notify::set_android_app(app.clone());
    blitz::shell::set_android_app(app);
}

/// The winit application behind one window: input translation and dispatch.
pub struct WindowApplication<Rend: anyrender::WindowRenderer, E: JsEngine + Clone> {
    pub(crate) inner: BlitzApplication<Rend>,
    pub(crate) engine: E,
    pub(crate) state: Rc<RefCell<WindowState>>,
    pub(crate) error: Rc<RefCell<Option<JsError>>>,
    pub(crate) started_at: Instant,
    pub(crate) document: Rc<RefCell<BlitzDom>>,
    /// Host dispatch callbacks are strong engine references retained by Rust,
    /// never names application JavaScript can invoke or replace.
    pub(crate) host_hooks: crate::dom_bridge::HostHooks<E::StrongRef>,
    pub(crate) pending_pointer_input: Vec<(WindowId, PendingPointerInput)>,
    /// Raw device deltas waiting for the frame that delivers them to the
    /// pointer-lock element. Device events have no DOM target and must not be
    /// folded into absolute pointer hit testing.
    pub(crate) pending_locked_pointer_movement: Vec<(WindowId, (f64, f64))>,
    pub(crate) pending_keyboard_input: Vec<(WindowId, PendingKeyboardInput)>,
    /// The editable control each native window has enabled its IME for, and
    /// the last candidate-window area sent with it.
    pub(crate) ime_targets: HashMap<WindowId, ImeTarget>,
    pub(crate) pending_drag_input: Vec<(WindowId, PendingDrag)>,
    /// The files the drag currently over this application announced itself with.
    ///
    /// winit names them when the drag enters and again when it is released, and
    /// not on the moves in between, so the session's list is held here for the
    /// events that would otherwise carry none. Each queued event takes a share
    /// of it rather than reading it back at dispatch, so a drag that ends and a
    /// second that begins inside one turn cannot report each other's files.
    pub(crate) drag_paths: std::rc::Rc<[std::path::PathBuf]>,
    /// The last surface size winit reported, and the last one acted on.
    ///
    /// A drag reports a new size far faster than a size can be applied: every
    /// one costs a swapchain reconfigure, measured at ~30 ms on an X11 session
    /// because configuring waits for the frames already in flight. All but the
    /// last are stale before anything is painted, so they are collapsed here to
    /// one reconfigure, one layout and one `resize` event per turn — the size
    /// the window ended the turn at, which is the only one worth painting.
    pub(crate) pending_resize: HashMap<WindowId, winit::dpi::PhysicalSize<u32>>,
    pub(crate) applied_resize: HashMap<WindowId, winit::dpi::PhysicalSize<u32>>,
    pub(crate) pointer_positions: HashMap<WindowId, (f64, f64)>,
    /// The pointer position and tree revision the cursor was last resolved from.
    ///
    /// A hit test is the cost of resolving one, and neither a pointer that has
    /// not moved nor a tree that has not changed can have changed the answer.
    /// The position is kept as bits because it is compared, never measured.
    pub(crate) cursor_resolved_from: HashMap<WindowId, (u64, u64, u64)>,
    pub(crate) applied_cursor: HashMap<WindowId, Option<CursorIcon>>,
    /// DOM `pointerId`s, one per contact the platform is currently tracking.
    ///
    /// Which button is down under each of them, and which node it went down on,
    /// is JavaScript's — see `pointer_input`'s module comment.
    pub(crate) pointer_ids: PointerIds,
    pub(crate) modifiers: ModifiersState,
    pub(crate) load_dispatched: bool,
    /// Whether the application's first complete frame has been presented.
    ///
    /// The native window is created hidden. Mapping it before the renderer has
    /// a frame lets the compositor expose its uninitialised/default contents;
    /// on a cold wgpu start that is several visibly broken frames. The first
    /// redraw after critical resources and `load` paints while it is still
    /// hidden, then reveals it in the same callback.
    pub(crate) startup_revealed: bool,
    /// Whether that first frame ends with the window being mapped.
    ///
    /// False for a `hidden` window type, which asks to start unmapped: it still
    /// paints its first frame — the tray or `window.show()` must have something
    /// to reveal — but nothing maps it until one of them does.
    pub(crate) reveal_on_startup: bool,
    /// Whether the window has a surface to paint into; see `surface_lifecycle`.
    pub(crate) surface: SurfaceState,
    /// A synthetic surface cycle a test asked for, run at the next pump.
    pub(crate) synthetic_phase: Option<SyntheticPhase>,
    pub(crate) tray: Option<tray::TrayController>,
    /// The application menu, which exists independently of the tray.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    pub(crate) app_menu: Option<menu::AppMenuController>,
    pub(crate) notify: notify::NotifyController,
    pub(crate) hid: hid::HidController,
    pub(crate) gamepads: gamepad::Controller,
    pub(crate) quit_requested: bool,
    /// XSettings' toolkit scale, needed on X11 desktops that keep Xft/DPI at 96.
    pub(crate) system_scale_override: Option<f64>,
}

/// Forgets the window the `blitsen/window` module addresses.
///
/// Called when a session ends, so a later call reports "no window" instead of
/// reaching a destroyed one.
pub fn release_window() {
    crate::dom_bridge::window::publish(None);
}

/// Adapts Blitsen's shared document to the Blitz shell's document interface.
pub struct SharedBlitzDocument(pub Rc<RefCell<BlitzDom>>);

impl BlitzDocument for SharedBlitzDocument {
    fn inner(&self) -> DocGuard<'_> {
        DocGuard::RefCell(std::cell::Ref::map(self.0.borrow(), |document| {
            &**document.document_ref()
        }))
    }

    fn inner_mut(&mut self) -> DocGuardMut<'_> {
        DocGuardMut::RefCell(std::cell::RefMut::map(self.0.borrow_mut(), |document| {
            &mut **document.document_mut()
        }))
    }

    fn poll(&mut self, _task_context: Option<std::task::Context<'_>>) -> bool {
        let mut document = self.0.borrow_mut();
        let pending_before = document.document_ref().has_pending_critical_resources();
        let changes_before = document.document_ref().has_changes();
        document.document_mut().handle_messages();
        pending_before != document.document_ref().has_pending_critical_resources()
            || changes_before != document.document_ref().has_changes()
    }
}
