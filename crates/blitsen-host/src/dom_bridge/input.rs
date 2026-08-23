//! Focus-scoped native input state for polling-oriented applications.
//!
//! DOM events remain the primary input API. This is the additive part they do
//! not provide: one atomic snapshot of held physical keys and buttons plus raw
//! mouse movement accumulated since the previous snapshot.
//!
//! The snapshot models one pointer, because that is what a frame loop asks for:
//! where the cursor is and what is held down. Every window event carries winit's
//! `primary` flag and only the primary pointer is recorded here, so a second
//! finger on a touchscreen does not move the position the first one set. The
//! rest of the contacts are not lost — the DOM pointer events carry all of them,
//! with `pointerId` and pressure, which is the API a multi-touch application
//! wants anyway.
//!
//! # What each host really supplies
//!
//! Desktop and Android run the same session and reach this module through the
//! same [`observe`], but the platforms do not send the same events, and the
//! differences are readings rather than gaps:
//!
//! - Android delivers a touch down as `PointerEntered` and `PointerButton` with
//!   no move in between, so the position has to be taken from those two as well
//!   as from `PointerMoved` — a tap that never slides would otherwise press a
//!   button at an unknown place.
//! - A finger that lifts is a `PointerLeft`, exactly as a cursor leaving the
//!   window is on the desktop, and in both cases there is no longer a position
//!   to report. That is why the position goes back to `None` there rather than
//!   lingering where the pointer last was.
//! - [`pointer_movement`] is raw device motion, which Android's backend does not
//!   produce at all: there is no mouse under a finger to have moved. It stays 0
//!   there, and an application wanting the gesture reads the position instead.
//! - Wheel deltas are likewise a desktop signal. Android sends none unless a
//!   real mouse is attached, and a scroll gesture is touch movement rather than
//!   a wheel.
//! - Held keys are keyed by physical code, and a key the platform cannot name
//!   physically is not held at all. That is most of Android's soft keyboard,
//!   which reports characters without the key that produced them — those still
//!   arrive as DOM `keydown`, which is where a text field reads them from
//!   anyway. What this reports is the keys a game asks "is it down" about, and
//!   an on-screen keyboard has none.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use winit::event::{ButtonSource, ElementState, MouseButton, MouseScrollDelta, WindowEvent};

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct PointerState {
    x: Option<f64>,
    y: Option<f64>,
    buttons: BTreeSet<String>,
    movement_x: f64,
    movement_y: f64,
    wheel_line_x: f64,
    wheel_line_y: f64,
    wheel_pixel_x: f64,
    wheel_pixel_y: f64,
}

#[derive(Clone, Serialize)]
struct PressedKey {
    code: String,
    key: String,
}

#[derive(Default)]
struct State {
    sequence: u64,
    focused: bool,
    keys: BTreeMap<String, String>,
    pointer: PointerState,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    sequence: u64,
    focused: bool,
    keys: Vec<PressedKey>,
    pointer: PointerState,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn changed(state: &mut State) {
    state.sequence = state.sequence.saturating_add(1);
}

pub(crate) fn reset() {
    STATE.with_borrow_mut(|state| *state = State::default());
}

pub(crate) fn focus(focused: bool) {
    STATE.with_borrow_mut(|state| {
        state.focused = focused;
        if !focused {
            state.keys.clear();
            state.pointer.buttons.clear();
        }
        changed(state);
    });
}

pub(crate) fn key(code: String, key: String, pressed: bool) {
    if code.is_empty() {
        return;
    }
    STATE.with_borrow_mut(|state| {
        if pressed {
            state.keys.insert(code, key);
        } else {
            state.keys.remove(&code);
        }
        changed(state);
    });
}

/// Records what one window event says about the pointer.
///
/// Every event that carries a position sets one, not just the moves: Android
/// sends a touch down with no move in front of it, so a tap would otherwise
/// report a pressed button and no place it was pressed.
///
/// `scale` is the window's scale factor, because the snapshot answers in CSS
/// pixels — the same coordinate space a `pointerdown` listener on the same
/// window reads, so the two cannot disagree about where the pointer is.
pub(crate) fn observe(event: &WindowEvent, scale: f64) {
    match event {
        WindowEvent::PointerEntered {
            primary: true,
            position,
            ..
        }
        | WindowEvent::PointerMoved {
            primary: true,
            position,
            ..
        } => {
            let logical = position.to_logical::<f64>(scale);
            pointer_position(Some((logical.x, logical.y)));
        }
        WindowEvent::PointerButton {
            primary: true,
            state,
            position,
            button,
            ..
        } => {
            let logical = position.to_logical::<f64>(scale);
            pointer_position(Some((logical.x, logical.y)));
            pointer_button(button_name(button), *state == ElementState::Pressed);
        }
        // The pointer is gone rather than parked: a cursor that left the window
        // is somewhere this application cannot see, and a finger that lifted is
        // nowhere at all. Held buttons are deliberately left alone — a drag that
        // crosses the window edge is still a drag, and the platform keeps
        // delivering it.
        WindowEvent::PointerLeft { primary: true, .. } => pointer_position(None),
        WindowEvent::MouseWheel { delta, .. } => match delta {
            MouseScrollDelta::LineDelta(x, y) => wheel_lines(f64::from(*x), f64::from(*y)),
            MouseScrollDelta::PixelDelta(position) => wheel_pixels(position.x, position.y),
        },
        _ => {}
    }
}

/// The name [`PointerState::buttons`] carries a pressed button under.
///
/// winit's own `mouse_button` is what maps the non-mouse sources: a finger and a
/// pen tip both answer as the primary button, which is what the DOM says about
/// them too, so a snapshot reads the same on a touchscreen as under a mouse. A
/// source it cannot name at all is still reported — the button is held, and
/// saying so under a name that admits it is unknown beats dropping it.
fn button_name(button: &ButtonSource) -> String {
    match button.clone().mouse_button() {
        Some(MouseButton::Left) => "primary".to_owned(),
        Some(MouseButton::Right) => "secondary".to_owned(),
        Some(MouseButton::Middle) => "auxiliary".to_owned(),
        Some(MouseButton::Back) => "back".to_owned(),
        Some(MouseButton::Forward) => "forward".to_owned(),
        Some(other) => format!("other-{}", other as u8 + 1),
        None => "unknown".to_owned(),
    }
}

fn pointer_position(position: Option<(f64, f64)>) {
    STATE.with_borrow_mut(|state| {
        state.pointer.x = position.map(|(x, _)| x);
        state.pointer.y = position.map(|(_, y)| y);
        changed(state);
    });
}

fn pointer_button(button: String, pressed: bool) {
    STATE.with_borrow_mut(|state| {
        if pressed {
            state.pointer.buttons.insert(button);
        } else {
            state.pointer.buttons.remove(&button);
        }
        changed(state);
    });
}

/// Accumulates raw device motion, which is the one signal here that does not
/// come from a window event.
///
/// Only while focused, and that is the point of the gate rather than tidiness:
/// `DeviceEvent::PointerMotion` is the mouse itself, delivered whichever window
/// the pointer is over and whichever application owns it. An unfocused window
/// summing it would be reading the mouse of whatever the user switched to.
pub(crate) fn pointer_movement(x: f64, y: f64) {
    STATE.with_borrow_mut(|state| {
        if !state.focused {
            return;
        }
        state.pointer.movement_x += x;
        state.pointer.movement_y += y;
        changed(state);
    });
}

fn wheel_lines(x: f64, y: f64) {
    STATE.with_borrow_mut(|state| {
        state.pointer.wheel_line_x += x;
        state.pointer.wheel_line_y += y;
        changed(state);
    });
}

fn wheel_pixels(x: f64, y: f64) {
    STATE.with_borrow_mut(|state| {
        state.pointer.wheel_pixel_x += x;
        state.pointer.wheel_pixel_y += y;
        changed(state);
    });
}

/// Takes one reading. Relative movement and wheel values are deltas since the
/// previous reading, so taking the snapshot clears only those accumulators.
pub(crate) fn snapshot() -> serde_json::Value {
    STATE.with_borrow_mut(|state| {
        let snapshot = Snapshot {
            sequence: state.sequence,
            focused: state.focused,
            keys: state
                .keys
                .iter()
                .map(|(code, key)| PressedKey {
                    code: code.clone(),
                    key: key.clone(),
                })
                .collect(),
            pointer: state.pointer.clone(),
        };
        state.pointer.movement_x = 0.0;
        state.pointer.movement_y = 0.0;
        state.pointer.wheel_line_x = 0.0;
        state.pointer.wheel_line_y = 0.0;
        state.pointer.wheel_pixel_x = 0.0;
        state.pointer.wheel_pixel_y = 0.0;
        serde_json::to_value(snapshot).expect("native input snapshots serialize")
    })
}

#[cfg(test)]
mod tests {
    use winit::dpi::PhysicalPosition;
    use winit::event::{FingerId, Force, PointerKind, PointerSource, TouchPhase};

    use super::*;

    // The events a mouse produces, in the order a desktop backend sends them.
    // Positions are physical, because that is what winit reports and converting
    // them is this module's job.
    fn mouse_moved(x: f64, y: f64) -> WindowEvent {
        WindowEvent::PointerMoved {
            device_id: None,
            position: PhysicalPosition::new(x, y),
            primary: true,
            source: PointerSource::Mouse,
        }
    }

    fn mouse_button(button: MouseButton, state: ElementState, x: f64, y: f64) -> WindowEvent {
        WindowEvent::PointerButton {
            device_id: None,
            state,
            position: PhysicalPosition::new(x, y),
            primary: true,
            button: ButtonSource::Mouse(button),
        }
    }

    // What Android sends for one finger: a down with no move in front of it, an
    // optional slide, then a release and a leave. `primary` is winit's own flag
    // for the first finger down, and the second finger carries `false`.
    fn touch(finger: usize, primary: bool, x: f64, y: f64, phase: TouchPhase) -> WindowEvent {
        let finger_id = FingerId::from_raw(finger);
        let force = Some(Force::Normalized(1.0));
        let position = PhysicalPosition::new(x, y);
        match phase {
            TouchPhase::Started => WindowEvent::PointerEntered {
                device_id: None,
                position,
                primary,
                kind: PointerKind::Touch(finger_id),
            },
            TouchPhase::Moved => WindowEvent::PointerMoved {
                device_id: None,
                position,
                primary,
                source: PointerSource::Touch { finger_id, force },
            },
            _ => WindowEvent::PointerLeft {
                device_id: None,
                position: Some(position),
                primary,
                kind: PointerKind::Touch(finger_id),
            },
        }
    }

    fn touch_button(finger: usize, primary: bool, x: f64, y: f64, pressed: bool) -> WindowEvent {
        WindowEvent::PointerButton {
            device_id: None,
            state: if pressed {
                ElementState::Pressed
            } else {
                ElementState::Released
            },
            position: PhysicalPosition::new(x, y),
            primary,
            button: ButtonSource::Touch {
                finger_id: FingerId::from_raw(finger),
                force: Some(Force::Normalized(1.0)),
            },
        }
    }

    #[test]
    fn a_mouse_reports_where_it_is_in_css_pixels() {
        reset();
        observe(&mouse_moved(160.0, 80.0), 2.0);
        observe(
            &mouse_button(MouseButton::Right, ElementState::Pressed, 160.0, 80.0),
            2.0,
        );
        let value = snapshot();
        // Halved by the scale factor: the snapshot answers in the same units a
        // `pointerdown` listener on this window sees.
        assert_eq!(value["pointer"]["x"], 80.0);
        assert_eq!(value["pointer"]["y"], 40.0);
        assert_eq!(value["pointer"]["buttons"][0], "secondary");

        observe(
            &mouse_button(MouseButton::Right, ElementState::Released, 160.0, 80.0),
            2.0,
        );
        observe(
            &WindowEvent::MouseWheel {
                device_id: None,
                delta: MouseScrollDelta::LineDelta(0.0, -3.0),
                phase: TouchPhase::Moved,
            },
            2.0,
        );
        let value = snapshot();
        assert_eq!(value["pointer"]["buttons"].as_array().unwrap().len(), 0);
        // Wheel deltas are the platform's own units, unscaled: a line is a line
        // whatever the window's scale factor is.
        assert_eq!(value["pointer"]["wheelLineY"], -3.0);
    }

    // The cursor is somewhere this application cannot see, which is not the
    // place it was last seen.
    #[test]
    fn a_pointer_that_left_the_window_has_no_position() {
        reset();
        observe(&mouse_moved(10.0, 10.0), 1.0);
        observe(
            &mouse_button(MouseButton::Left, ElementState::Pressed, 10.0, 10.0),
            1.0,
        );
        observe(
            &WindowEvent::PointerLeft {
                device_id: None,
                position: Some(PhysicalPosition::new(10.0, 0.0)),
                primary: true,
                kind: PointerKind::Mouse,
            },
            1.0,
        );
        let value = snapshot();
        assert!(value["pointer"]["x"].is_null(), "{value}");
        assert!(value["pointer"]["y"].is_null(), "{value}");
        // The button survives: a drag that crosses the window edge is still a
        // drag, and the platform goes on delivering it.
        assert_eq!(value["pointer"]["buttons"][0], "primary");
    }

    // Android sends no move before a tap, so a snapshot taken between the down
    // and the release used to report a pressed button and no place it happened.
    #[test]
    fn an_android_tap_presses_somewhere() {
        reset();
        observe(&touch(0, true, 40.0, 60.0, TouchPhase::Started), 2.0);
        observe(&touch_button(0, true, 40.0, 60.0, true), 2.0);
        let pressed = snapshot();
        assert_eq!(pressed["pointer"]["x"], 20.0);
        assert_eq!(pressed["pointer"]["y"], 30.0);
        assert_eq!(pressed["pointer"]["buttons"][0], "primary");

        observe(&touch_button(0, true, 40.0, 60.0, false), 2.0);
        observe(&touch(0, true, 40.0, 60.0, TouchPhase::Ended), 2.0);
        let lifted = snapshot();
        assert_eq!(lifted["pointer"]["buttons"].as_array().unwrap().len(), 0);
        // The finger is gone, so there is no pointer to have a position.
        assert!(lifted["pointer"]["x"].is_null(), "{lifted}");
    }

    #[test]
    fn a_second_finger_does_not_move_the_pointer_the_first_one_set() {
        reset();
        observe(&touch(0, true, 10.0, 10.0, TouchPhase::Started), 1.0);
        observe(&touch_button(0, true, 10.0, 10.0, true), 1.0);
        observe(&touch(1, false, 300.0, 300.0, TouchPhase::Started), 1.0);
        observe(&touch_button(1, false, 300.0, 300.0, true), 1.0);
        observe(&touch(1, false, 320.0, 300.0, TouchPhase::Moved), 1.0);
        let value = snapshot();
        assert_eq!(value["pointer"]["x"], 10.0);
        assert_eq!(value["pointer"]["y"], 10.0);

        // And the second finger lifting does not take the first one's position
        // with it: the primary contact is still down.
        observe(&touch_button(1, false, 320.0, 300.0, false), 1.0);
        observe(&touch(1, false, 320.0, 300.0, TouchPhase::Ended), 1.0);
        let value = snapshot();
        assert_eq!(value["pointer"]["x"], 10.0);
        assert_eq!(value["pointer"]["buttons"][0], "primary");
    }

    // Raw device motion is the mouse itself rather than this window's view of
    // it, so it arrives whichever application the user is working in.
    #[test]
    fn raw_movement_is_only_summed_while_this_window_is_focused() {
        reset();
        pointer_movement(5.0, 5.0);
        assert_eq!(snapshot()["pointer"]["movementX"], 0.0);
        focus(true);
        pointer_movement(5.0, -1.0);
        let value = snapshot();
        assert_eq!(value["pointer"]["movementX"], 5.0);
        assert_eq!(value["pointer"]["movementY"], -1.0);
    }

    #[test]
    fn every_pointer_source_has_a_button_name() {
        for (source, expected) in [
            (ButtonSource::Mouse(MouseButton::Left), "primary"),
            (ButtonSource::Mouse(MouseButton::Middle), "auxiliary"),
            (ButtonSource::Mouse(MouseButton::Back), "back"),
            (ButtonSource::Mouse(MouseButton::Forward), "forward"),
            (ButtonSource::Mouse(MouseButton::Button6), "other-6"),
            (
                ButtonSource::Touch {
                    finger_id: FingerId::from_raw(0),
                    force: None,
                },
                "primary",
            ),
            (ButtonSource::Unknown(3), "unknown"),
        ] {
            assert_eq!(button_name(&source), expected);
        }
    }

    // Every key the platform cannot name physically would otherwise share one
    // entry under the empty string, and releasing either would release both.
    #[test]
    fn a_key_with_no_physical_code_is_not_held() {
        reset();
        focus(true);
        key(String::new(), "a".into(), true);
        assert_eq!(snapshot()["keys"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn snapshot_keeps_held_state_and_consumes_relative_deltas() {
        reset();
        focus(true);
        key("KeyA".into(), "a".into(), true);
        pointer_movement(3.0, -2.0);
        let first = snapshot();
        let second = snapshot();
        assert_eq!(first["keys"][0]["code"], "KeyA");
        assert_eq!(second["keys"][0]["code"], "KeyA");
        assert_eq!(first["pointer"]["movementX"], 3.0);
        assert_eq!(second["pointer"]["movementX"], 0.0);
    }

    #[test]
    fn losing_focus_releases_every_held_control() {
        reset();
        key("KeyA".into(), "a".into(), true);
        pointer_button("primary".into(), true);
        focus(false);
        let value = snapshot();
        assert_eq!(value["keys"].as_array().unwrap().len(), 0);
        assert_eq!(value["pointer"]["buttons"].as_array().unwrap().len(), 0);
    }
}
