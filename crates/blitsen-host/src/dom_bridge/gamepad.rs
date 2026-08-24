//! Process-local bridge state for the standard Gamepad API.
//!
//! The platform backend is owned by the window session and, after JavaScript
//! first uses the API, polled exactly once per redraw. This module is only the
//! synchronous view JavaScript reads and the two ordered queues crossing the
//! already-borrowed session boundary.

use std::cell::RefCell;
use std::collections::HashMap;

use blitsen_js::{JsEngine, JsError};
use serde::Serialize;

use super::{argument, json_value};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RawGamepad {
    pub(crate) key: String,
    pub(crate) id: String,
    pub(crate) mapping: String,
    pub(crate) axes: Vec<f64>,
    pub(crate) buttons: Vec<f64>,
    pub(crate) vibration_actuator: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ConnectionChange {
    Connected(RawGamepad),
    Disconnected(String),
}

#[derive(Clone, Debug, Default)]
pub(crate) struct BackendFrame {
    pub(crate) changes: Vec<ConnectionChange>,
    /// Fresh state only for controllers which produced input this poll.
    pub(crate) connected: Vec<RawGamepad>,
    pub(crate) vibration_completed: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GamepadButton {
    pub(crate) pressed: bool,
    pub(crate) touched: bool,
    pub(crate) value: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GamepadSnapshot {
    pub(crate) id: String,
    pub(crate) index: usize,
    pub(crate) connected: bool,
    pub(crate) timestamp: f64,
    pub(crate) mapping: String,
    pub(crate) axes: Vec<f64>,
    pub(crate) buttons: Vec<GamepadButton>,
    pub(crate) vibration_actuator: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConnectionMessage {
    pub(crate) kind: &'static str,
    pub(crate) gamepad: GamepadSnapshot,
}

pub(crate) struct RegistryUpdate {
    pub(crate) messages: Vec<ConnectionMessage>,
    pub(crate) snapshots_changed: bool,
}

#[derive(Default)]
pub(crate) struct Registry {
    slots: Vec<Option<(String, GamepadSnapshot)>>,
    connected: HashMap<String, usize>,
    preferred: HashMap<String, usize>,
    #[cfg(test)]
    snapshot_builds: usize,
}

impl Registry {
    pub(crate) fn apply(&mut self, frame: BackendFrame, now_ms: f64) -> RegistryUpdate {
        let mut messages = Vec::new();
        let mut snapshots_changed = false;
        for change in frame.changes {
            match change {
                ConnectionChange::Connected(raw) => {
                    if let Some(&index) = self.connected.get(&raw.key) {
                        snapshots_changed |= self.update(index, raw, now_ms);
                        continue;
                    }
                    let index = self.allocate(&raw.key);
                    let key = raw.key.clone();
                    let snapshot = snapshot(raw, index, true, now_ms);
                    #[cfg(test)]
                    {
                        self.snapshot_builds += 1;
                    }
                    self.connected.insert(key.clone(), index);
                    self.preferred.insert(key.clone(), index);
                    self.slots[index] = Some((key, snapshot.clone()));
                    snapshots_changed = true;
                    messages.push(ConnectionMessage {
                        kind: "connected",
                        gamepad: snapshot,
                    });
                }
                ConnectionChange::Disconnected(key) => {
                    let Some(index) = self.connected.remove(&key) else {
                        continue;
                    };
                    let Some((_, mut snapshot)) = self.slots[index].take() else {
                        continue;
                    };
                    snapshot.connected = false;
                    snapshot.timestamp = now_ms;
                    snapshots_changed = true;
                    messages.push(ConnectionMessage {
                        kind: "disconnected",
                        gamepad: snapshot,
                    });
                }
            }
        }
        for raw in frame.connected {
            if let Some(&index) = self.connected.get(&raw.key) {
                snapshots_changed |= self.update(index, raw, now_ms);
            }
        }
        RegistryUpdate {
            messages,
            snapshots_changed,
        }
    }

    fn allocate(&mut self, key: &str) -> usize {
        if let Some(&preferred) = self.preferred.get(key)
            && self.slots.get(preferred).is_some_and(Option::is_none)
        {
            return preferred;
        }
        if let Some(index) = self.slots.iter().position(Option::is_none) {
            return index;
        }
        self.slots.push(None);
        self.slots.len() - 1
    }

    fn update(&mut self, index: usize, raw: RawGamepad, now_ms: f64) -> bool {
        let Some((key, current)) = self.slots[index].as_mut() else {
            return false;
        };
        let RawGamepad {
            key: next_key,
            id,
            mapping,
            mut axes,
            buttons,
            vibration_actuator,
        } = raw;
        let mut changed = current.id != id
            || current.mapping != mapping
            || current.axes.len() != axes.len()
            || current.buttons.len() != buttons.len()
            || current.vibration_actuator != vibration_actuator;
        for value in &mut axes {
            *value = normalized(*value, -1.0, 1.0);
        }
        changed |= current.axes != axes;
        if current.buttons.len() != buttons.len() {
            current
                .buttons
                .resize(buttons.len(), GamepadButton::default());
        }
        for (current, value) in current.buttons.iter_mut().zip(buttons) {
            let value = normalized(value, 0.0, 1.0);
            let next = GamepadButton {
                pressed: value > 0.5,
                touched: value > 0.0,
                value,
            };
            changed |= *current != next;
            *current = next;
        }
        *key = next_key;
        current.id = id;
        current.mapping = mapping;
        current.axes = axes;
        current.vibration_actuator = vibration_actuator;
        if changed {
            current.timestamp = now_ms;
        }
        changed
    }

    #[cfg(test)]
    pub(crate) fn snapshots(&self) -> Vec<Option<GamepadSnapshot>> {
        self.slots
            .iter()
            .map(|slot| slot.as_ref().map(|(_, snapshot)| snapshot.clone()))
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn snapshot_builds(&self) -> usize {
        self.snapshot_builds
    }

    pub(crate) fn key_for_index(&self, index: usize) -> Option<(&str, bool)> {
        self.slots
            .get(index)
            .and_then(Option::as_ref)
            .map(|(key, snapshot)| (key.as_str(), snapshot.vibration_actuator))
    }
}

fn normalized(value: f64, min: f64, max: f64) -> f64 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        0.0
    }
}

fn snapshot(raw: RawGamepad, index: usize, connected: bool, timestamp: f64) -> GamepadSnapshot {
    GamepadSnapshot {
        id: raw.id,
        index,
        connected,
        timestamp,
        mapping: raw.mapping,
        axes: raw
            .axes
            .into_iter()
            .map(|value| normalized(value, -1.0, 1.0))
            .collect(),
        buttons: raw
            .buttons
            .into_iter()
            .map(|value| {
                let value = normalized(value, 0.0, 1.0);
                GamepadButton {
                    pressed: value > 0.5,
                    touched: value > 0.0,
                    value,
                }
            })
            .collect(),
        vibration_actuator: raw.vibration_actuator,
    }
}

struct BridgeState {
    slots: String,
    messages: Vec<ConnectionMessage>,
    next_command: u64,
    requests: Vec<VibrationRequest>,
    completions: Vec<VibrationCompletion>,
    touched: bool,
    touch_generation: u64,
    #[cfg(test)]
    publishes: usize,
}

impl Default for BridgeState {
    fn default() -> Self {
        Self {
            slots: "[]".to_owned(),
            messages: Vec::new(),
            next_command: 0,
            requests: Vec::new(),
            completions: Vec::new(),
            touched: false,
            touch_generation: 0,
            #[cfg(test)]
            publishes: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct VibrationRequest {
    pub(crate) command_id: u64,
    pub(crate) index: usize,
    pub(crate) strong: f64,
    pub(crate) weak: f64,
    pub(crate) duration_ms: u32,
    pub(crate) start_delay_ms: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct VibrationCompletion {
    command_id: u64,
    result: Option<String>,
    error: Option<String>,
    error_name: Option<String>,
}

thread_local! {
    static BRIDGE: RefCell<BridgeState> = RefCell::new(BridgeState::default());
}

pub(crate) fn reset() {
    BRIDGE.with_borrow_mut(|state| {
        let touch_generation = state.touch_generation;
        *state = BridgeState {
            touch_generation,
            ..BridgeState::default()
        };
    });
}

pub(crate) fn poll_generation() -> Option<u64> {
    BRIDGE.with_borrow(|state| state.touched.then_some(state.touch_generation))
}

fn touch() {
    BRIDGE.with_borrow_mut(|state| {
        if !state.touched {
            state.touched = true;
            state.touch_generation = state.touch_generation.saturating_add(1);
        }
    });
}

pub(crate) fn publish(registry: &Registry, messages: Vec<ConnectionMessage>) {
    struct Snapshots<'a>(&'a [Option<(String, GamepadSnapshot)>]);

    impl Serialize for Snapshots<'_> {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            use serde::ser::SerializeSeq;

            let mut slots = serializer.serialize_seq(Some(self.0.len()))?;
            for slot in self.0 {
                slots.serialize_element(&slot.as_ref().map(|(_, snapshot)| snapshot))?;
            }
            slots.end()
        }
    }

    let slots = serde_json::to_string(&Snapshots(&registry.slots))
        .expect("normalized gamepad snapshots serialize");
    BRIDGE.with_borrow_mut(|state| {
        state.slots = slots;
        state.messages.extend(messages);
        #[cfg(test)]
        {
            state.publishes += 1;
        }
    });
}

#[cfg(test)]
pub(crate) fn touch_for_test() {
    touch();
}

#[cfg(test)]
pub(crate) fn publish_count() -> usize {
    BRIDGE.with_borrow(|state| state.publishes)
}

pub(crate) fn take_requests() -> Vec<VibrationRequest> {
    BRIDGE.with_borrow_mut(|state| std::mem::take(&mut state.requests))
}

pub(crate) fn complete(command_id: u64, result: Result<&'static str, (&'static str, String)>) {
    let (value, error_name, error) = match result {
        Ok(value) => (Some(value.to_owned()), None, None),
        Err((name, message)) => (None, Some(name.to_owned()), Some(message)),
    };
    BRIDGE.with_borrow_mut(|state| {
        state.completions.push(VibrationCompletion {
            command_id,
            result: value,
            error,
            error_name,
        });
    });
}

pub(crate) fn pending() -> bool {
    BRIDGE.with_borrow(|state| !state.messages.is_empty() || !state.completions.is_empty())
}

#[cfg(test)]
pub(crate) fn take_completion_results() -> Vec<(u64, Option<String>, Option<String>)> {
    BRIDGE.with_borrow_mut(|state| {
        std::mem::take(&mut state.completions)
            .into_iter()
            .map(|completion| {
                (
                    completion.command_id,
                    completion.result,
                    completion.error_name,
                )
            })
            .collect()
    })
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
pub(super) fn install<E: JsEngine + 'static>(engine: &mut E) -> Result<(), JsError> {
    engine.define_global_function(
        "__blitsenGamepadTouch",
        Box::new(move |call| {
            touch();
            Ok(call.this)
        }),
    )?;
    engine.define_global_function(
        "__blitsenGamepads",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            BRIDGE.with_borrow(|state| engine.string(&state.slots))
        }),
    )?;
    engine.define_global_function(
        "__blitsenGamepadPending",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            Ok(engine.boolean(pending()))
        }),
    )?;
    engine.define_global_function(
        "__blitsenGamepadTake",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let messages = BRIDGE.with_borrow_mut(|state| {
                let connections = std::mem::take(&mut state.messages)
                    .into_iter()
                    .map(|message| {
                        serde_json::json!({
                            "type": "connection",
                            "kind": message.kind,
                            "gamepad": message.gamepad,
                        })
                    });
                let completions =
                    std::mem::take(&mut state.completions)
                        .into_iter()
                        .map(|completion| {
                            serde_json::json!({
                                "type": "completion",
                                "commandId": completion.command_id,
                                "result": completion.result,
                                "error": completion.error,
                                "errorName": completion.error_name,
                            })
                        });
                connections.chain(completions).collect::<Vec<_>>()
            });
            let value =
                serde_json::to_value(messages).map_err(|error| JsError::new(error.to_string()))?;
            json_value(&mut engine, &value)
        }),
    )?;
    engine.define_global_function(
        "__blitsenGamepadVibrate",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let parse = |value: String, name: &str| {
                value
                    .parse::<f64>()
                    .map_err(|_| JsError::new(format!("invalid {name}")))
            };
            let index = argument(&mut engine, &call, 0, "gamepad index")?
                .parse::<usize>()
                .map_err(|_| JsError::new("invalid gamepad index"))?;
            let strong = parse(
                argument(&mut engine, &call, 1, "strong magnitude")?,
                "strong magnitude",
            )?;
            let weak = parse(
                argument(&mut engine, &call, 2, "weak magnitude")?,
                "weak magnitude",
            )?;
            let duration = parse(
                argument(&mut engine, &call, 3, "vibration duration")?,
                "vibration duration",
            )?;
            let start_delay = parse(
                argument(&mut engine, &call, 4, "vibration start delay")?,
                "vibration start delay",
            )?;
            if !strong.is_finite()
                || !weak.is_finite()
                || !duration.is_finite()
                || !start_delay.is_finite()
                || !(0.0..=1.0).contains(&strong)
                || !(0.0..=1.0).contains(&weak)
                || !(0.0..=60_000.0).contains(&duration)
                || !(0.0..=60_000.0).contains(&start_delay)
            {
                return Err(JsError::new(
                    "gamepad magnitudes must be 0..1 and duration/start delay must be 0..60000ms",
                ));
            }
            let command_id = BRIDGE.with_borrow_mut(|state| {
                state.next_command = state.next_command.saturating_add(1);
                let command_id = state.next_command;
                state.requests.push(VibrationRequest {
                    command_id,
                    index,
                    strong,
                    weak,
                    duration_ms: duration.round() as u32,
                    start_delay_ms: start_delay.round() as u32,
                });
                command_id
            });
            engine.string(&command_id.to_string())
        }),
    )
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
pub(super) fn install<E: JsEngine + 'static>(_engine: &mut E) -> Result<(), JsError> {
    Ok(())
}

#[cfg(all(
    test,
    any(target_os = "linux", target_os = "windows", target_os = "macos")
))]
mod tests {
    use super::*;

    fn execute(source: &str) {
        let mut engine = blitsen_quickjs::QuickJs::new().expect("an engine");
        let _services = crate::runtime_services::RuntimeServices::install(&mut engine)
            .expect("runtime services");
        crate::harness::execute_animation_harness(
            engine,
            "<!doctype html><html><body></body></html>".to_owned(),
            source.to_owned(),
            1,
            200,
            100,
        )
        .expect("the gamepad activation probe runs");
    }

    #[test]
    fn javascript_touches_polling_only_on_first_api_or_listener_use() {
        reset();
        execute("document.body.textContent = 'no gamepads';");
        assert_eq!(poll_generation(), None);

        for source in [
            "navigator.getGamepads(); navigator.getGamepads();",
            "addEventListener('gamepadconnected', () => {});",
            "addEventListener('gamepaddisconnected', () => {});",
            "globalThis[Symbol.for('blitsen.native')].input.onDeviceChange(() => {});",
        ] {
            reset();
            execute(source);
            assert!(
                poll_generation().is_some(),
                "this use must activate backend polling: {source}"
            );
        }
        reset();
    }

    #[test]
    fn javascript_sees_normalized_snapshots_and_ordered_connection_events() {
        reset();
        let raw = RawGamepad {
            key: "controller-7".into(),
            id: "Synthetic Pad".into(),
            mapping: "standard".into(),
            axes: vec![0.25, -2.0],
            buttons: vec![0.75, 0.0],
            vibration_actuator: true,
        };
        let mut registry = Registry::default();
        let messages = registry.apply(
            BackendFrame {
                changes: vec![ConnectionChange::Connected(raw.clone())],
                connected: vec![raw],
                ..BackendFrame::default()
            },
            12.5,
        );
        publish(&registry, messages.messages);

        const SCRIPT: &str = r#"
          const seen = [];
          addEventListener("gamepadconnected", event => seen.push(
            `web:${event.gamepad.index}:${event.gamepad.id}`));
          const { input } = globalThis[Symbol.for("blitsen.native")];
          input.onDeviceChange(event => seen.push(`native:${event.type}:${event.index}`));
          requestAnimationFrame(() => {
            const pads = navigator.getGamepads();
            const pad = pads[0];
            if (!(pad instanceof Gamepad) || !(pad.buttons[0] instanceof GamepadButton))
              throw new Error("snapshots do not use the standard gamepad classes");
            if (!Object.isFrozen(pads) || !Object.isFrozen(pad.axes) || !Object.isFrozen(pad.buttons))
              throw new Error("gamepad snapshots are mutable");
            if (pad.mapping !== "standard" || pad.timestamp !== 12.5
              || pad.axes.join(",") !== "0.25,-1"
              || pad.buttons[0].value !== 0.75 || !pad.buttons[0].pressed
              || pad.vibrationActuator?.type !== "dual-rumble")
              throw new Error("gamepad snapshot values were not normalized");
            void pad.vibrationActuator.playEffect("dual-rumble", {
              startDelay: 500, duration: 100, strongMagnitude: 1,
            });
            document.documentElement.setAttribute("data-gamepads",
              `${seen.join("|")};slots=${pads.length};connected=${pad.connected}`);
          });
        "#;
        let mut engine = blitsen_quickjs::QuickJs::new().expect("an engine");
        let _services = crate::runtime_services::RuntimeServices::install(&mut engine)
            .expect("runtime services");
        let snapshots = crate::harness::execute_animation_harness(
            engine,
            "<!doctype html><html><body></body></html>".to_owned(),
            SCRIPT.to_owned(),
            1,
            200,
            100,
        )
        .expect("the gamepad harness runs");
        let value = serde_json::to_value(&snapshots[0]).expect("snapshot serializes");
        let recorded = value["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .find(|node| node["tag"] == "html")
            .and_then(|node| node["attributes"]["data-gamepads"].as_str())
            .expect("the script records its observations");
        assert_eq!(
            recorded,
            "web:0:Synthetic Pad|native:connected:0;slots=1;connected=true"
        );
        let requests = take_requests();
        assert_eq!(
            requests.len(),
            1,
            "the effect delay is sent to the backend, not a JavaScript timer"
        );
        assert_eq!(
            (
                requests[0].strong,
                requests[0].weak,
                requests[0].duration_ms,
                requests[0].start_delay_ms,
            ),
            (1.0, 0.0, 100, 500)
        );
        reset();
    }
}
