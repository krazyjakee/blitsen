//! The native window: winit application, input translation and frame pumping.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use blitsen_blitz::BlitzDom;
use blitsen_core::WindowState;
use blitsen_dom::DomBackend;
use blitsen_js::{JsEngine, JsError};
use blitz::dom::{DocGuard, DocGuardMut, Document as BlitzDocument};
use blitz::shell::BlitzApplication;
use winit::application::ApplicationHandler;
use winit::cursor::CursorIcon;
use winit::event::{ButtonSource, DeviceEvent, ElementState, MouseButton, StartCause, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::ModifiersState;
use winit::window::{ResizeDirection, Window, WindowId};

#[cfg(target_os = "macos")]
use winit::application::macos::ApplicationHandlerExtMacOS;

use crate::drag_drop::PendingDrag;
use crate::pointer_input::{PendingPointerInput, PointerIds};
use crate::surface_lifecycle::{SurfaceState, SyntheticPhase};

pub(crate) mod gamepad;
pub(crate) mod hid;
mod input;
pub(crate) mod menu;
pub(crate) mod notify;
mod session;
pub(crate) mod tray;

use input::{ImeTarget, PendingKeyboardInput};
pub(crate) use input::{InputBootstrap, ModifierFlags, css_pointer_coordinates, take_queued_for};
pub use session::{Session, WindowSession};

/// Width of the resize border supplied for an undecorated window, in logical
/// pixels. Native decorations normally own this hit area; without them the
/// application surface reaches the window edge and the runtime must provide it.
const BORDERLESS_RESIZE_INSET: f64 = 6.0;

fn resize_direction_at(
    physical_x: f64,
    physical_y: f64,
    width: u32,
    height: u32,
    scale: f64,
) -> Option<ResizeDirection> {
    let inset = (BORDERLESS_RESIZE_INSET * scale).max(1.0);
    let horizontal = if physical_x < inset {
        Some(ResizeDirection::West)
    } else if physical_x >= f64::from(width) - inset {
        Some(ResizeDirection::East)
    } else {
        None
    };
    let vertical = if physical_y < inset {
        Some(ResizeDirection::North)
    } else if physical_y >= f64::from(height) - inset {
        Some(ResizeDirection::South)
    } else {
        None
    };
    match (horizontal, vertical) {
        (Some(ResizeDirection::West), Some(ResizeDirection::North)) => {
            Some(ResizeDirection::NorthWest)
        }
        (Some(ResizeDirection::East), Some(ResizeDirection::North)) => {
            Some(ResizeDirection::NorthEast)
        }
        (Some(ResizeDirection::West), Some(ResizeDirection::South)) => {
            Some(ResizeDirection::SouthWest)
        }
        (Some(ResizeDirection::East), Some(ResizeDirection::South)) => {
            Some(ResizeDirection::SouthEast)
        }
        (Some(direction), None) | (None, Some(direction)) => Some(direction),
        _ => None,
    }
}

fn borderless_resize_direction(
    window: &dyn Window,
    physical_x: f64,
    physical_y: f64,
) -> Option<ResizeDirection> {
    if window.is_decorated()
        || !window.is_resizable()
        || window.is_maximized()
        || window.fullscreen().is_some()
    {
        return None;
    }
    let size = window.surface_size();
    resize_direction_at(
        physical_x,
        physical_y,
        size.width,
        size.height,
        window.scale_factor(),
    )
}

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
    target_os = "linux",
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
    target_os = "linux",
    all(target_os = "macos", target_arch = "x86_64")
)))]
fn native_window_renderer() -> NativeWindowRenderer {
    eprintln!("blitsen: renderer=vello-gpu backend=wgpu");
    NativeWindowRenderer::new()
}

/// The window renderer a Linux run can actually use, decided once at startup.
///
/// Linux is the one desktop target where this is not a property of the build.
/// A headless runner under Xvfb and a workstation whose Vulkan driver is
/// missing both offer a window system with no GPU behind it, and
/// `anyrender_vello` does not degrade there: it unwraps the surface it could
/// not create and takes the process with it before the first frame. That is
/// issue #366's failure, and it is what `tests/surface_lifecycle.rs` reproduces
/// on a runner.
#[cfg(target_os = "linux")]
pub(crate) enum SelectedRenderer {
    /// wgpu found a device that is not the CPU pretending to be one.
    Gpu(Box<anyrender_vello::VelloWindowRenderer>),
    /// The software rasterizer over a `softbuffer` framebuffer.
    Cpu(Box<anyrender_vello_cpu::VelloCpuWindowRenderer>),
}

/// Whether wgpu can see a GPU worth handing Vello.
///
/// The question is put to wgpu rather than to the environment, and it is put
/// before winit creates a surface, which is the ordering [`WindowSession::open`]
/// already depended on. An adapter reporting [`wgpu::DeviceType::Cpu`] is not a
/// yes: lavapipe does enumerate, and Vello's device request against it then
/// fails to return inside any time a frame loop can wait for — measured at over
/// twenty-five minutes for the one window `surface_lifecycle` opens — so a
/// software adapter counts here as no adapter at all.
#[cfg(target_os = "linux")]
fn gpu_adapter_is_available() -> bool {
    use anyrender_vello::wgpu;

    let instance = wgpu::Instance::default();
    pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()))
        .iter()
        .any(|adapter| adapter.get_info().device_type != wgpu::DeviceType::Cpu)
}

/// The renderer selection reported on stderr, as every other target reports it.
#[cfg(target_os = "linux")]
fn native_window_renderer() -> SelectedRenderer {
    if gpu_adapter_is_available() {
        eprintln!("blitsen: renderer=vello-gpu backend=wgpu");
        SelectedRenderer::Gpu(Box::new(anyrender_vello::VelloWindowRenderer::new()))
    } else {
        eprintln!(
            "blitsen: renderer=vello-cpu window-backend=softbuffer \
             reason=no-gpu-adapter"
        );
        SelectedRenderer::Cpu(Box::new(anyrender_vello_cpu::VelloCpuWindowRenderer::new()))
    }
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

/// Forgets the window the `native:window` module addresses.
///
/// Called when a session ends, so a later call reports "no window" instead of
/// reaching a destroyed one.
pub fn release_window() {
    crate::dom_bridge::window::publish(None);
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

    fn animation_frames_pending(&self) -> bool {
        if self.has_parked_error() {
            return false;
        }
        let result = (|| {
            let mut engine = self.engine.clone();
            let hook = engine.retained_value(&self.host_hooks.animation_frames_pending)?;
            let pending = engine.call(&hook, None, &[])?;
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

    fn run_animation_frame(&self) {
        if self.has_parked_error() {
            return;
        }
        let timestamp = self.started_at.elapsed().as_secs_f64() * 1_000.0;
        let result = (|| {
            let mut engine = self.engine.clone();
            let timestamp = engine.number(timestamp);
            let hook = engine.retained_value(&self.host_hooks.animation_frame_tick)?;
            engine.call(&hook, None, &[timestamp])?;
            engine.drain_microtasks()?;
            Ok(())
        })();
        if let Err(error) = result {
            self.park_error(error);
        }
    }

    fn sync_window(&self, width: u32, height: u32, device_pixel_ratio: f64) {
        if self.has_parked_error() {
            return;
        }
        *self.state.borrow_mut() = WindowState::new(width, height, device_pixel_ratio);
        let result = (|| {
            let mut engine = self.engine.clone();
            let window = engine.retained_value(&self.host_hooks.window)?;
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
        let Some((scale, resize_direction)) = self.inner.windows.get(&window_id).map(|view| {
            (
                f64::from(view.doc.inner().viewport().hidpi_scale),
                borderless_resize_direction(view.window.as_ref(), physical_x, physical_y),
            )
        }) else {
            return;
        };
        // The runtime owns the otherwise-missing frame of an undecorated
        // window. Its resize cursor therefore takes precedence over CSS in the
        // narrow edge hit area, just as an operating-system decoration does.
        if let Some(direction) = resize_direction {
            self.cursor_resolved_from.remove(&window_id);
            let cursor_icon = CursorIcon::from(direction);
            let icon = Some(cursor_icon);
            if self.applied_cursor.get(&window_id) != Some(&icon) {
                self.applied_cursor.insert(window_id, icon);
                if let Some(view) = self.inner.windows.get(&window_id) {
                    view.window.set_cursor(cursor_icon.into());
                    view.window.set_cursor_visible(true);
                }
            }
            return;
        }
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

    /// Starts the platform resize loop for a press in the implicit frame of an
    /// undecorated window. The press belongs to that frame, not to the DOM.
    fn start_borderless_resize(&self, window_id: WindowId, event: &WindowEvent) -> bool {
        if crate::dom_bridge::window::web_pointer_locked() {
            return false;
        }
        let WindowEvent::PointerButton {
            position,
            state: ElementState::Pressed,
            button: ButtonSource::Mouse(MouseButton::Left),
            ..
        } = event
        else {
            return false;
        };
        let Some(view) = self.inner.windows.get(&window_id) else {
            return false;
        };
        let Some(direction) =
            borderless_resize_direction(view.window.as_ref(), position.x, position.y)
        else {
            return false;
        };
        match view.window.drag_resize_window(direction) {
            Ok(()) => true,
            Err(error) => {
                eprintln!("blitsen: could not start borderless window resize: {error}");
                false
            }
        }
    }

    pub(crate) fn sync_native_window(&mut self, window_id: WindowId) {
        if let Some(scale) = self.system_scale_override
            && let Some(view) = self.inner.windows.get_mut(&window_id)
            && f64::from(view.doc.inner().viewport().hidpi_scale) < scale
        {
            view.doc
                .inner_mut()
                .viewport_mut()
                .set_hidpi_scale(scale as f32);
        }
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
        let mut engine = self.engine.clone();
        let event_type = engine.string(event_type)?;
        let hook = engine.retained_value(&self.host_hooks.lifecycle)?;
        let result = engine.call(&hook, None, &[event_type])?;
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
        if self.start_borderless_resize(window_id, &event) {
            return;
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
        // Blitz has its own editor-side keyboard and IME handlers, but they know
        // nothing about this runtime's DOM events. Letting the same event
        // continue there would mutate the shared editor before `keydown` or
        // `compositionupdate`, then mutate it a second time when the bridge
        // applies the event's default action.
        if matches!(
            &event,
            WindowEvent::KeyboardInput { .. } | WindowEvent::Ime(_)
        ) {
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
        if redraw && !self.surface.is_lost() && !self.has_parked_error() {
            self.gamepads
                .poll(self.started_at.elapsed().as_secs_f64() * 1_000.0);
        }
        // `requestAnimationFrame` means "before the next paint", and a window
        // with no surface has no next paint. Android's winit backend stops
        // dispatching redraws entirely while the app is stopped; the desktop
        // backends have no such gate, so the rule is applied here instead and
        // means the same thing on every target (see `surface_lifecycle`).
        if redraw && !self.surface.is_lost() && (self.startup_revealed || startup_paint) {
            self.run_animation_frame();
        }
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
        // present has no frame to ask for. This retained callback is the turn's
        // single pending-work query, after the frame and native work settle.
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
mod borderless_resize_tests {
    use super::{ResizeDirection, resize_direction_at};

    #[test]
    fn resolves_each_edge_and_corner() {
        let direction = |x, y| resize_direction_at(x, y, 200, 100, 1.0);

        assert_eq!(direction(100.0, 50.0), None);
        assert_eq!(direction(3.0, 50.0), Some(ResizeDirection::West));
        assert_eq!(direction(197.0, 50.0), Some(ResizeDirection::East));
        assert_eq!(direction(100.0, 3.0), Some(ResizeDirection::North));
        assert_eq!(direction(100.0, 97.0), Some(ResizeDirection::South));
        assert_eq!(direction(3.0, 3.0), Some(ResizeDirection::NorthWest));
        assert_eq!(direction(197.0, 3.0), Some(ResizeDirection::NorthEast));
        assert_eq!(direction(3.0, 97.0), Some(ResizeDirection::SouthWest));
        assert_eq!(direction(197.0, 97.0), Some(ResizeDirection::SouthEast));
    }

    #[test]
    fn resize_inset_is_scaled_to_physical_pixels() {
        assert_eq!(
            resize_direction_at(10.0, 100.0, 400, 200, 2.0),
            Some(ResizeDirection::West)
        );
        assert_eq!(resize_direction_at(12.0, 100.0, 400, 200, 2.0), None);
    }
}
