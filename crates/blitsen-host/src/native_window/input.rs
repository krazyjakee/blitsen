//! Native keyboard, IME, and shared input dispatch plumbing.

use std::cell::RefCell;

use blitsen_dom::{DomBackend, Rect};
use blitsen_js::{JsEngine, JsError};
use blitz::dom::NodeId;
use serde::Serialize;
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event::{ElementState, Ime, WindowEvent};
use winit::keyboard::{Key, ModifiersState, PhysicalKey};
use winit::window::{ImeCapabilities, ImeEnableRequest, ImeRequest, ImeRequestData, WindowId};

use super::WindowApplication;

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
struct KeyboardEventInit {
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
struct ImeEventInit {
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

fn dom_key_name(key: &Key) -> String {
    match key {
        Key::Character(character) => character.to_string(),
        Key::Named(named) => format!("{named:?}"),
        Key::Dead(_) => "Dead".into(),
        Key::Unidentified(_) => "Unidentified".into(),
    }
}

fn dom_key_code(key: PhysicalKey) -> String {
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
    pub(super) fn parked_error(&self) -> Option<JsError> {
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

    pub(super) fn queue_keyboard_input(
        &mut self,
        window_id: WindowId,
        event: &WindowEvent,
    ) -> bool {
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
    pub(super) fn sync_ime(&mut self, window_id: WindowId) -> Result<(), JsError> {
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

    pub(super) fn drain_locked_pointer_movement(&mut self, window_id: WindowId) {
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
    fn keyboard_initializer_preserves_the_serialized_public_shape() {
        let init = KeyboardEventInit {
            bubbles: true,
            cancelable: true,
            key: "a".to_owned(),
            code: "KeyA".to_owned(),
            repeat: false,
            modifiers: ModifierFlags::from(ModifiersState::CONTROL | ModifiersState::ALT),
        };
        assert_eq!(
            serde_json::to_value(("key\"down", init)).unwrap(),
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
    fn ime_initializer_preserves_utf8_cursor_offsets_and_typed_shape() {
        let (kind, init) = ime_call(Ime::Preedit("中".into(), Some((0, 3))));
        assert_eq!(
            serde_json::to_value((kind, init)).unwrap(),
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
