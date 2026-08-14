//! The native window: winit application, input translation and frame pumping.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use blitsen_blitz::BlitzDom;
use blitsen_core::WindowState;
use blitsen_dom::{DomBackend, DomName};
use blitsen_js::{JsEngine, JsError};
use blitz::dom::{DocGuard, DocGuardMut, Document as BlitzDocument, NodeId};
use blitz::shell::{BlitzApplication, BlitzShellProxy, WindowConfig, create_default_event_loop};
use blitz::traits::net::NetProvider;
use serde::Serialize;
use winit::application::ApplicationHandler;
use winit::cursor::CursorIcon;
use winit::dpi::LogicalSize;
use winit::event::{
    ButtonSource, DeviceEvent, ElementState, MouseButton, MouseScrollDelta, PointerSource,
    StartCause, WindowEvent,
};
use winit::event_loop::pump_events::EventLoopExtPumpEvents;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, ModifiersState, PhysicalKey};
use winit::window::WindowAttributes;
use winit::window::WindowId;

#[cfg(target_os = "macos")]
use winit::application::macos::ApplicationHandlerExtMacOS;

use crate::app::AppFiles;
use crate::{DomRuntime, OpenDirectoryOptions};

/// The winit application behind one window: input translation and dispatch.
pub struct WindowApplication<Rend: anyrender::WindowRenderer, E: JsEngine + Clone> {
    pub(crate) inner: BlitzApplication<Rend>,
    pub(crate) engine: E,
    pub(crate) state: Rc<RefCell<WindowState>>,
    pub(crate) error: Rc<RefCell<Option<JsError>>>,
    pub(crate) started_at: Instant,
    pub(crate) document: Rc<RefCell<BlitzDom>>,
    pub(crate) pending_mouse_input: Vec<(WindowId, PendingMouseInput)>,
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
    pub(crate) mouse_down_targets: HashMap<u16, NodeId>,
    pub(crate) mouse_buttons: u16,
    pub(crate) modifiers: ModifiersState,
    pub(crate) load_dispatched: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum PendingMouseInput {
    Move {
        physical_x: f64,
        physical_y: f64,
    },
    Button {
        physical_x: f64,
        physical_y: f64,
        button: MouseButton,
        state: ElementState,
    },
    Wheel {
        delta_x: f64,
        delta_y: f64,
    },
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MouseEventInit {
    bubbles: bool,
    cancelable: bool,
    client_x: f64,
    client_y: f64,
    offset_x: f32,
    offset_y: f32,
    screen_x: f64,
    screen_y: f64,
    button: u16,
    buttons: u16,
    delta_x: f64,
    delta_y: f64,
    ctrl_key: bool,
    shift_key: bool,
    alt_key: bool,
    meta_key: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KeyboardEventInit {
    bubbles: bool,
    cancelable: bool,
    key: String,
    code: String,
    repeat: bool,
    ctrl_key: bool,
    shift_key: bool,
    alt_key: bool,
    meta_key: bool,
}

/// One open native window, its document, and the I/O runtime behind them.
///
/// Both hosts drive a session the same way: [`open`](Self::open) once, then
/// [`pump`](Self::pump) until it reports the window is gone. Phase 1 pumps from
/// a task on Bun's loop (TECH.md §3, S1 option 1); Phase 2 pumps from its own
/// outer loop. Nothing else about the session differs between them.
pub struct WindowSession<E: JsEngine + Clone> {
    /// I/O runtime entered around every winit turn.
    pub runtime: tokio::runtime::Runtime,
    /// The winit loop, advanced without blocking by [`pump`](Self::pump).
    pub event_loop: EventLoop,
    /// Window, document and input translation.
    pub application: WindowApplication<anyrender_vello::VelloWindowRenderer, E>,
    /// The first error raised inside a winit callback, surfaced by `pump`.
    pub error: Rc<RefCell<Option<JsError>>>,
    /// Where the application's files come from, retained for reload.
    pub files: AppFiles,
    /// What the session was opened with, retained for reload.
    pub options: OpenDirectoryOptions,
}

impl<E: JsEngine + Clone + 'static> WindowSession<E> {
    /// Parses the entrypoint, runs its scripts, and opens a native window.
    pub fn open(
        engine: &mut E,
        files: AppFiles,
        options: OpenDirectoryOptions,
    ) -> Result<Self, JsError> {
        let started_at = Instant::now();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            // Network and audio work are asynchronous rather than CPU-parallel.
            // Tokio's default is one worker per core: 24 idle workers on the
            // benchmark host bought no throughput and retained allocator arenas.
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|error| JsError::new(error.to_string()))?;
        let guard = runtime.enter();
        let event_loop = create_default_event_loop();
        let (proxy, receiver) = BlitzShellProxy::new(event_loop.create_proxy());
        let net_provider = files.net_provider().unwrap_or_else(|| {
            Arc::new(blitz::net::Provider::new(Some(Arc::new(proxy.clone()))))
                as Arc<dyn NetProvider>
        });
        let document = crate::app::load_document(
            engine,
            &files,
            net_provider,
            options.width,
            options.height,
            None,
            false,
        )?;
        let renderer = anyrender_vello::VelloWindowRenderer::new();
        let attributes = WindowAttributes::default()
            .with_title(options.title.clone())
            .with_surface_size(LogicalSize::new(options.width, options.height));
        let window = WindowConfig::with_attributes(
            Box::new(SharedBlitzDocument(Rc::clone(&document.document))),
            renderer,
            attributes,
        );
        let mut application = BlitzApplication::new(proxy, receiver);
        application.add_window(window);
        let error = Rc::new(RefCell::new(None));
        let application = WindowApplication {
            inner: application,
            engine: engine.clone(),
            state: document.window_state,
            error: Rc::clone(&error),
            started_at,
            document: document.document,
            pending_mouse_input: Vec::new(),
            pending_keyboard_input: Vec::new(),
            pending_resize: HashMap::new(),
            applied_resize: HashMap::new(),
            pointer_positions: HashMap::new(),
            cursor_resolved_from: HashMap::new(),
            applied_cursor: HashMap::new(),
            mouse_down_targets: HashMap::new(),
            mouse_buttons: 0,
            modifiers: ModifiersState::empty(),
            load_dispatched: false,
        };
        drop(guard);
        Ok(Self {
            runtime,
            event_loop,
            application,
            error,
            files,
            options,
        })
    }

    /// Re-parses the entrypoint and replaces the document in the open window.
    pub fn reload(&mut self, engine: &mut E) -> Result<(), JsError> {
        let _guard = self.runtime.enter();
        let window_id = self
            .application
            .inner
            .windows
            .keys()
            .copied()
            .next()
            .ok_or_else(|| JsError::new("native window is not ready"))?;
        let viewport = self.application.inner.windows[&window_id]
            .doc
            .inner()
            .viewport()
            .clone();
        let scale = f64::from(viewport.hidpi_scale);
        let logical = winit::dpi::PhysicalSize::new(viewport.window_size.0, viewport.window_size.1)
            .to_logical::<u32>(scale);
        let proxy = self.application.inner.proxy.clone();
        let net_provider = self.files.net_provider().unwrap_or_else(|| {
            Arc::new(blitz::net::Provider::new(Some(Arc::new(proxy)))) as Arc<dyn NetProvider>
        });
        let document = crate::app::load_document(
            engine,
            &self.files,
            net_provider,
            logical.width,
            logical.height,
            Some(viewport),
            false,
        )?;

        let view = self
            .application
            .inner
            .windows
            .get_mut(&window_id)
            .expect("window id was read from this map");
        view.replace_document(
            Box::new(SharedBlitzDocument(Rc::clone(&document.document))),
            false,
        );
        let application = &mut self.application;
        application.state = document.window_state;
        application.document = document.document;
        application.started_at = Instant::now();
        application.pending_mouse_input.clear();
        application.pending_keyboard_input.clear();
        application.pointer_positions.clear();
        application.cursor_resolved_from.clear();
        application.applied_cursor.clear();
        application.mouse_down_targets.clear();
        application.mouse_buttons = 0;
        application.load_dispatched = false;
        Ok(())
    }

    /// Reloads one linked stylesheet without rerunning JavaScript.
    ///
    /// Reports whether the file was actually linked by the document.
    pub fn reload_css(&mut self, file: &str) -> Result<bool, JsError> {
        let _guard = self.runtime.enter();
        let root = Path::new(&self.options.root);
        let changed = root
            .join(file)
            .canonicalize()
            .map_err(|error| JsError::new(format!("could not reload CSS file {file}: {error}")))?;
        if !changed.starts_with(root) {
            return Err(JsError::new(format!(
                "CSS reload escaped application directory: {file}"
            )));
        }
        let href_name = DomName::attribute("href");
        let rel_name = DomName::attribute("rel");
        let hrefs = {
            let document = self.application.document.borrow();
            document
                .query_selector_all(document.document(), "link[href]")
                .map_err(crate::dom_error)?
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
            self.application
                .document
                .borrow_mut()
                .document_mut()
                .reload_resource_by_href(href);
        }
        Ok(!hrefs.is_empty())
    }

    /// Advances winit once without blocking, reporting whether a window remains.
    ///
    /// The one place a windowed frame turns. An error raised inside a winit
    /// callback cannot be returned from there, so it is parked and surfaces
    /// here, at the first call after it happened.
    pub fn pump(&mut self) -> Result<bool, JsError> {
        self.pump_for(Some(Duration::ZERO))
    }

    /// Advances winit once, waiting at most `timeout` for an event.
    ///
    /// `None` waits until an event arrives. The standalone runtime uses this
    /// while nothing is animating, so a static window consumes no polling
    /// turns. Callers embedded in another event loop should use [`Self::pump`].
    pub fn pump_for(&mut self, timeout: Option<Duration>) -> Result<bool, JsError> {
        let _guard = self.runtime.enter();
        self.event_loop
            .pump_app_events(timeout, &mut self.application);
        if let Some(error) = self.error.borrow_mut().take() {
            return Err(error);
        }
        Ok(!self.application.inner.windows.is_empty()
            || !self.application.inner.pending_windows.is_empty())
    }

    /// Reports whether JavaScript has an animation-frame callback to run.
    pub fn animation_frames_pending(&self) -> bool {
        self.application.animation_frames_pending()
    }

    /// Schedules a redraw of every open window.
    pub fn request_redraw(&self) {
        for view in self.application.inner.windows.values() {
            view.window.request_redraw();
        }
    }
}

/// Forgets the window the `native:window` module addresses.
///
/// Called when a session ends, so a later call reports "no window" instead of
/// reaching a destroyed one.
pub fn release_window() {
    crate::dom_bridge::window::publish(None);
}

pub(crate) fn dom_mouse_button(button: MouseButton) -> u16 {
    match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        MouseButton::Back => 3,
        MouseButton::Forward => 4,
        other => other as u16,
    }
}

pub(crate) fn dom_mouse_button_mask(button: MouseButton) -> u16 {
    match button {
        MouseButton::Left => 1,
        MouseButton::Right => 2,
        MouseButton::Middle => 4,
        MouseButton::Back => 8,
        MouseButton::Forward => 16,
        other => 1_u16
            .checked_shl(u32::from(dom_mouse_button(other)))
            .unwrap_or(0),
    }
}

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
    fn queue_mouse_input(&mut self, window_id: WindowId, event: &WindowEvent) -> bool {
        let input = match event {
            WindowEvent::PointerMoved {
                position,
                source: PointerSource::Mouse,
                ..
            } => {
                self.pointer_positions
                    .insert(window_id, (position.x, position.y));
                self.pending_mouse_input.retain(|(queued_window, input)| {
                    *queued_window != window_id || !matches!(input, PendingMouseInput::Move { .. })
                });
                PendingMouseInput::Move {
                    physical_x: position.x,
                    physical_y: position.y,
                }
            }
            WindowEvent::PointerButton {
                position,
                button: ButtonSource::Mouse(button),
                state,
                ..
            } => {
                self.pointer_positions
                    .insert(window_id, (position.x, position.y));
                PendingMouseInput::Button {
                    physical_x: position.x,
                    physical_y: position.y,
                    button: *button,
                    state: *state,
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (delta_x, delta_y) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => {
                        (f64::from(*x) * 40.0, f64::from(*y) * 40.0)
                    }
                    MouseScrollDelta::PixelDelta(position) => (position.x, position.y),
                };
                PendingMouseInput::Wheel { delta_x, delta_y }
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
                return false;
            }
            // Outside the window the cursor belongs to whatever the pointer is
            // over now, and the position last reported is no longer where it is.
            WindowEvent::PointerLeft { .. } => {
                self.pointer_positions.remove(&window_id);
                self.cursor_resolved_from.remove(&window_id);
                return false;
            }
            _ => return false,
        };
        self.pending_mouse_input.push((window_id, input));
        true
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
        let event_type =
            serde_json::to_string(event_type).map_err(|error| JsError::new(error.to_string()))?;
        let init = serde_json::to_string(init).map_err(|error| JsError::new(error.to_string()))?;
        let mut engine = self.engine.clone();
        let result = engine.evaluate_script(
            &format!("globalThis.__blitsenDispatchKeyboardEvent({event_type}, {init})"),
            "blitsen:native-keyboard-event",
        )?;
        engine.to_boolean(&result)
    }

    fn drain_keyboard_input(&mut self, window_id: WindowId) {
        if self.error.borrow().is_some() {
            return;
        }
        let mut inputs = Vec::new();
        self.pending_keyboard_input
            .retain(|(queued_window, input)| {
                if *queued_window == window_id {
                    inputs.push(input.clone());
                    false
                } else {
                    true
                }
            });
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
                        ctrl_key: self.modifiers.control_key(),
                        shift_key: self.modifiers.shift_key(),
                        alt_key: self.modifiers.alt_key(),
                        meta_key: self.modifiers.meta_key(),
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
                *self.error.borrow_mut() = Some(error);
                return;
            }
        }
    }

    fn dispatch_mouse_event(
        &self,
        event_type: &str,
        target: NodeId,
        init: &MouseEventInit,
    ) -> Result<bool, JsError> {
        let event_type =
            serde_json::to_string(event_type).map_err(|error| JsError::new(error.to_string()))?;
        let target = serde_json::to_string(&DomRuntime::serialize_handle(target))
            .map_err(|error| JsError::new(error.to_string()))?;
        let init = serde_json::to_string(init).map_err(|error| JsError::new(error.to_string()))?;
        let mut engine = self.engine.clone();
        let result = engine.evaluate_script(
            &format!("globalThis.__blitsenDispatchMouseEvent({event_type}, {target}, {init})"),
            "blitsen:native-mouse-event",
        )?;
        engine.to_boolean(&result)
    }

    fn drain_mouse_input(&mut self, window_id: WindowId) {
        if self.error.borrow().is_some() {
            return;
        }
        let mut inputs = Vec::new();
        self.pending_mouse_input.retain(|(queued_window, input)| {
            if *queued_window == window_id {
                inputs.push(*input);
                false
            } else {
                true
            }
        });
        if inputs.is_empty() {
            return;
        }
        let Some((scale, screen_origin_x, screen_origin_y)) =
            self.inner.windows.get(&window_id).map(|view| {
                let scale = f64::from(view.doc.inner().viewport().hidpi_scale);
                let origin = view.window.outer_position().unwrap_or_default();
                (
                    scale,
                    f64::from(origin.x) / scale,
                    f64::from(origin.y) / scale,
                )
            })
        else {
            return;
        };

        for input in inputs {
            let (physical_x, physical_y, event_type, button, wheel_delta) = match input {
                PendingMouseInput::Move {
                    physical_x,
                    physical_y,
                } => (physical_x, physical_y, "mousemove", 0, None),
                PendingMouseInput::Button {
                    physical_x,
                    physical_y,
                    button,
                    state,
                } => {
                    let mask = dom_mouse_button_mask(button);
                    match state {
                        ElementState::Pressed => self.mouse_buttons |= mask,
                        ElementState::Released => self.mouse_buttons &= !mask,
                    }
                    (
                        physical_x,
                        physical_y,
                        if state == ElementState::Pressed {
                            "mousedown"
                        } else {
                            "mouseup"
                        },
                        dom_mouse_button(button),
                        None,
                    )
                }
                PendingMouseInput::Wheel { delta_x, delta_y } => {
                    let (physical_x, physical_y) = self
                        .pointer_positions
                        .get(&window_id)
                        .copied()
                        .unwrap_or_default();
                    (physical_x, physical_y, "wheel", 0, Some((delta_x, delta_y)))
                }
            };
            let (client_x, client_y, screen_x, screen_y) = css_pointer_coordinates(
                physical_x,
                physical_y,
                scale,
                screen_origin_x,
                screen_origin_y,
            );
            let hit = (|| {
                let snapshot = self.document.borrow_mut().flush_layout()?;
                self.document
                    .borrow()
                    .hit_test(client_x as f32, client_y as f32, snapshot)
            })();
            let hit = match hit {
                Ok(Some(hit)) => hit,
                Ok(None) => continue,
                Err(error) => {
                    *self.error.borrow_mut() = Some(JsError::new(error.to_string()));
                    return;
                }
            };
            let init = MouseEventInit {
                bubbles: true,
                cancelable: true,
                client_x,
                client_y,
                offset_x: hit.offset_x,
                offset_y: hit.offset_y,
                screen_x,
                screen_y,
                button,
                buttons: self.mouse_buttons,
                delta_x: wheel_delta.map_or(0.0, |delta| delta.0),
                delta_y: wheel_delta.map_or(0.0, |delta| delta.1),
                ctrl_key: self.modifiers.control_key(),
                shift_key: self.modifiers.shift_key(),
                alt_key: self.modifiers.alt_key(),
                meta_key: self.modifiers.meta_key(),
            };
            if let Err(error) = self.dispatch_mouse_event(event_type, hit.target, &init) {
                *self.error.borrow_mut() = Some(error);
                return;
            }
            if let PendingMouseInput::Button { button, state, .. } = input {
                let button_id = dom_mouse_button(button);
                if state == ElementState::Pressed {
                    self.mouse_down_targets.insert(button_id, hit.target);
                } else {
                    let down_target = self.mouse_down_targets.remove(&button_id);
                    if button == MouseButton::Left
                        && down_target == Some(hit.target)
                        && let Err(error) = self.dispatch_mouse_event("click", hit.target, &init)
                    {
                        *self.error.borrow_mut() = Some(error);
                        return;
                    }
                }
            }
        }
    }

    fn animation_frames_pending(&self) -> bool {
        if self.error.borrow().is_some() {
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
                *self.error.borrow_mut() = Some(error);
                false
            }
        }
    }

    fn run_animation_frame(&self) -> bool {
        if self.error.borrow().is_some() {
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
                *self.error.borrow_mut() = Some(error);
                false
            }
        }
    }

    fn sync_window(&self, width: u32, height: u32, device_pixel_ratio: f64) {
        if self.error.borrow().is_some() {
            return;
        }
        *self.state.borrow_mut() = WindowState::new(width, height, device_pixel_ratio);
        let result = (|| {
            let mut engine = self.engine.clone();
            let window = engine.evaluate_script("globalThis", "blitsen:window-resize-target")?;
            self.state.borrow().sync(&mut engine, &window)
        })();
        if let Err(error) = result {
            *self.error.borrow_mut() = Some(error);
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
        if self.error.borrow().is_some() {
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
                *self.error.borrow_mut() = Some(crate::dom_error(error));
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
                *self.error.borrow_mut() = Some(crate::dom_error(error));
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

    fn sync_native_window(&self, window_id: WindowId) {
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
    fn publish_window(&self) {
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
        if let Err(error) = self.dispatch_window_event("resize") {
            *self.error.borrow_mut() = Some(error);
        }
        if let Some(view) = self.inner.windows.get(&window_id) {
            view.window.request_redraw();
        }
    }

    fn maybe_dispatch_load(&mut self) {
        if self.load_dispatched || self.error.borrow().is_some() {
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
            Err(error) => *self.error.borrow_mut() = Some(error),
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
        self.inner.resumed(event_loop);
    }

    fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.inner.can_create_surfaces(event_loop);
        // Published before `load` is dispatched, so the first listener an
        // application registers already has a window to act on.
        self.publish_window();
        let windows: Vec<_> = self.inner.windows.keys().copied().collect();
        for id in windows {
            self.sync_native_window(id);
        }
        self.maybe_dispatch_load();
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
        let queued_mouse_input = self.queue_mouse_input(window_id, &event);
        let queued_keyboard_input = self.queue_keyboard_input(window_id, &event);
        let viewport_changed = matches!(&event, WindowEvent::ScaleFactorChanged { .. });
        let redraw = matches!(&event, WindowEvent::RedrawRequested);
        if redraw {
            // Before the frame rather than after it: a redraw that painted the
            // size before last would be a frame the drag visibly lagged by.
            self.apply_pending_resize(event_loop, window_id);
            self.drain_mouse_input(window_id);
            self.drain_keyboard_input(window_id);
        }
        let animation_pending = redraw && self.run_animation_frame();
        self.inner.window_event(event_loop, window_id, event);
        // After Blitz has had the frame, because painting it re-resolves Blitz's
        // own hover state and sets a cursor from it.
        if redraw {
            self.sync_cursor(window_id);
        }
        if viewport_changed {
            self.sync_native_window(window_id);
            if let Err(error) = self.dispatch_window_event("resize") {
                *self.error.borrow_mut() = Some(error);
            }
            if let Some(view) = self.inner.windows.get(&window_id) {
                view.window.request_redraw();
            }
        }
        if animation_pending && let Some(view) = self.inner.windows.get(&window_id) {
            view.window.request_redraw();
        }
        if (queued_mouse_input || queued_keyboard_input)
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
        self.inner.about_to_wait(event_loop);
        self.settle_native_resize(event_loop);
        // The turn's last reported size, applied once. A redraw in the same
        // turn has usually taken it already; this is the turn that had none.
        let windows: Vec<_> = self.pending_resize.keys().copied().collect();
        for window_id in windows {
            self.apply_pending_resize(event_loop, window_id);
        }
        self.maybe_dispatch_load();
        if self.animation_frames_pending() {
            for view in self.inner.windows.values() {
                view.window.request_redraw();
            }
        }
    }

    fn suspended(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.inner.suspended(event_loop);
    }

    fn destroy_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.inner.destroy_surfaces(event_loop);
    }

    fn memory_warning(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.inner.memory_warning(event_loop);
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
    fn mouse_coordinates_and_button_masks_match_dom_conventions() {
        assert_eq!(dom_mouse_button(MouseButton::Left), 0);
        assert_eq!(dom_mouse_button(MouseButton::Middle), 1);
        assert_eq!(dom_mouse_button(MouseButton::Right), 2);
        assert_eq!(dom_mouse_button_mask(MouseButton::Left), 1);
        assert_eq!(dom_mouse_button_mask(MouseButton::Right), 2);
        assert_eq!(dom_mouse_button_mask(MouseButton::Middle), 4);
        assert_eq!(
            css_pointer_coordinates(300.0, 180.0, 2.0, 40.0, 30.0),
            (150.0, 90.0, 190.0, 120.0)
        );
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
