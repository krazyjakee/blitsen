//! Dispatching pointer input: hit testing, and the call into JavaScript.
//!
//! The other half of [`super`]: that module says what device an event came
//! from, this one says where in the document it landed and hands it over. Only
//! the pointer's identity crosses the boundary — every consequence of it, from
//! `buttons` to pointer capture to the `click` a lift produces, is settled in
//! `dom_bridge/bootstrap/events.js`.

use blitsen_js::{JsEngine, JsError};
use blitz::dom::NodeId;
use serde::Serialize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::window::WindowId;

use super::{PendingPointerInput, PointerAction, classify_pointer_event};
use crate::DomRuntime;
use crate::native_window::{
    InputBootstrap, ModifierFlags, WindowApplication, css_pointer_coordinates, take_queued_for,
};

/// A pointer event as JavaScript receives it.
///
/// One bag for both events dispatched from it: the `PointerEvent` reads all of
/// it and the compatibility `MouseEvent` reads the members it recognises. The
/// pointer members are absent on a wheel, which is a `MouseEvent` and has no
/// pointer behind it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PointerEventInit {
    pub(crate) bubbles: bool,
    pub(crate) cancelable: bool,
    pub(crate) client_x: f64,
    pub(crate) client_y: f64,
    pub(crate) offset_x: f32,
    pub(crate) offset_y: f32,
    pub(crate) screen_x: f64,
    pub(crate) screen_y: f64,
    pub(crate) button: i32,
    pub(crate) delta_x: f64,
    pub(crate) delta_y: f64,
    /// Connected root-to-target handles from the hit test that chose the target.
    ///
    /// This is an internal dispatch hint, not a public `PointerEvent` member.
    pub(crate) propagation_path: Vec<String>,
    #[serde(flatten)]
    pub(crate) modifiers: ModifierFlags,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) pointer_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) pointer_type: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) is_primary: Option<bool>,
    /// The measured force, absent where the device has no pressure sensor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) force: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tilt_x: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tilt_y: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) twist: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tangential_pressure: Option<f64>,
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

/// The bootstrap entry point that owns one input.
///
/// An input with a pointer behind it goes to the pointer dispatcher, which runs
/// the whole sequence from it — the `PointerEvent`, the compatibility mouse
/// event and the `click`. The wheel has no pointer behind it and is a
/// `MouseEvent` outright, so it goes to the mouse one, which is where its scroll
/// default action lives. Sending it to the pointer dispatcher instead dispatched
/// a `PointerEvent` named "wheel" that scrolled nothing.
fn entry_point(init: &PointerEventInit) -> InputBootstrap {
    if init.pointer_id.is_some() {
        InputBootstrap::Pointer
    } else {
        InputBootstrap::Mouse
    }
}

impl<Rend: anyrender::WindowRenderer, E: JsEngine + Clone> WindowApplication<Rend, E> {
    /// Holds a pointer event until the frame that will dispatch it.
    ///
    /// Reports whether anything was queued, which is what makes the window ask
    /// for the redraw that drains it.
    pub(crate) fn queue_pointer_input(&mut self, window_id: WindowId, event: &WindowEvent) -> bool {
        let last_position = self.pointer_positions.get(&window_id).copied();
        let (input, position) =
            match classify_pointer_event(event, &mut self.pointer_ids, last_position) {
                PointerAction::Queue(input, position) => (input, position),
                PointerAction::Modifiers(modifiers) => {
                    self.modifiers = modifiers;
                    return false;
                }
                PointerAction::MouseGone => {
                    self.pointer_positions.remove(&window_id);
                    self.cursor_resolved_from.remove(&window_id);
                    return false;
                }
                PointerAction::Ignore => return false,
            };
        if let Some(position) = position {
            self.pointer_positions.insert(window_id, position);
        }
        // One move per pointer per turn: a queue of stale positions for the same
        // contact only costs hit tests nothing will read. Moves by *other*
        // contacts are kept, which is what makes two fingers dragging at once
        // two independent streams rather than one that alternates.
        if let PendingPointerInput::Move { pointer, .. } = input {
            self.pending_pointer_input
                .retain(|(queued_window, queued)| {
                    *queued_window != window_id
                        || !matches!(
                            queued,
                            PendingPointerInput::Move { pointer: queued, .. }
                                if queued.pointer_id == pointer.pointer_id
                        )
                });
        }
        self.pending_pointer_input.push((window_id, input));
        true
    }

    fn dispatch_input(
        &self,
        event_type: &str,
        target: NodeId,
        init: &PointerEventInit,
    ) -> Result<bool, JsError> {
        let entry_point = entry_point(init);
        let target = DomRuntime::serialize_handle(target);
        self.call_input_bootstrap(entry_point, &(event_type, target, init))
    }

    /// Dispatches everything the turn queued, at the tree the frame settled on.
    pub(crate) fn drain_pointer_input(&mut self, window_id: WindowId) {
        let Some(inputs) = take_queued_for(
            self.error.as_ref(),
            &mut self.pending_pointer_input,
            &window_id,
        ) else {
            return;
        };
        if inputs.is_empty() {
            return;
        }
        let Some((scale, screen_origin_x, screen_origin_y)) = self.window_geometry(window_id)
        else {
            return;
        };

        for input in inputs {
            let pointer = input.pointer();
            let (physical_x, physical_y, event_type, button, wheel_delta) = match input {
                PendingPointerInput::Move {
                    physical_x,
                    physical_y,
                    ..
                } => (
                    physical_x,
                    physical_y,
                    "pointermove",
                    // The button that changed, and on a move none did. The
                    // compatibility `mousemove` reads 0 instead, which is where
                    // that difference between the two interfaces is applied.
                    -1,
                    None,
                ),
                PendingPointerInput::Button {
                    physical_x,
                    physical_y,
                    button,
                    state,
                    ..
                } => (
                    physical_x,
                    physical_y,
                    if state == ElementState::Pressed {
                        "pointerdown"
                    } else {
                        "pointerup"
                    },
                    i32::from(dom_mouse_button(button)),
                    None,
                ),
                PendingPointerInput::Cancel {
                    physical_x,
                    physical_y,
                    ..
                } => (physical_x, physical_y, "pointercancel", -1, None),
                PendingPointerInput::Wheel { delta_x, delta_y } => {
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
            let hit = match self.hit_test(client_x, client_y) {
                Ok(Some(hit)) => hit,
                Ok(None) => continue,
                Err(error) => {
                    self.park_error(JsError::new(error.to_string()));
                    return;
                }
            };
            let init = PointerEventInit {
                bubbles: true,
                cancelable: true,
                client_x,
                client_y,
                offset_x: hit.offset_x,
                offset_y: hit.offset_y,
                screen_x,
                screen_y,
                button,
                delta_x: wheel_delta.map_or(0.0, |delta| delta.0),
                delta_y: wheel_delta.map_or(0.0, |delta| delta.1),
                propagation_path: hit
                    .path
                    .iter()
                    .map(|node| DomRuntime::serialize_handle(*node))
                    .collect(),
                modifiers: self.modifier_flags(),
                pointer_id: pointer.map(|pointer| pointer.pointer_id),
                pointer_type: pointer.map(|pointer| pointer.pointer_type.as_str()),
                is_primary: pointer.map(|pointer| pointer.primary),
                force: pointer.and_then(|pointer| pointer.identity.force),
                tilt_x: pointer.map(|pointer| pointer.identity.tilt_x),
                tilt_y: pointer.map(|pointer| pointer.identity.tilt_y),
                twist: pointer.map(|pointer| pointer.identity.twist),
                tangential_pressure: pointer.map(|pointer| pointer.identity.tangential_pressure),
            };
            if let Err(error) = self.dispatch_input(event_type, hit.target, &init) {
                self.park_error(error);
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use winit::dpi::PhysicalPosition;
    use winit::event::{ButtonSource, FingerId, MouseScrollDelta, PointerKind};

    use super::super::PointerIds;
    use super::*;

    fn queued(event: &WindowEvent, ids: &mut PointerIds) -> PendingPointerInput {
        match classify_pointer_event(event, ids, None) {
            PointerAction::Queue(input, _) => input,
            _ => panic!("expected a queued input"),
        }
    }

    /// One `PointerEventInit` with the pointer members a contact would carry,
    /// and one with the none a wheel does.
    fn init_for(input: &PendingPointerInput) -> PointerEventInit {
        let pointer = input.pointer();
        PointerEventInit {
            bubbles: true,
            cancelable: true,
            client_x: 0.0,
            client_y: 0.0,
            offset_x: 0.0,
            offset_y: 0.0,
            screen_x: 0.0,
            screen_y: 0.0,
            button: 0,
            delta_x: 0.0,
            delta_y: 0.0,
            propagation_path: Vec::new(),
            modifiers: ModifierFlags::default(),
            pointer_id: pointer.map(|pointer| pointer.pointer_id),
            pointer_type: pointer.map(|pointer| pointer.pointer_type.as_str()),
            is_primary: pointer.map(|pointer| pointer.primary),
            force: pointer.and_then(|pointer| pointer.identity.force),
            tilt_x: pointer.map(|pointer| pointer.identity.tilt_x),
            tilt_y: pointer.map(|pointer| pointer.identity.tilt_y),
            twist: pointer.map(|pointer| pointer.identity.twist),
            tangential_pressure: pointer.map(|pointer| pointer.identity.tangential_pressure),
        }
    }

    #[test]
    fn a_contact_goes_to_the_pointer_dispatcher_and_the_wheel_to_the_mouse_one() {
        let mut ids = PointerIds::default();
        let touch = queued(
            &WindowEvent::PointerButton {
                device_id: None,
                state: ElementState::Pressed,
                position: PhysicalPosition::new(0.0, 0.0),
                primary: true,
                button: ButtonSource::Touch {
                    finger_id: FingerId::from_raw(0),
                    force: None,
                },
            },
            &mut ids,
        );
        let wheel = queued(
            &WindowEvent::MouseWheel {
                device_id: None,
                delta: MouseScrollDelta::LineDelta(0.0, 1.0),
                phase: winit::event::TouchPhase::Moved,
            },
            &mut ids,
        );
        assert!(touch.pointer().is_some());
        assert!(wheel.pointer().is_none());
        assert_eq!(entry_point(&init_for(&touch)), InputBootstrap::Pointer);
        // The wheel's default action is a scroll and it lives on the mouse path.
        // Routing it through the pointer dispatcher dispatched a `PointerEvent`
        // named "wheel" that nothing acted on.
        assert_eq!(entry_point(&init_for(&wheel)), InputBootstrap::Mouse);
        // The serialized bag says the same thing, which is what the DOM reads.
        let wheel = serde_json::to_string(&init_for(&wheel)).unwrap();
        assert!(!wheel.contains("pointerId"), "{wheel}");
        assert!(
            serde_json::to_string(&init_for(&touch))
                .unwrap()
                .contains("\"pointerType\":\"touch\"")
        );
        // A cancellation is still a contact, so it keeps the pointer path.
        let cancelled = queued(
            &WindowEvent::PointerLeft {
                device_id: None,
                position: None,
                primary: true,
                kind: PointerKind::Touch(FingerId::from_raw(0)),
            },
            &mut ids,
        );
        assert_eq!(entry_point(&init_for(&cancelled)), InputBootstrap::Pointer);
    }

    #[test]
    fn the_native_hit_path_is_serialized_as_an_internal_dispatch_hint() {
        let mut ids = PointerIds::default();
        let input = queued(
            &WindowEvent::PointerButton {
                device_id: None,
                state: ElementState::Pressed,
                position: PhysicalPosition::new(0.0, 0.0),
                primary: true,
                button: ButtonSource::Touch {
                    finger_id: FingerId::from_raw(0),
                    force: None,
                },
            },
            &mut ids,
        );
        let mut init = init_for(&input);
        init.propagation_path = vec!["document".into(), "body".into(), "target".into()];
        let serialized = serde_json::to_value(init).unwrap();
        assert_eq!(
            serialized["propagationPath"],
            serde_json::json!(["document", "body", "target"])
        );
    }

    #[test]
    fn mouse_coordinates_and_button_numbers_match_dom_conventions() {
        assert_eq!(dom_mouse_button(MouseButton::Left), 0);
        assert_eq!(dom_mouse_button(MouseButton::Middle), 1);
        assert_eq!(dom_mouse_button(MouseButton::Right), 2);
        assert_eq!(
            css_pointer_coordinates(300.0, 180.0, 2.0, 40.0, 30.0),
            (150.0, 90.0, 190.0, 120.0)
        );
    }
}
