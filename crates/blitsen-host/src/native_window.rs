//! The native window: winit application, input translation and frame pumping.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use blitsen_blitz::BlitzDom;
use blitsen_core::WindowState;
use blitsen_dom::{DomBackend, Rect};
use blitsen_js::{JsEngine, JsError};
use blitz::dom::{DocGuard, DocGuardMut, Document as BlitzDocument, NodeId};
use blitz::shell::BlitzApplication;
use serde::Serialize;
use winit::application::ApplicationHandler;
use winit::cursor::CursorIcon;
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event::{DeviceEvent, ElementState, Ime, StartCause, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, ModifiersState, PhysicalKey};
use winit::window::{ImeCapabilities, ImeEnableRequest, ImeRequest, ImeRequestData, WindowId};

#[cfg(target_os = "macos")]
use winit::application::macos::ApplicationHandlerExtMacOS;

use crate::drag_drop::PendingDrag;
use crate::pointer_input::{PendingPointerInput, PointerIds};
use crate::surface_lifecycle::{SurfaceState, SyntheticPhase};

pub(crate) mod hid;
pub(crate) mod menu;
pub(crate) mod notify;
mod session;
pub(crate) mod tray;

pub use session::WindowSession;

/// The window renderer safe for this target.
///
/// Vello's Metal compute path has caused full-session GPU resets on Intel Macs
/// (#229), while the API 32/33 Android AVD's lavapipe adapter exposes no usable
/// storage buffer and Vello panics during device creation (#151). Those targets
/// use the CPU rasterizer and a software framebuffer. Android retains an
/// explicit GPU qualification build; it is never selected automatically.
#[cfg(any(
    all(target_os = "macos", target_arch = "x86_64"),
    all(target_os = "android", not(feature = "android-vello-gpu"))
))]
pub type NativeWindowRenderer = anyrender_vello_cpu::VelloCpuWindowRenderer;

/// The window renderer safe for this target.
#[cfg(not(any(
    all(target_os = "macos", target_arch = "x86_64"),
    all(target_os = "android", not(feature = "android-vello-gpu"))
)))]
pub type NativeWindowRenderer = anyrender_vello::VelloWindowRenderer;

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
fn native_window_renderer() -> NativeWindowRenderer {
    eprintln!(
        "blitsen: renderer=vello-cpu window-backend=softbuffer \
         reason=Intel-macOS-Metal-safety-fallback"
    );
    NativeWindowRenderer::new()
}

#[cfg(all(target_os = "android", not(feature = "android-vello-gpu")))]
fn native_window_renderer() -> NativeWindowRenderer {
    eprintln!(
        "blitsen: renderer=vello-cpu window-backend=softbuffer \
         reason=Android-safe-default gpu-qualification-feature=android-vello-gpu"
    );
    NativeWindowRenderer::new()
}

#[cfg(all(target_os = "android", feature = "android-vello-gpu"))]
fn native_window_renderer() -> NativeWindowRenderer {
    eprintln!(
        "blitsen: renderer=vello-gpu backend=wgpu \
         qualification=Android-mobile-GPU feature=android-vello-gpu"
    );
    NativeWindowRenderer::new()
}

#[cfg(not(any(
    target_os = "android",
    all(target_os = "macos", target_arch = "x86_64")
)))]
fn native_window_renderer() -> NativeWindowRenderer {
    eprintln!("blitsen: renderer=vello-gpu backend=wgpu");
    NativeWindowRenderer::new()
}

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
    /// Host dispatch callbacks are engine values retained by Rust, never names
    /// application JavaScript can invoke or replace.
    pub(crate) host_hooks: crate::dom_bridge::HostHooks<E::Value>,
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
    pub(crate) quit_requested: bool,
}

#[derive(Clone)]
pub(crate) enum PendingKeyboardInput {
    Key {
        event_type: &'static str,
        key: String,
        code: String,
        repeat: bool,
    },
    Ime(Ime),
    WindowFocus(bool),
    WindowModeRelease {
        pointer: bool,
        fullscreen: bool,
        reason: &'static str,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ImeTarget {
    node: NodeId,
    area: Rect,
}

/// Modifier state shared by keyboard and pointer event initializer bags.
#[derive(Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModifierFlags {
    ctrl_key: bool,
    shift_key: bool,
    alt_key: bool,
    meta_key: bool,
}

impl From<ModifiersState> for ModifierFlags {
    fn from(modifiers: ModifiersState) -> Self {
        Self {
            ctrl_key: modifiers.control_key(),
            shift_key: modifiers.shift_key(),
            alt_key: modifiers.alt_key(),
            meta_key: modifiers.meta_key(),
        }
    }
}

/// The input dispatcher in the DOM bootstrap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InputBootstrap {
    Keyboard,
    Ime,
    Pointer,
    Mouse,
    Drag,
}

impl InputBootstrap {
    #[cfg(test)]
    fn entry_point(self) -> &'static str {
        match self {
            Self::Keyboard => "__blitsenDispatchKeyboardEvent",
            Self::Ime => "__blitsenDispatchImeEvent",
            Self::Pointer => "__blitsenDispatchPointerEvent",
            Self::Mouse => "__blitsenDispatchMouseEvent",
            Self::Drag => "__blitsenDispatchDragEvent",
        }
    }

    fn script_name(self) -> &'static str {
        match self {
            Self::Keyboard => "blitsen:native-keyboard-event",
            Self::Ime => "blitsen:native-ime-event",
            Self::Pointer | Self::Mouse => "blitsen:native-pointer-input",
            Self::Drag => "blitsen:native-drag-input",
        }
    }

    fn hook<V>(self, hooks: &crate::dom_bridge::HostHooks<V>) -> &V {
        match self {
            Self::Keyboard => &hooks.keyboard,
            Self::Ime => &hooks.ime,
            Self::Pointer => &hooks.pointer,
            Self::Mouse => &hooks.mouse,
            Self::Drag => &hooks.drag,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KeyboardEventInit {
    bubbles: bool,
    cancelable: bool,
    key: String,
    code: String,
    repeat: bool,
    #[serde(flatten)]
    modifiers: ModifierFlags,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImeEventInit {
    data: String,
    cursor_start: Option<usize>,
    cursor_end: Option<usize>,
    before_bytes: Option<usize>,
    after_bytes: Option<usize>,
}

fn ime_call(event: Ime) -> (&'static str, ImeEventInit) {
    match event {
        Ime::Enabled => ("enabled", ImeEventInit::default()),
        Ime::Disabled => ("disabled", ImeEventInit::default()),
        Ime::Preedit(data, cursor) => {
            let (cursor_start, cursor_end) = cursor.unzip();
            (
                "preedit",
                ImeEventInit {
                    data,
                    cursor_start,
                    cursor_end,
                    ..Default::default()
                },
            )
        }
        Ime::Commit(data) => (
            "commit",
            ImeEventInit {
                data,
                ..Default::default()
            },
        ),
        Ime::DeleteSurrounding {
            before_bytes,
            after_bytes,
        } => (
            "deleteSurrounding",
            ImeEventInit {
                before_bytes: Some(before_bytes),
                after_bytes: Some(after_bytes),
                ..Default::default()
            },
        ),
    }
}

fn ime_request_data(area: Rect) -> ImeRequestData {
    ImeRequestData::default().with_cursor_area(
        LogicalPosition::new(f64::from(area.x), f64::from(area.y)).into(),
        LogicalSize::new(f64::from(area.width), f64::from(area.height)).into(),
    )
}

fn ime_enable_request(area: Rect) -> ImeRequest {
    ImeRequest::Enable(
        ImeEnableRequest::new(
            ImeCapabilities::new().with_cursor_area(),
            ime_request_data(area),
        )
        .expect("cursor-area capability has cursor-area data"),
    )
}

/// Window-relative physical pixels as the DOM's `client` and `screen` pairs.
///
/// Shared by every input this window dispatches: a pointer, a wheel and a
/// dragged file all arrive in physical pixels from the window's top-left corner
/// and all report CSS pixels to JavaScript.
pub(crate) fn css_pointer_coordinates(
    physical_x: f64,
    physical_y: f64,
    scale: f64,
    screen_origin_x: f64,
    screen_origin_y: f64,
) -> (f64, f64, f64, f64) {
    let client_x = physical_x / scale;
    let client_y = physical_y / scale;
    (
        client_x,
        client_y,
        screen_origin_x + client_x,
        screen_origin_y + client_y,
    )
}

/// Takes one key's queued values in order, unless an earlier callback failed.
///
/// An already parked error leaves the queue untouched so surfacing that error
/// cannot silently consume input. Once draining starts, all matching values are
/// removed before dispatch, preserving the existing rule that a dispatch error
/// drops the rest of that window's turn rather than replaying it later.
pub(crate) fn take_queued_for<K: PartialEq, T: Clone, Error>(
    parked_error: &RefCell<Option<Error>>,
    queue: &mut Vec<(K, T)>,
    key: &K,
) -> Option<Vec<T>> {
    if parked_error.borrow().is_some() {
        return None;
    }
    let mut taken = Vec::new();
    queue.retain(|(queued_key, value)| {
        if queued_key == key {
            taken.push(value.clone());
            false
        } else {
            true
        }
    });
    Some(taken)
}

/// Parks an error only when no earlier callback error is waiting to surface.
fn park_first_error<Error>(parked_error: &RefCell<Option<Error>>, error: Error) {
    let mut parked_error = parked_error.borrow_mut();
    if parked_error.is_none() {
        *parked_error = Some(error);
    }
}

#[cfg(test)]
fn input_call_script(
    bootstrap: InputBootstrap,
    arguments: &impl Serialize,
) -> Result<String, JsError> {
    let arguments =
        serde_json::to_string(arguments).map_err(|error| JsError::new(error.to_string()))?;
    Ok(format!(
        "globalThis.{}(...{arguments})",
        bootstrap.entry_point()
    ))
}

/// Forgets the window the `native:window` module addresses.
///
/// Called when a session ends, so a later call reports "no window" instead of
/// reaching a destroyed one.
pub fn release_window() {
    crate::dom_bridge::window::publish(None);
}

pub(crate) fn dom_key_name(key: &Key) -> String {
    match key {
        Key::Character(character) => character.to_string(),
        Key::Named(named) => format!("{named:?}"),
        Key::Dead(_) => "Dead".into(),
        Key::Unidentified(_) => "Unidentified".into(),
    }
}

pub(crate) fn dom_key_code(key: PhysicalKey) -> String {
    match key {
        PhysicalKey::Code(code) => format!("{code:?}"),
        PhysicalKey::Unidentified(_) => String::new(),
    }
}

impl<Rend: anyrender::WindowRenderer, E: JsEngine + Clone> WindowApplication<Rend, E> {
    /// Delivers everything the tray and the application menu raised this turn.
    ///
    /// muda's menu-event channel is one channel for every menu in the process,
    /// so it is drained here rather than by either owner: whichever looked
    /// first would take the other's clicks. Each owner then claims the ids its
    /// own bindings recognise, and an id neither claims belonged to a menu that
    /// has since been replaced.
    fn apply_menu_signals(&mut self, event_loop: &dyn ActiveEventLoop) {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        let native = menu::take_native_menu_events();
        let tray_signals = match &self.tray {
            Some(tray) => {
                #[cfg(any(target_os = "windows", target_os = "macos"))]
                tray.claim(&native);
                tray.poll();
                tray.take_signals()
            }
            None => Vec::new(),
        };
        for signal in tray_signals {
            match signal {
                menu::MenuSignal::Command(crate::TrayAction::Show) => {
                    for view in self.inner.windows.values() {
                        view.window.set_visible(true);
                        view.window.focus_window();
                        view.window.request_redraw();
                    }
                }
                menu::MenuSignal::Command(crate::TrayAction::Hide) => {
                    for view in self.inner.windows.values() {
                        view.window.set_visible(false);
                    }
                }
                menu::MenuSignal::Command(crate::TrayAction::Quit) => {
                    self.quit_requested = true;
                    event_loop.exit();
                }
                menu::MenuSignal::Command(crate::TrayAction::Separator) => {}
                menu::MenuSignal::Click => crate::dom_bridge::tray::clicked(),
                menu::MenuSignal::Action { id, checked } => {
                    crate::dom_bridge::tray::action(id, checked);
                }
            }
        }
        // The application menu raises nothing but application-defined actions:
        // its roles are the platform's own commands and never enter JavaScript,
        // which is what separates a role from a custom item.
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        if let Some(app_menu) = &self.app_menu {
            app_menu.claim(&native);
            for signal in app_menu.take_signals() {
                if let menu::MenuSignal::Action { id, checked } = signal {
                    crate::dom_bridge::menu::action(id, checked);
                }
            }
        }
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        let menu_pending = crate::dom_bridge::menu::pending();
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        let menu_pending = false;
        if crate::dom_bridge::tray::pending() || menu_pending {
            for view in self.inner.windows.values() {
                view.window.request_redraw();
            }
        }
    }

    /// Whether a callback error is waiting for [`WindowSession::pump`] to take it.
    pub(crate) fn has_parked_error(&self) -> bool {
        self.error.borrow().is_some()
    }

    /// Retains the first callback error; later cascade errors cannot replace it.
    pub(crate) fn park_error(&self, error: JsError) {
        park_first_error(self.error.as_ref(), error);
    }

    /// Returns the error that stops JavaScript from running again this turn.
    fn parked_error(&self) -> Option<JsError> {
        self.error.borrow().clone()
    }

    /// Calls one typed input entry point with JSON-serialized positional arguments.
    pub(crate) fn call_input_bootstrap(
        &self,
        bootstrap: InputBootstrap,
        arguments: &impl Serialize,
    ) -> Result<bool, JsError> {
        if let Some(error) = self.parked_error() {
            return Err(error);
        }
        let arguments =
            serde_json::to_string(arguments).map_err(|error| JsError::new(error.to_string()))?;
        let mut engine = self.engine.clone();
        let arguments = engine.evaluate_script(&arguments, bootstrap.script_name())?;
        let arguments = engine.to_array(&arguments)?;
        let hook = bootstrap.hook(&self.host_hooks).clone();
        let result = engine.call(&hook, None, &arguments)?;
        engine.to_boolean(&result)
    }

    /// Snapshots the modifiers that every queued input in this turn observes.
    pub(crate) fn modifier_flags(&self) -> ModifierFlags {
        self.modifiers.into()
    }

    /// The scale factor and screen origin one window's input resolves against.
    ///
    /// `None` once the window is gone, which is a turn whose queued input has
    /// nowhere to land rather than an error.
    pub(crate) fn window_geometry(&self, window_id: WindowId) -> Option<(f64, f64, f64)> {
        self.inner.windows.get(&window_id).map(|view| {
            let scale = f64::from(view.doc.inner().viewport().hidpi_scale);
            let origin = view.window.outer_position().unwrap_or_default();
            (
                scale,
                f64::from(origin.x) / scale,
                f64::from(origin.y) / scale,
            )
        })
    }

    /// Resolves a viewport point to the node under it, against a settled layout.
    ///
    /// Every input this window dispatches picks its target this way, so the
    /// flush belongs here rather than at each caller: a hit test read against a
    /// dirty tree answers where an element was before the frame moved it.
    pub(crate) fn hit_test(
        &self,
        client_x: f64,
        client_y: f64,
    ) -> Result<Option<blitsen_dom::HitTest<NodeId>>, blitsen_dom::DomError> {
        let snapshot = self.document.borrow_mut().flush_layout()?;
        self.document
            .borrow()
            .hit_test(client_x as f32, client_y as f32, snapshot)
    }

    fn queue_keyboard_input(&mut self, window_id: WindowId, event: &WindowEvent) -> bool {
        let input = match event {
            WindowEvent::KeyboardInput { event, .. } => {
                let pressed = event.state == ElementState::Pressed;
                let key = dom_key_name(&event.logical_key);
                let code = dom_key_code(event.physical_key);
                crate::dom_bridge::input::key(code.clone(), key.clone(), pressed);
                PendingKeyboardInput::Key {
                    event_type: if pressed { "keydown" } else { "keyup" },
                    key,
                    code,
                    repeat: event.repeat,
                }
            }
            WindowEvent::Focused(focused) => {
                crate::dom_bridge::input::focus(*focused);
                PendingKeyboardInput::WindowFocus(*focused)
            }
            WindowEvent::Ime(event) => PendingKeyboardInput::Ime(event.clone()),
            _ => return false,
        };
        self.pending_keyboard_input.push((window_id, input));
        true
    }

    fn dispatch_keyboard_event(
        &self,
        event_type: &str,
        init: &KeyboardEventInit,
    ) -> Result<bool, JsError> {
        self.call_input_bootstrap(InputBootstrap::Keyboard, &(event_type, init))
    }

    fn dispatch_ime_event(&self, event: Ime) -> Result<bool, JsError> {
        let (kind, init) = ime_call(event);
        self.call_input_bootstrap(InputBootstrap::Ime, &(kind, init))
    }

    pub(crate) fn drain_keyboard_input(&mut self, window_id: WindowId) {
        let Some(inputs) = take_queued_for(
            self.error.as_ref(),
            &mut self.pending_keyboard_input,
            &window_id,
        ) else {
            return;
        };
        for (index, input) in inputs.iter().enumerate() {
            // Winit guarantees an empty preedit immediately before a commit.
            // It exists to make editors that treat those as independent native
            // operations clear their marked range; our commit operation
            // replaces that range atomically. Hiding this synthetic pair from
            // JavaScript avoids an observable empty `input` between the last
            // composition update and its committed value. A standalone empty
            // preedit (cancellation) is still dispatched normally.
            if matches!(input, PendingKeyboardInput::Ime(Ime::Preedit(text, None)) if text.is_empty())
                && matches!(
                    inputs.get(index + 1),
                    Some(PendingKeyboardInput::Ime(Ime::Commit(_)))
                )
            {
                continue;
            }
            let result = match input {
                PendingKeyboardInput::Key {
                    event_type,
                    key,
                    code,
                    repeat,
                } => self.dispatch_keyboard_event(
                    event_type,
                    &KeyboardEventInit {
                        bubbles: true,
                        cancelable: true,
                        key: key.clone(),
                        code: code.clone(),
                        repeat: *repeat,
                        modifiers: self.modifier_flags(),
                    },
                ),
                PendingKeyboardInput::Ime(event) => self.dispatch_ime_event(event.clone()),
                PendingKeyboardInput::WindowFocus(focused) => {
                    let mut engine = self.engine.clone();
                    engine
                        .evaluate_script(
                            &format!(
                                "globalThis.dispatchEvent(new Event({}))",
                                if *focused { "\"focus\"" } else { "\"blur\"" }
                            ),
                            "blitsen:native-window-focus",
                        )
                        .and_then(|value| engine.to_boolean(&value))
                }
                PendingKeyboardInput::WindowModeRelease {
                    pointer,
                    fullscreen,
                    reason,
                } => {
                    let reason = serde_json::to_string(reason)
                        .map_err(|error| JsError::new(error.to_string()));
                    reason.and_then(|reason| {
                        let mut engine = self.engine.clone();
                        let reason = engine.string(&reason)?;
                        let pointer = engine.boolean(*pointer);
                        let fullscreen = engine.boolean(*fullscreen);
                        let hook = self.host_hooks.release_window_modes.clone();
                        let value = engine.call(&hook, None, &[pointer, fullscreen, reason])?;
                        engine.to_boolean(&value)
                    })
                }
            };
            if let Err(error) = result {
                self.park_error(error);
                return;
            }
        }
    }

    /// Enables the platform IME only for the focused editable control and
    /// keeps its candidate window beside the painted caret.
    fn sync_ime(&mut self, window_id: WindowId) -> Result<(), JsError> {
        let next = self.document.borrow().focused_form_cursor_area();
        let current = self.ime_targets.get(&window_id).copied();
        let Some(view) = self.inner.windows.get(&window_id) else {
            self.ime_targets.remove(&window_id);
            return Ok(());
        };

        if current.map(|target| target.node) != next.map(|(node, _)| node) {
            if current.is_some() {
                view.window
                    .request_ime_update(ImeRequest::Disable)
                    .map_err(|error| JsError::new(format!("could not disable IME: {error}")))?;
            }
            match next {
                Some((node, area)) => {
                    view.window
                        .request_ime_update(ime_enable_request(area))
                        .map_err(|error| JsError::new(format!("could not enable IME: {error}")))?;
                    self.ime_targets.insert(window_id, ImeTarget { node, area });
                }
                None => {
                    self.ime_targets.remove(&window_id);
                }
            }
            return Ok(());
        }

        if let (Some(current), Some((node, area))) = (current, next)
            && current.area != area
        {
            view.window
                .request_ime_update(ImeRequest::Update(ime_request_data(area)))
                .map_err(|error| {
                    JsError::new(format!("could not update IME cursor area: {error}"))
                })?;
            self.ime_targets.insert(window_id, ImeTarget { node, area });
        }
        Ok(())
    }

    fn drain_locked_pointer_movement(&mut self, window_id: WindowId) {
        let Some(movements) = take_queued_for(
            self.error.as_ref(),
            &mut self.pending_locked_pointer_movement,
            &window_id,
        ) else {
            return;
        };
        for (x, y) in movements {
            let result = (|| {
                let mut engine = self.engine.clone();
                let x = engine.number(x);
                let y = engine.number(y);
                let hook = self.host_hooks.locked_pointer_motion.clone();
                let value = engine.call(&hook, None, &[x, y])?;
                engine.to_boolean(&value)
            })();
            if let Err(error) = result {
                self.park_error(error);
                return;
            }
        }
    }

    /// Restores security-sensitive window modes immediately, then queues their
    /// observable DOM changes in the same ordered frame input stream as focus.
    pub(crate) fn release_web_window_modes(&mut self, window_id: WindowId, reason: &'static str) {
        let (pointer, fullscreen) = crate::dom_bridge::window::release_web_modes();
        if !pointer && !fullscreen {
            return;
        }
        self.pending_locked_pointer_movement.clear();
        self.pending_keyboard_input.push((
            window_id,
            PendingKeyboardInput::WindowModeRelease {
                pointer,
                fullscreen,
                reason,
            },
        ));
    }

    fn animation_frames_pending(&self) -> bool {
        if self.has_parked_error() {
            return false;
        }
        let result = (|| {
            let mut engine = self.engine.clone();
            let pending = engine.evaluate_script(
                "globalThis.__blitsenAnimationFramesPending()",
                "blitsen:animation-frame-pending",
            )?;
            engine.to_boolean(&pending)
        })();
        match result {
            Ok(pending) => pending,
            Err(error) => {
                self.park_error(error);
                false
            }
        }
    }

    fn run_animation_frame(&self) -> bool {
        if self.has_parked_error() {
            return false;
        }
        let timestamp = self.started_at.elapsed().as_secs_f64() * 1_000.0;
        let result = (|| {
            let mut engine = self.engine.clone();
            let pending = engine.evaluate_script(
                &format!("globalThis.__blitsenAnimationFrameTick({timestamp})"),
                "blitsen:animation-frame-tick",
            )?;
            engine.drain_microtasks()?;
            Ok(engine.to_number(&pending)? > 0.0)
        })();
        match result {
            Ok(pending) => pending,
            Err(error) => {
                self.park_error(error);
                false
            }
        }
    }

    fn sync_window(&self, width: u32, height: u32, device_pixel_ratio: f64) {
        if self.has_parked_error() {
            return;
        }
        *self.state.borrow_mut() = WindowState::new(width, height, device_pixel_ratio);
        let result = (|| {
            let mut engine = self.engine.clone();
            let window = engine.evaluate_script("globalThis", "blitsen:window-resize-target")?;
            self.state.borrow().sync(&mut engine, &window)
        })();
        if let Err(error) = result {
            self.park_error(error);
        }
    }

    /// Puts the cursor the document resolves under the pointer on the window.
    ///
    /// Blitz sets a cursor of its own from its hover state, and its hover hit
    /// test cannot reach an element laid out past its parent's box, so this runs
    /// after the frame Blitz painted and has the last word (issue #128).
    ///
    /// Resolved once per frame rather than once per pointer move: a cursor is
    /// also changed by a class or an inline style landing under a pointer that
    /// never moved, and a frame is the moment both are settled. The pointer
    /// position and the tree revision together say whether the answer could have
    /// changed at all, which keeps a still pointer over a still document from
    /// paying for a hit test every frame.
    fn sync_cursor(&mut self, window_id: WindowId) {
        if self.has_parked_error() {
            return;
        }
        let Some(&(physical_x, physical_y)) = self.pointer_positions.get(&window_id) else {
            return;
        };
        let Some(scale) = self
            .inner
            .windows
            .get(&window_id)
            .map(|view| f64::from(view.doc.inner().viewport().hidpi_scale))
        else {
            return;
        };
        let client_x = physical_x / scale;
        let client_y = physical_y / scale;
        let revision = match self.document.borrow_mut().flush_layout() {
            Ok(snapshot) => snapshot.revision(),
            Err(error) => {
                self.park_error(crate::dom_error(error));
                return;
            }
        };
        let source = (client_x.to_bits(), client_y.to_bits(), revision);
        if self.cursor_resolved_from.get(&window_id) == Some(&source) {
            return;
        }
        self.cursor_resolved_from.insert(window_id, source);
        let icon = match self
            .document
            .borrow()
            .cursor_at(client_x as f32, client_y as f32)
        {
            Ok(icon) => icon,
            Err(error) => {
                self.park_error(crate::dom_error(error));
                return;
            }
        };
        if self.applied_cursor.get(&window_id) == Some(&icon) {
            return;
        }
        self.applied_cursor.insert(window_id, icon);
        let Some(view) = self.inner.windows.get(&window_id) else {
            return;
        };
        // `cursor: none` is a hidden pointer rather than an arrow, which is the
        // one value winit spells with a second call.
        match icon {
            Some(icon) => {
                view.window.set_cursor(icon.into());
                view.window.set_cursor_visible(true);
            }
            None => view.window.set_cursor_visible(false),
        }
    }

    pub(crate) fn sync_native_window(&self, window_id: WindowId) {
        let Some((width, height, scale)) = self.inner.windows.get(&window_id).map(|view| {
            let document = view.doc.inner();
            let viewport = document.viewport();
            let logical =
                winit::dpi::PhysicalSize::new(viewport.window_size.0, viewport.window_size.1)
                    .to_logical::<u32>(f64::from(viewport.hidpi_scale));
            (logical.width, logical.height, viewport.hidpi_scale)
        }) else {
            return;
        };
        self.sync_window(width, height, f64::from(scale));
    }

    fn dispatch_window_event(&self, event_type: &str) -> Result<bool, JsError> {
        if let Some(error) = self.parked_error() {
            return Err(error);
        }
        let event_type =
            serde_json::to_string(event_type).map_err(|error| JsError::new(error.to_string()))?;
        let mut engine = self.engine.clone();
        let result = engine.evaluate_script(
            &format!("globalThis.__blitsenDispatchLifecycleEvent({event_type})"),
            "blitsen:native-window-event",
        )?;
        engine.to_boolean(&result)
    }

    /// Hands `native:window` the window it acts on, or takes it away.
    ///
    /// There is deliberately only one in the current host. Issue #105 chooses
    /// isolated JavaScript contexts for future windows, so `create` remains
    /// absent until this session can publish the window belonging to the
    /// calling context instead of one process-wide slot.
    pub(crate) fn publish_window(&self) {
        crate::dom_bridge::window::publish(
            self.inner
                .windows
                .values()
                .next()
                .map(|view| Arc::clone(&view.window)),
        );
    }

    /// Reports a resize winit applied outright, which raises no event of its own.
    ///
    /// Wayland resizes the surface there and then; X11 asks the server and the
    /// answer arrives as `SurfaceResized`. Feeding the Wayland answer through
    /// the same event keeps one path to the viewport, `innerWidth` and the
    /// `resize` event rather than a second one that could disagree.
    fn settle_native_resize(&mut self, event_loop: &dyn ActiveEventLoop) {
        let Some(size) = crate::dom_bridge::window::take_applied_resize() else {
            return;
        };
        let Some(&window_id) = self.inner.windows.keys().next() else {
            return;
        };
        self.window_event(event_loop, window_id, WindowEvent::SurfaceResized(size));
    }

    /// Applies the size the window arrived at, if it is not the one it is at.
    ///
    /// The whole cost of a resize is here — the swapchain reconfigure, the
    /// relayout the new viewport forces, and the `resize` listeners an
    /// application registered — so a size that changes nothing is dropped
    /// rather than paid for. Winit reports the size again on every configure a
    /// window manager sends, including the ones that only moved it.
    fn apply_pending_resize(&mut self, event_loop: &dyn ActiveEventLoop, window_id: WindowId) {
        // Leave the resize queued until `pump` has surfaced the earlier error.
        if self.has_parked_error() {
            return;
        }
        let Some(size) = self.pending_resize.remove(&window_id) else {
            return;
        };
        if self.applied_resize.get(&window_id) == Some(&size) {
            return;
        }
        self.applied_resize.insert(window_id, size);
        self.inner
            .window_event(event_loop, window_id, WindowEvent::SurfaceResized(size));
        self.sync_native_window(window_id);
        if !self.has_parked_error()
            && let Err(error) = self.dispatch_window_event("resize")
        {
            self.park_error(error);
        }
        if let Some(view) = self.inner.windows.get(&window_id) {
            view.window.request_redraw();
        }
    }

    pub(crate) fn maybe_dispatch_load(&mut self) {
        if self.load_dispatched || self.has_parked_error() {
            return;
        }
        if self
            .document
            .borrow()
            .document_ref()
            .has_pending_critical_resources()
        {
            return;
        }
        match self.dispatch_window_event("load") {
            Ok(_) => {
                self.load_dispatched = true;
                for view in self.inner.windows.values() {
                    view.window.request_redraw();
                }
            }
            Err(error) => self.park_error(error),
        }
    }

    /// Makes the next redraw the startup frame, if everything it needs is ready.
    ///
    /// `blitz-shell` suppresses ordinary redraws for a view it considers
    /// invisible, so its view-side flag is raised before painting while the
    /// actual platform window remains hidden. [`finish_startup_reveal`] maps it
    /// only after that paint has returned.
    fn prepare_startup_reveal(&mut self, window_id: WindowId) -> bool {
        if self.startup_revealed
            || !self.load_dispatched
            || self.has_parked_error()
            || self.surface.is_lost()
            || self
                .document
                .borrow()
                .document_ref()
                .has_pending_critical_resources()
        {
            return false;
        }
        let Some(view) = self.inner.windows.get_mut(&window_id) else {
            return false;
        };
        if !view.renderer.is_active() {
            return false;
        }
        view.is_visible = true;
        true
    }

    /// Asks for the hidden paint once renderer and document readiness coincide.
    fn request_startup_redraw_if_ready(&self) {
        if self.startup_revealed
            || !self.load_dispatched
            || self.has_parked_error()
            || self.surface.is_lost()
            || self
                .document
                .borrow()
                .document_ref()
                .has_pending_critical_resources()
        {
            return;
        }
        for view in self.inner.windows.values() {
            if view.renderer.is_active() {
                view.window.request_redraw();
            }
        }
    }

    /// Maps the native window after its prepared startup redraw was submitted.
    fn finish_startup_reveal(&mut self, window_id: WindowId) {
        if self.has_parked_error()
            || self
                .document
                .borrow()
                .document_ref()
                .has_pending_critical_resources()
        {
            // A first-frame callback may have started another critical load.
            // Keep the native window hidden and let that resource's wake-up ask
            // for the eventual startup frame.
            if let Some(view) = self.inner.windows.get_mut(&window_id) {
                view.is_visible = false;
            }
            return;
        }
        let Some(view) = self.inner.windows.get(&window_id) else {
            return;
        };
        if self.reveal_on_startup {
            view.window.set_visible(true);
        }
        // Set either way: what it gates below is ordinary redraws, and a hidden
        // window that never set it would never animate or paint again.
        self.startup_revealed = true;
    }
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

impl<Rend: anyrender::WindowRenderer, E: JsEngine + Clone> ApplicationHandler
    for WindowApplication<Rend, E>
{
    fn new_events(&mut self, event_loop: &dyn ActiveEventLoop, cause: StartCause) {
        self.inner.new_events(event_loop, cause);
        if cause == StartCause::Init
            && let Some(tray) = &mut self.tray
            && let Err(error) = tray.initialize()
        {
            self.park_error(JsError::new(error));
        }
        self.apply_menu_signals(event_loop);
    }

    fn resumed(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.on_resumed(event_loop);
    }

    fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.on_can_create_surfaces(event_loop);
    }

    fn proxy_wake_up(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.inner.proxy_wake_up(event_loop);
        self.apply_menu_signals(event_loop);
        self.maybe_dispatch_load();
        // Renderer readiness and resource completion both arrive through the
        // proxy. Whichever one was last now schedules the hidden startup paint.
        self.request_startup_redraw_if_ready();
    }

    fn window_event(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        // Before anything else this turn does with the event, and whatever else
        // it does: the native snapshot is what an application polls instead of
        // listening, so it has to reflect the pointer even on the events this
        // handler goes on to consume itself.
        crate::dom_bridge::input::observe(
            &event,
            self.inner
                .windows
                .get(&window_id)
                .map_or(1.0, |view| view.window.scale_factor()),
        );
        if matches!(event, WindowEvent::CloseRequested)
            && self
                .tray
                .as_ref()
                .is_some_and(tray::TrayController::close_to_tray)
        {
            if let Some(view) = self.inner.windows.get(&window_id) {
                view.window.set_visible(false);
            }
            return;
        }
        // Held rather than acted on, and applied below once the turn's last one
        // is known. Winit has already coalesced the redraw requests that follow
        // it, so the frame this turn paints is the one that pays for the size.
        if let WindowEvent::SurfaceResized(size) = event {
            self.pending_resize.insert(window_id, size);
            if let Some(view) = self.inner.windows.get(&window_id) {
                view.window.request_redraw();
            }
            return;
        }
        if matches!(&event, WindowEvent::Focused(false)) {
            self.release_web_window_modes(window_id, "focus-loss");
        }
        let suppress_absolute_pointer = crate::dom_bridge::window::web_pointer_locked()
            && matches!(
                &event,
                WindowEvent::PointerMoved { .. }
                    | WindowEvent::PointerEntered { .. }
                    | WindowEvent::PointerLeft { .. }
            );
        let queued_pointer_input =
            !suppress_absolute_pointer && self.queue_pointer_input(window_id, &event);
        let queued_keyboard_input = self.queue_keyboard_input(window_id, &event);
        let queued_drag_input = self.queue_drag_input(window_id, &event);
        // Blitz has its own editor-side IME handler, but it knows nothing about
        // this runtime's DOM events. Letting the same event continue there
        // would mutate the shared editor before `compositionupdate` and then
        // mutate it a second time when the bridge applies the default action.
        if matches!(&event, WindowEvent::Ime(_)) {
            if let Some(view) = self.inner.windows.get(&window_id) {
                view.window.request_redraw();
            }
            return;
        }
        let viewport_changed = matches!(&event, WindowEvent::ScaleFactorChanged { .. });
        let redraw = matches!(&event, WindowEvent::RedrawRequested);
        let startup_paint = redraw && self.prepare_startup_reveal(window_id);
        if redraw {
            // Before the frame rather than after it: a redraw that painted the
            // size before last would be a frame the drag visibly lagged by.
            self.apply_pending_resize(event_loop, window_id);
            self.drain_locked_pointer_movement(window_id);
            self.drain_pointer_input(window_id);
            self.drain_keyboard_input(window_id);
            self.drain_drag_input(window_id);
        }
        // `requestAnimationFrame` means "before the next paint", and a window
        // with no surface has no next paint. Android's winit backend stops
        // dispatching redraws entirely while the app is stopped; the desktop
        // backends have no such gate, so the rule is applied here instead and
        // means the same thing on every target (see `surface_lifecycle`).
        let animation_pending = redraw
            && !self.surface.is_lost()
            && (self.startup_revealed || startup_paint)
            && self.run_animation_frame();
        // A startup rAF is allowed to discover another critical resource. Do
        // not let blitz-shell paint (or the platform map) until it has settled.
        let startup_paint = startup_paint
            && !self.has_parked_error()
            && !self
                .document
                .borrow()
                .document_ref()
                .has_pending_critical_resources();
        if !startup_paint
            && !self.startup_revealed
            && let Some(view) = self.inner.windows.get_mut(&window_id)
        {
            view.is_visible = false;
        }
        self.inner.window_event(event_loop, window_id, event);
        if redraw
            && !self.has_parked_error()
            && let Err(error) = self.sync_ime(window_id)
        {
            self.park_error(error);
        }
        if startup_paint {
            self.finish_startup_reveal(window_id);
        }
        // After Blitz has had the frame, because painting it re-resolves Blitz's
        // own hover state and sets a cursor from it.
        if redraw && !crate::dom_bridge::window::web_pointer_locked() {
            self.sync_cursor(window_id);
        }
        if viewport_changed {
            if !self.has_parked_error() {
                self.sync_native_window(window_id);
            }
            if !self.has_parked_error()
                && let Err(error) = self.dispatch_window_event("resize")
            {
                self.park_error(error);
            }
            if let Some(view) = self.inner.windows.get(&window_id) {
                view.window.request_redraw();
            }
        }
        if animation_pending && let Some(view) = self.inner.windows.get(&window_id) {
            view.window.request_redraw();
        }
        if (queued_pointer_input || queued_keyboard_input || queued_drag_input)
            && let Some(view) = self.inner.windows.get(&window_id)
        {
            view.window.request_redraw();
        }
    }

    fn device_event(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
        device_id: Option<winit::event::DeviceId>,
        event: DeviceEvent,
    ) {
        if let DeviceEvent::PointerMotion { delta: (x, y) } = &event {
            crate::dom_bridge::input::pointer_movement(*x, *y);
            if crate::dom_bridge::window::web_pointer_locked()
                && let Some((&window_id, view)) = self.inner.windows.iter().next()
            {
                self.pending_locked_pointer_movement
                    .push((window_id, (*x, *y)));
                view.window.request_redraw();
            }
        }
        self.inner.device_event(event_loop, device_id, event);
    }

    fn about_to_wait(&mut self, event_loop: &dyn ActiveEventLoop) {
        // First: a synthetic cycle is meant to be indistinguishable from one
        // the platform sent, so it runs before the turn's other work, exactly
        // where a real `destroy_surfaces` would have landed.
        self.run_synthetic_phase(event_loop);
        self.apply_menu_signals(event_loop);
        self.inner.about_to_wait(event_loop);
        self.settle_native_resize(event_loop);
        // The turn's last reported size, applied once. A redraw in the same
        // turn has usually taken it already; this is the turn that had none.
        let windows: Vec<_> = self.pending_resize.keys().copied().collect();
        for window_id in windows {
            self.apply_pending_resize(event_loop, window_id);
        }
        self.maybe_dispatch_load();
        // The surface is asked before JavaScript is: a window that cannot
        // present has no frame to ask for, and the question below costs a
        // script evaluation on every turn of the loop.
        if !self.surface.is_lost() && self.animation_frames_pending() {
            for view in self.inner.windows.values() {
                view.window.request_redraw();
            }
        }
    }

    fn suspended(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.on_suspended(event_loop);
    }

    fn destroy_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.on_destroy_surfaces(event_loop);
    }

    fn memory_warning(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.on_memory_warning(event_loop);
    }

    #[cfg(target_os = "macos")]
    fn macos_handler(&mut self) -> Option<&mut dyn ApplicationHandlerExtMacOS> {
        Some(self)
    }
}

#[cfg(target_os = "macos")]
impl<Rend: anyrender::WindowRenderer, E: JsEngine + Clone> ApplicationHandlerExtMacOS
    for WindowApplication<Rend, E>
{
    fn standard_key_binding(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
        window_id: WindowId,
        action: &str,
    ) {
        self.inner
            .standard_key_binding(event_loop, window_id, action);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taking_queued_input_preserves_both_orders() {
        let parked_error = RefCell::<Option<&str>>::new(None);
        let mut queue = vec![
            (2, "other-first"),
            (1, "first"),
            (2, "other-second"),
            (1, "second"),
        ];

        assert_eq!(
            take_queued_for(&parked_error, &mut queue, &1),
            Some(vec!["first", "second"])
        );
        assert_eq!(queue, [(2, "other-first"), (2, "other-second")]);
    }

    #[test]
    fn the_first_parked_error_wins_and_leaves_queued_input_untouched() {
        let parked_error = RefCell::new(None);
        let first = JsError::with_stack("first callback failed", "first stack");
        park_first_error(&parked_error, first.clone());
        park_first_error(&parked_error, JsError::new("cascade failed too"));
        let mut queue = vec![(1, "first"), (2, "other"), (1, "second")];

        assert_eq!(take_queued_for(&parked_error, &mut queue, &1), None);
        assert_eq!(parked_error.borrow().as_ref(), Some(&first));
        assert_eq!(queue, [(1, "first"), (2, "other"), (1, "second")]);

        // Once `pump` surfaces that exact error, the preserved input is what
        // the next turn drains; neither the first nor the second key was lost.
        assert_eq!(parked_error.borrow_mut().take(), Some(first));
        assert_eq!(
            take_queued_for(&parked_error, &mut queue, &1),
            Some(vec!["first", "second"])
        );
        assert_eq!(queue, [(2, "other")]);
    }

    #[test]
    fn keyboard_input_calls_preserve_the_serialized_public_shape() {
        let init = KeyboardEventInit {
            bubbles: true,
            cancelable: true,
            key: "a".to_owned(),
            code: "KeyA".to_owned(),
            repeat: false,
            modifiers: ModifierFlags::from(ModifiersState::CONTROL | ModifiersState::ALT),
        };
        let script = input_call_script(InputBootstrap::Keyboard, &("key\"down", init)).unwrap();
        let arguments = script
            .strip_prefix("globalThis.__blitsenDispatchKeyboardEvent(...")
            .and_then(|script| script.strip_suffix(')'))
            .expect("the typed keyboard entry point wraps one argument array");

        assert_eq!(
            serde_json::from_str::<serde_json::Value>(arguments).unwrap(),
            serde_json::json!([
                "key\"down",
                {
                    "bubbles": true,
                    "cancelable": true,
                    "key": "a",
                    "code": "KeyA",
                    "repeat": false,
                    "ctrlKey": true,
                    "shiftKey": false,
                    "altKey": true,
                    "metaKey": false,
                }
            ])
        );
    }

    #[test]
    fn ime_input_calls_preserve_utf8_cursor_offsets_and_typed_shape() {
        let (kind, init) = ime_call(Ime::Preedit("中".into(), Some((0, 3))));
        let script = input_call_script(InputBootstrap::Ime, &(kind, init)).unwrap();
        let arguments = script
            .strip_prefix("globalThis.__blitsenDispatchImeEvent(...")
            .and_then(|script| script.strip_suffix(')'))
            .expect("the typed IME entry point wraps one argument array");

        assert_eq!(
            serde_json::from_str::<serde_json::Value>(arguments).unwrap(),
            serde_json::json!([
                "preedit",
                {
                    "data": "中",
                    "cursorStart": 0,
                    "cursorEnd": 3,
                    "beforeBytes": null,
                    "afterBytes": null,
                }
            ])
        );
    }

    #[test]
    fn native_ime_enable_request_carries_the_painted_caret_area() {
        let area = Rect {
            x: 12.5,
            y: 24.0,
            width: 1.5,
            height: 20.0,
        };
        let ImeRequest::Enable(enable) = ime_enable_request(area) else {
            panic!("an editable control enables IME");
        };
        assert!(enable.capabilities().cursor_area());
        let Some((position, size)) = enable.request_data().cursor_area else {
            panic!("the enable request carries a candidate-window area");
        };
        assert_eq!(
            position,
            winit::dpi::Position::Logical(LogicalPosition::new(12.5, 24.0))
        );
        assert_eq!(size, winit::dpi::Size::Logical(LogicalSize::new(1.5, 20.0)));
    }

    #[test]
    fn modifier_flags_keep_the_dom_initializer_shape() {
        let modifiers = ModifierFlags::from(ModifiersState::CONTROL | ModifiersState::ALT);
        assert_eq!(
            serde_json::to_value(modifiers).unwrap(),
            serde_json::json!({
                "ctrlKey": true,
                "shiftKey": false,
                "altKey": true,
                "metaKey": false,
            })
        );
    }

    #[test]
    fn key_names_and_codes_match_dom_conventions() {
        assert_eq!(dom_key_name(&Key::Character("a".into())), "a");
        assert_eq!(
            dom_key_name(&Key::Named(winit::keyboard::NamedKey::Tab)),
            "Tab"
        );
        assert_eq!(
            dom_key_code(PhysicalKey::Code(winit::keyboard::KeyCode::KeyA)),
            "KeyA"
        );
    }
}
