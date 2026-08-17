//! The native window: winit application, input translation and frame pumping.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
use std::process::Command;

use blitsen_blitz::BlitzDom;
use blitsen_core::WindowState;
use blitsen_dom::DomBackend;
use blitsen_js::{JsEngine, JsError};
use blitz::dom::{DocGuard, DocGuardMut, Document as BlitzDocument};
use blitz::shell::BlitzApplication;
use serde::Serialize;
use winit::application::ApplicationHandler;
use winit::cursor::CursorIcon;
use winit::event::{DeviceEvent, ElementState, StartCause, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, ModifiersState, PhysicalKey};
use winit::window::WindowId;

#[cfg(target_os = "macos")]
use winit::application::macos::ApplicationHandlerExtMacOS;

use crate::pointer_input::{PendingPointerInput, PointerIds};
use crate::surface_lifecycle::{SurfaceState, SyntheticPhase};

mod session;

pub use session::WindowSession;

/// The window renderer used on supported Intel Macs.
///
/// Vello's Metal compute path has caused full-session GPU resets on Intel Macs
/// (#229). Supported machines use the CPU rasterizer and softbuffer. The
/// affected MacBookPro14,3 is rejected before this renderer or a window is
/// created because Core Animation presentation also triggered the reset.
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
pub type NativeWindowRenderer = anyrender_vello_cpu::VelloCpuWindowRenderer;

/// The window renderer safe for this target.
#[cfg(not(all(target_os = "macos", target_arch = "x86_64")))]
pub type NativeWindowRenderer = anyrender_vello::VelloWindowRenderer;

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
fn native_window_renderer() -> NativeWindowRenderer {
    eprintln!(
        "blitsen: renderer=vello-cpu window-backend=softbuffer \
         reason=Intel-macOS-Metal-safety-fallback"
    );
    NativeWindowRenderer::new()
}

/// Why this Intel Mac must not create a window surface.
///
/// `MacBookPro14,3` is the 2017 15-inch model on which both Vello/wgpu and the
/// 0.1.1 CPU renderer's Core Animation presentation triggered Radeon Metal
/// compute resets and killed WindowServer (#229). An unidentified Intel Mac is
/// also failed closed: model detection is the safety boundary, so silently
/// continuing when it fails would turn a diagnostic problem into data loss.
#[cfg(any(all(target_os = "macos", target_arch = "x86_64"), test))]
fn unsafe_intel_mac_presentation(model: Option<&str>) -> Option<String> {
    match model.map(str::trim).filter(|model| !model.is_empty()) {
        Some("MacBookPro14,3") => Some(
            "Blitsen windowing is disabled on MacBookPro14,3: both GPU rendering and \
             0.1.1's CPU/Core Animation presentation have triggered Radeon Pro 560 \
             Metal compute resets and terminated WindowServer (#229). `blitsen doctor` \
             and `blitsen build` remain available; opening a window is refused to \
             prevent another desktop-session loss."
                .to_string(),
        ),
        Some(_) => None,
        None => Some(
            "Blitsen could not identify this Intel Mac model, so windowing is disabled: \
             model detection guards a known Metal/Core Animation desktop-session-loss \
             failure (#229). `blitsen doctor` and `blitsen build` remain available."
                .to_string(),
        ),
    }
}

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
fn ensure_window_presentation_is_safe() -> Result<(), JsError> {
    let model = Command::new("/usr/sbin/sysctl")
        .args(["-n", "hw.model"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok());
    match unsafe_intel_mac_presentation(model.as_deref()) {
        Some(reason) => Err(JsError::new(reason)),
        None => Ok(()),
    }
}

#[cfg(not(all(target_os = "macos", target_arch = "x86_64")))]
fn ensure_window_presentation_is_safe() -> Result<(), JsError> {
    Ok(())
}

#[cfg(not(all(target_os = "macos", target_arch = "x86_64")))]
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
pub use blitz::shell::set_android_app;

/// The winit application behind one window: input translation and dispatch.
pub struct WindowApplication<Rend: anyrender::WindowRenderer, E: JsEngine + Clone> {
    pub(crate) inner: BlitzApplication<Rend>,
    pub(crate) engine: E,
    pub(crate) state: Rc<RefCell<WindowState>>,
    pub(crate) error: Rc<RefCell<Option<JsError>>>,
    pub(crate) started_at: Instant,
    pub(crate) document: Rc<RefCell<BlitzDom>>,
    pub(crate) pending_pointer_input: Vec<(WindowId, PendingPointerInput)>,
    pub(crate) pending_keyboard_input: Vec<(WindowId, PendingKeyboardInput)>,
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
    /// Whether the window has a surface to paint into; see `surface_lifecycle`.
    pub(crate) surface: SurfaceState,
    /// A synthetic surface cycle a test asked for, run at the next pump.
    pub(crate) synthetic_phase: Option<SyntheticPhase>,
}

#[derive(Clone)]
pub(crate) enum PendingKeyboardInput {
    Key {
        event_type: &'static str,
        key: String,
        code: String,
        repeat: bool,
    },
    WindowFocus(bool),
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
    Pointer,
    Mouse,
}

impl InputBootstrap {
    fn entry_point(self) -> &'static str {
        match self {
            Self::Keyboard => "__blitsenDispatchKeyboardEvent",
            Self::Pointer => "__blitsenDispatchPointerEvent",
            Self::Mouse => "__blitsenDispatchMouseEvent",
        }
    }

    fn script_name(self) -> &'static str {
        match self {
            Self::Keyboard => "blitsen:native-keyboard-event",
            Self::Pointer | Self::Mouse => "blitsen:native-pointer-input",
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

#[cfg(test)]
mod presentation_safety_tests {
    use super::unsafe_intel_mac_presentation;

    #[test]
    fn blocks_the_model_that_reset_the_radeon_gpu() {
        let reason = unsafe_intel_mac_presentation(Some("MacBookPro14,3\n"))
            .expect("the affected model must be blocked");
        assert!(reason.contains("Radeon Pro 560"));
        assert!(reason.contains("opening a window is refused"));
    }

    #[test]
    fn permits_other_identified_intel_macs_to_keep_the_cpu_renderer() {
        assert!(unsafe_intel_mac_presentation(Some("MacBookPro15,1")).is_none());
    }

    #[test]
    fn fails_closed_when_model_detection_fails() {
        let reason = unsafe_intel_mac_presentation(None)
            .expect("unknown Intel hardware must not bypass the safety check");
        assert!(reason.contains("could not identify this Intel Mac model"));
    }
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
        let script = input_call_script(bootstrap, arguments)?;
        let mut engine = self.engine.clone();
        let result = engine.evaluate_script(&script, bootstrap.script_name())?;
        engine.to_boolean(&result)
    }

    /// Snapshots the modifiers that every queued input in this turn observes.
    pub(crate) fn modifier_flags(&self) -> ModifierFlags {
        self.modifiers.into()
    }

    fn queue_keyboard_input(&mut self, window_id: WindowId, event: &WindowEvent) -> bool {
        let input = match event {
            WindowEvent::KeyboardInput { event, .. } => PendingKeyboardInput::Key {
                event_type: if event.state == ElementState::Pressed {
                    "keydown"
                } else {
                    "keyup"
                },
                key: dom_key_name(&event.logical_key),
                code: dom_key_code(event.physical_key),
                repeat: event.repeat,
            },
            WindowEvent::Focused(focused) => PendingKeyboardInput::WindowFocus(*focused),
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

    fn drain_keyboard_input(&mut self, window_id: WindowId) {
        let Some(inputs) = take_queued_for(
            self.error.as_ref(),
            &mut self.pending_keyboard_input,
            &window_id,
        ) else {
            return;
        };
        for input in inputs {
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
                        key,
                        code,
                        repeat,
                        modifiers: self.modifier_flags(),
                    },
                ),
                PendingKeyboardInput::WindowFocus(focused) => {
                    let mut engine = self.engine.clone();
                    engine
                        .evaluate_script(
                            &format!(
                                "globalThis.dispatchEvent(new Event({}))",
                                if focused { "\"focus\"" } else { "\"blur\"" }
                            ),
                            "blitsen:native-window-focus",
                        )
                        .and_then(|value| engine.to_boolean(&value))
                }
            };
            if let Err(error) = result {
                self.park_error(error);
                return;
            }
        }
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
    /// There is deliberately only one: multiple windows wait on the shared
    /// versus isolated JavaScript context decision, and `create` is declared
    /// absent until it is settled.
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
    }

    fn resumed(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.on_resumed(event_loop);
    }

    fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.on_can_create_surfaces(event_loop);
    }

    fn proxy_wake_up(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.inner.proxy_wake_up(event_loop);
        self.maybe_dispatch_load();
    }

    fn window_event(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
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
        let queued_pointer_input = self.queue_pointer_input(window_id, &event);
        let queued_keyboard_input = self.queue_keyboard_input(window_id, &event);
        let viewport_changed = matches!(&event, WindowEvent::ScaleFactorChanged { .. });
        let redraw = matches!(&event, WindowEvent::RedrawRequested);
        if redraw {
            // Before the frame rather than after it: a redraw that painted the
            // size before last would be a frame the drag visibly lagged by.
            self.apply_pending_resize(event_loop, window_id);
            self.drain_pointer_input(window_id);
            self.drain_keyboard_input(window_id);
        }
        // `requestAnimationFrame` means "before the next paint", and a window
        // with no surface has no next paint. Android's winit backend stops
        // dispatching redraws entirely while the app is stopped; the desktop
        // backends have no such gate, so the rule is applied here instead and
        // means the same thing on every target (see `surface_lifecycle`).
        let animation_pending = redraw && !self.surface.is_lost() && self.run_animation_frame();
        self.inner.window_event(event_loop, window_id, event);
        // After Blitz has had the frame, because painting it re-resolves Blitz's
        // own hover state and sets a cursor from it.
        if redraw {
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
        if (queued_pointer_input || queued_keyboard_input)
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
        self.inner.device_event(event_loop, device_id, event);
    }

    fn about_to_wait(&mut self, event_loop: &dyn ActiveEventLoop) {
        // First: a synthetic cycle is meant to be indistinguishable from one
        // the platform sent, so it runs before the turn's other work, exactly
        // where a real `destroy_surfaces` would have landed.
        self.run_synthetic_phase(event_loop);
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
