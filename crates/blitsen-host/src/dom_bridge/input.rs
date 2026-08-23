//! Focus-scoped native input state for polling-oriented applications.
//!
//! DOM events remain the primary input API. This is the additive part they do
//! not provide: one atomic snapshot of held physical keys and buttons plus raw
//! mouse movement accumulated since the previous snapshot.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

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

pub(crate) fn pointer_position(x: f64, y: f64) {
    STATE.with_borrow_mut(|state| {
        state.pointer.x = Some(x);
        state.pointer.y = Some(y);
        changed(state);
    });
}

pub(crate) fn pointer_button(button: String, pressed: bool) {
    STATE.with_borrow_mut(|state| {
        if pressed {
            state.pointer.buttons.insert(button);
        } else {
            state.pointer.buttons.remove(&button);
        }
        changed(state);
    });
}

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

pub(crate) fn wheel_lines(x: f64, y: f64) {
    STATE.with_borrow_mut(|state| {
        state.pointer.wheel_line_x += x;
        state.pointer.wheel_line_y += y;
        changed(state);
    });
}

pub(crate) fn wheel_pixels(x: f64, y: f64) {
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
    use super::*;

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
