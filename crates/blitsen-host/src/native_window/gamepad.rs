//! Per-frame gamepad discovery and the desktop gilrs backend.

use crate::dom_bridge::gamepad::{
    self, BackendFrame, ConnectionChange, RawGamepad, Registry, VibrationRequest,
};

pub(crate) trait Backend {
    fn poll(&mut self) -> BackendFrame;
    fn vibrate(
        &mut self,
        key: &str,
        strong: f64,
        weak: f64,
        duration_ms: u32,
        start_delay_ms: u32,
    ) -> Result<(), String>;
}

pub(crate) struct Controller {
    backend: Box<dyn Backend>,
    registry: Registry,
    active_vibrations: std::collections::HashMap<String, u64>,
    activation: Option<u64>,
}

impl Controller {
    pub(crate) fn with_backend(backend: Box<dyn Backend>) -> Self {
        Self {
            backend,
            registry: Registry::default(),
            active_vibrations: std::collections::HashMap::new(),
            activation: None,
        }
    }

    pub(crate) fn platform() -> Self {
        Self::with_backend(platform_backend())
    }

    /// Polls once for this redraw after first API use and publishes changed views.
    pub(crate) fn poll(&mut self, now_ms: f64) {
        let Some(activation) = gamepad::poll_generation() else {
            return;
        };
        let newly_activated = self.activation != Some(activation);
        self.activation = Some(activation);
        let frame = self.backend.poll();
        for key in &frame.vibration_completed {
            if let Some(command_id) = self.active_vibrations.remove(key) {
                gamepad::complete(command_id, Ok("complete"));
            }
        }
        for change in &frame.changes {
            if let ConnectionChange::Disconnected(key) = change
                && let Some(command_id) = self.active_vibrations.remove(key)
            {
                gamepad::complete(command_id, Ok("preempted"));
            }
        }
        let update = self.registry.apply(frame, now_ms);
        if newly_activated || update.snapshots_changed {
            gamepad::publish(&self.registry, update.messages);
        }
    }

    pub(crate) fn apply_requests(&mut self) -> bool {
        let requests = gamepad::take_requests();
        let any = !requests.is_empty();
        for request in requests {
            self.apply_request(&request);
        }
        any
    }

    fn apply_request(&mut self, request: &VibrationRequest) {
        let Some((key, vibration)) = self.registry.key_for_index(request.index) else {
            gamepad::complete(
                request.command_id,
                Err((
                    "NotFoundError",
                    format!("gamepad slot {} is not connected", request.index),
                )),
            );
            return;
        };
        let key = key.to_owned();
        if !vibration {
            gamepad::complete(
                request.command_id,
                Err((
                    "NotSupportedError",
                    format!("gamepad slot {} has no vibration actuator", request.index),
                )),
            );
            return;
        }
        let result = self.backend.vibrate(
            &key,
            request.strong,
            request.weak,
            request.duration_ms,
            request.start_delay_ms,
        );
        if let Some(command_id) = self.active_vibrations.remove(&key) {
            gamepad::complete(command_id, Ok("preempted"));
        }
        if let Err(message) = result {
            gamepad::complete(request.command_id, Err(("OperationError", message)));
        } else if request.duration_ms == 0 || (request.strong == 0.0 && request.weak == 0.0) {
            gamepad::complete(request.command_id, Ok("complete"));
        } else {
            self.active_vibrations.insert(key, request.command_id);
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
fn platform_backend() -> Box<dyn Backend> {
    match GilrsBackend::new() {
        Ok(backend) => Box::new(backend),
        Err(error) => {
            eprintln!("blitsen: gamepad-backend=unavailable reason={error}");
            Box::new(DisabledBackend)
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn platform_backend() -> Box<dyn Backend> {
    Box::new(DisabledBackend)
}

struct DisabledBackend;

impl Backend for DisabledBackend {
    fn poll(&mut self) -> BackendFrame {
        BackendFrame::default()
    }

    fn vibrate(
        &mut self,
        _key: &str,
        _strong: f64,
        _weak: f64,
        _duration_ms: u32,
        _start_delay_ms: u32,
    ) -> Result<(), String> {
        Err("the gamepad backend is unavailable".to_owned())
    }
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
struct GilrsBackend {
    gilrs: gilrs::Gilrs,
    force_feedback_enabled: bool,
    known: std::collections::HashMap<String, gilrs::GamepadId>,
    effects: std::collections::HashMap<String, gilrs::ff::Effect>,
    enumerated: bool,
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
impl GilrsBackend {
    fn new() -> Result<Self, String> {
        let gilrs = gilrs::GilrsBuilder::new()
            .with_default_filters(true)
            // gilrs' separate force-feedback server wakes every 50ms even
            // without a device or effect. Its platform hot-plug worker is
            // still needed here, but the periodic server starts on first use.
            .with_force_feedback(false)
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            gilrs,
            force_feedback_enabled: false,
            known: std::collections::HashMap::new(),
            effects: std::collections::HashMap::new(),
            enumerated: false,
        })
    }

    fn key(id: gilrs::GamepadId) -> String {
        format!("g{}", usize::from(id))
    }

    fn snapshot(id: gilrs::GamepadId, gamepad: &gilrs::Gamepad<'_>) -> RawGamepad {
        use gilrs::{Axis, Button, MappingSource};

        let standard = gamepad.mapping_source() != MappingSource::None;
        let axes = if standard {
            [
                Axis::LeftStickX,
                Axis::LeftStickY,
                Axis::RightStickX,
                Axis::RightStickY,
            ]
            .map(|axis| f64::from(gamepad.value(axis)))
            .to_vec()
        } else {
            Vec::new()
        };
        let buttons = if standard {
            [
                Button::South,
                Button::East,
                Button::West,
                Button::North,
                Button::LeftTrigger,
                Button::RightTrigger,
                Button::LeftTrigger2,
                Button::RightTrigger2,
                Button::Select,
                Button::Start,
                Button::LeftThumb,
                Button::RightThumb,
                Button::DPadUp,
                Button::DPadDown,
                Button::DPadLeft,
                Button::DPadRight,
                Button::Mode,
            ]
            .map(|button| {
                gamepad.button_data(button).map_or(0.0, |data| {
                    let value = f64::from(data.value());
                    if data.is_pressed() {
                        value.max(1.0)
                    } else {
                        value
                    }
                })
            })
            .to_vec()
        } else {
            Vec::new()
        };
        RawGamepad {
            key: Self::key(id),
            id: gamepad.name().to_owned(),
            mapping: if standard { "standard" } else { "" }.to_owned(),
            axes,
            buttons,
            vibration_actuator: gamepad.is_ff_supported(),
        }
    }

    fn enable_force_feedback(&mut self, key: &str) -> Result<(), String> {
        if self.force_feedback_enabled {
            return Ok(());
        }
        let replacement = gilrs::GilrsBuilder::new()
            .with_default_filters(true)
            .with_force_feedback(true)
            .build()
            .map_err(|error| error.to_string())?;
        let id = *self
            .known
            .get(key)
            .ok_or_else(|| "the gamepad disconnected before vibration started".to_owned())?;
        let gamepad = replacement.gamepad(id);
        if !gamepad.is_connected() {
            return Err("the gamepad disconnected before vibration started".to_owned());
        }
        if !gamepad.is_ff_supported() {
            return Err("the gamepad driver no longer reports force feedback".to_owned());
        }
        self.gilrs = replacement;
        self.force_feedback_enabled = true;
        Ok(())
    }
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
impl Backend for GilrsBackend {
    fn poll(&mut self) -> BackendFrame {
        use gilrs::EventType;

        let mut changes = Vec::new();
        let mut changed = Vec::new();
        let mut vibration_completed = Vec::new();
        while let Some(event) = self.gilrs.next_event() {
            let key = Self::key(event.id);
            match event.event {
                EventType::Connected => {
                    if self.known.insert(key, event.id).is_none() {
                        changes.push(ConnectionChange::Connected(Self::snapshot(
                            event.id,
                            &self.gilrs.gamepad(event.id),
                        )));
                    } else if !changed.contains(&event.id) {
                        changed.push(event.id);
                    }
                }
                EventType::Disconnected => {
                    self.known.remove(&key);
                    if let Some(effect) = self.effects.remove(&key) {
                        let _ = effect.stop();
                    }
                    changes.push(ConnectionChange::Disconnected(key));
                }
                EventType::ForceFeedbackEffectCompleted => {
                    self.effects.remove(&key);
                    vibration_completed.push(key);
                }
                _ => {
                    if self.known.values().any(|id| *id == event.id) && !changed.contains(&event.id)
                    {
                        changed.push(event.id);
                    }
                }
            }
        }
        // Some backends enumerate already-connected devices without queuing an
        // initial event. Add those in backend-id order so startup is stable.
        if !self.enumerated {
            let mut connected = self.gilrs.gamepads().collect::<Vec<_>>();
            connected.sort_by_key(|(id, _)| usize::from(*id));
            for (id, gamepad) in connected {
                let key = Self::key(id);
                if let std::collections::hash_map::Entry::Vacant(entry) = self.known.entry(key) {
                    entry.insert(id);
                    changes.push(ConnectionChange::Connected(Self::snapshot(id, &gamepad)));
                }
            }
            self.enumerated = true;
        }
        let connected = changed
            .into_iter()
            .filter_map(|id| {
                let gamepad = self.gilrs.gamepad(id);
                gamepad.is_connected().then(|| Self::snapshot(id, &gamepad))
            })
            .collect();
        self.gilrs.inc();
        BackendFrame {
            changes,
            connected,
            vibration_completed,
        }
    }

    fn vibrate(
        &mut self,
        key: &str,
        strong: f64,
        weak: f64,
        duration_ms: u32,
        start_delay_ms: u32,
    ) -> Result<(), String> {
        use gilrs::ff::{BaseEffect, BaseEffectType, EffectBuilder, Repeat, Replay, Ticks};

        if let Some(effect) = self.effects.remove(key) {
            let _ = effect.stop();
        }
        if duration_ms == 0 || (strong == 0.0 && weak == 0.0) {
            return Ok(());
        }
        self.enable_force_feedback(key)?;
        let id = *self
            .known
            .get(key)
            .ok_or_else(|| "the gamepad disconnected before vibration started".to_owned())?;
        let ticks = Ticks::from_ms(duration_ms.max(1));
        let delay = Ticks::from_ms(start_delay_ms);
        let magnitude = |value: f64| (value.clamp(0.0, 1.0) * f64::from(u16::MAX)).round() as u16;
        let mut builder = EffectBuilder::new();
        builder
            .add_effect(BaseEffect {
                kind: BaseEffectType::Strong {
                    magnitude: magnitude(strong),
                },
                scheduling: Replay {
                    after: delay,
                    play_for: ticks,
                    ..Default::default()
                },
                ..Default::default()
            })
            .add_effect(BaseEffect {
                kind: BaseEffectType::Weak {
                    magnitude: magnitude(weak),
                },
                scheduling: Replay {
                    after: delay,
                    play_for: ticks,
                    ..Default::default()
                },
                ..Default::default()
            })
            .gamepads(&[id])
            .repeat(Repeat::For(delay + ticks));
        let effect = builder
            .finish(&mut self.gilrs)
            .map_err(|error| error.to_string())?;
        effect.play().map_err(|error| error.to_string())?;
        self.effects.insert(key.to_owned(), effect);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    use super::*;

    type Vibration = (String, f64, f64, u32, u32);

    struct FakeBackend {
        polls: Rc<RefCell<usize>>,
        frames: VecDeque<BackendFrame>,
        vibrations: Rc<RefCell<Vec<Vibration>>>,
    }

    impl Backend for FakeBackend {
        fn poll(&mut self) -> BackendFrame {
            *self.polls.borrow_mut() += 1;
            self.frames.pop_front().unwrap_or_default()
        }

        fn vibrate(
            &mut self,
            key: &str,
            strong: f64,
            weak: f64,
            duration_ms: u32,
            start_delay_ms: u32,
        ) -> Result<(), String> {
            self.vibrations.borrow_mut().push((
                key.to_owned(),
                strong,
                weak,
                duration_ms,
                start_delay_ms,
            ));
            Ok(())
        }
    }

    fn raw(key: &str, value: f64, vibration_actuator: bool) -> RawGamepad {
        RawGamepad {
            key: key.to_owned(),
            // Deliberately identical: backend identity, not this public label,
            // is what must keep two matching controllers apart.
            id: "Example Controller".to_owned(),
            mapping: "standard".to_owned(),
            axes: vec![value, f64::NAN, 2.0, -2.0],
            buttons: vec![value, 2.0, -1.0],
            vibration_actuator,
        }
    }

    #[test]
    fn polling_and_publication_are_gated_by_first_api_use() {
        gamepad::reset();
        let polls = Rc::new(RefCell::new(0));
        let mut controller = Controller::with_backend(Box::new(FakeBackend {
            polls: Rc::clone(&polls),
            frames: VecDeque::new(),
            vibrations: Rc::new(RefCell::new(Vec::new())),
        }));

        assert_eq!(*polls.borrow(), 0, "construction starts no polling loop");
        assert!(
            !controller.apply_requests(),
            "an idle turn has no command work"
        );
        assert_eq!(*polls.borrow(), 0, "non-frame work never polls the backend");
        controller.poll(1.0);
        assert_eq!(
            *polls.borrow(),
            0,
            "an untouched Gamepad API pays no redraw-time backend work"
        );
        assert_eq!(gamepad::publish_count(), 0);

        gamepad::touch_for_test();
        controller.poll(2.0);
        assert_eq!(*polls.borrow(), 1, "one frame is exactly one backend poll");
        assert_eq!(gamepad::publish_count(), 1, "activation publishes once");
        controller.poll(3.0);
        assert_eq!(*polls.borrow(), 2, "an active API keeps backend cadence");
        assert_eq!(
            gamepad::publish_count(),
            1,
            "an unchanged frame neither snapshots nor republishes"
        );
    }

    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    #[test]
    fn platform_backend_defers_the_periodic_force_feedback_server() {
        let mut backend = GilrsBackend::new().expect("the platform gamepad backend starts");
        assert!(
            !backend.force_feedback_enabled,
            "ordinary discovery must not start gilrs' 50ms force-feedback loop"
        );
        backend
            .vibrate("not-connected", 0.0, 0.0, 0, 0)
            .expect("an already-quiet reset needs no device");
        assert!(
            !backend.force_feedback_enabled,
            "a no-op reset must not pay the force-feedback worker cost"
        );
    }

    #[test]
    fn slots_identity_reconnection_normalization_and_fifo_are_deterministic() {
        let mut registry = Registry::default();
        let first = registry.apply(
            BackendFrame {
                changes: vec![
                    ConnectionChange::Connected(raw("a", 0.75, true)),
                    ConnectionChange::Connected(raw("b", 0.25, false)),
                ],
                connected: vec![raw("a", 0.75, true), raw("b", 0.25, false)],
                ..BackendFrame::default()
            },
            10.0,
        );
        assert_eq!(
            first
                .messages
                .iter()
                .map(|event| event.kind)
                .collect::<Vec<_>>(),
            ["connected", "connected"]
        );
        assert_eq!(
            first
                .messages
                .iter()
                .map(|event| event.gamepad.index)
                .collect::<Vec<_>>(),
            [0, 1]
        );
        let slots = registry.snapshots();
        assert_eq!(slots[0].as_ref().unwrap().axes, [0.75, 0.0, 1.0, -1.0]);
        assert_eq!(slots[0].as_ref().unwrap().buttons[0].value, 0.75);
        assert!(slots[0].as_ref().unwrap().buttons[0].pressed);
        assert!(!slots[1].as_ref().unwrap().buttons[0].pressed);

        let disconnected = registry.apply(
            BackendFrame {
                changes: vec![ConnectionChange::Disconnected("a".into())],
                connected: vec![raw("b", 0.25, false)],
                ..BackendFrame::default()
            },
            20.0,
        );
        assert_eq!(disconnected.messages[0].kind, "disconnected");
        assert_eq!(disconnected.messages[0].gamepad.index, 0);
        assert!(!disconnected.messages[0].gamepad.connected);
        assert!(registry.snapshots()[0].is_none());

        let reconnected = registry.apply(
            BackendFrame {
                changes: vec![ConnectionChange::Connected(raw("a", 0.5, true))],
                connected: vec![raw("a", 0.5, true), raw("b", 0.25, false)],
                ..BackendFrame::default()
            },
            30.0,
        );
        assert_eq!(
            reconnected.messages[0].gamepad.index, 0,
            "a free preferred slot is reused"
        );
        assert_eq!(
            registry.snapshots()[1].as_ref().unwrap().timestamp,
            10.0,
            "an unchanged controller keeps its last-change timestamp"
        );

        registry.apply(
            BackendFrame {
                changes: vec![ConnectionChange::Disconnected("a".into())],
                connected: vec![raw("b", 0.25, false)],
                ..BackendFrame::default()
            },
            40.0,
        );
        let replacement = registry.apply(
            BackendFrame {
                changes: vec![ConnectionChange::Connected(raw("c", 0.0, false))],
                connected: vec![raw("b", 0.25, false), raw("c", 0.0, false)],
                ..BackendFrame::default()
            },
            50.0,
        );
        assert_eq!(
            replacement.messages[0].gamepad.index, 0,
            "the lowest hole is reused"
        );
        let displaced = registry.apply(
            BackendFrame {
                changes: vec![ConnectionChange::Connected(raw("a", 0.5, true))],
                connected: vec![
                    raw("a", 0.5, true),
                    raw("b", 0.25, false),
                    raw("c", 0.0, false),
                ],
                ..BackendFrame::default()
            },
            60.0,
        );
        assert_eq!(
            displaced.messages[0].gamepad.index, 2,
            "a reconnect never evicts the device occupying its preferred slot"
        );
    }

    #[test]
    fn registry_builds_once_on_connect_and_diffs_input_in_place() {
        let mut registry = Registry::default();
        let connected = registry.apply(
            BackendFrame {
                changes: vec![ConnectionChange::Connected(raw("a", 0.25, true))],
                ..BackendFrame::default()
            },
            10.0,
        );
        assert!(connected.snapshots_changed);
        assert_eq!(registry.snapshot_builds(), 1);

        let unchanged = registry.apply(
            BackendFrame {
                connected: vec![raw("a", 0.25, true)],
                ..BackendFrame::default()
            },
            20.0,
        );
        assert!(!unchanged.snapshots_changed);
        assert_eq!(registry.snapshot_builds(), 1);
        assert_eq!(registry.snapshots()[0].as_ref().unwrap().timestamp, 10.0);

        let changed = registry.apply(
            BackendFrame {
                connected: vec![raw("a", 0.75, true)],
                ..BackendFrame::default()
            },
            30.0,
        );
        assert!(changed.snapshots_changed);
        assert_eq!(registry.snapshot_builds(), 1);
        let snapshot = registry.snapshots()[0].clone().unwrap();
        assert_eq!(snapshot.axes[0], 0.75);
        assert_eq!(snapshot.timestamp, 30.0);
    }

    #[test]
    fn vibration_targets_the_registry_identity_and_refuses_unsupported_slots() {
        gamepad::reset();
        gamepad::touch_for_test();
        let polls = Rc::new(RefCell::new(0));
        let vibrations = Rc::new(RefCell::new(Vec::new()));
        let mut controller = Controller::with_backend(Box::new(FakeBackend {
            polls,
            frames: VecDeque::from([BackendFrame {
                changes: vec![
                    ConnectionChange::Connected(raw("a", 0.0, true)),
                    ConnectionChange::Connected(raw("b", 0.0, false)),
                ],
                connected: vec![raw("a", 0.0, true), raw("b", 0.0, false)],
                ..BackendFrame::default()
            }]),
            vibrations: Rc::clone(&vibrations),
        }));
        controller.poll(1.0);
        controller.apply_request(&VibrationRequest {
            command_id: 1,
            index: 0,
            strong: 0.8,
            weak: 0.3,
            duration_ms: 250,
            start_delay_ms: 75,
        });
        assert_eq!(
            &*vibrations.borrow(),
            &[("a".to_owned(), 0.8, 0.3, 250, 75)]
        );
        assert_eq!(controller.active_vibrations.get("a"), Some(&1));
        controller.apply_request(&VibrationRequest {
            command_id: 2,
            index: 1,
            strong: 1.0,
            weak: 1.0,
            duration_ms: 10,
            start_delay_ms: 0,
        });
        assert_eq!(controller.active_vibrations.get("a"), Some(&1));
        assert_eq!(
            gamepad::take_completion_results(),
            [(2, None, Some("NotSupportedError".to_owned()))]
        );
    }

    #[test]
    fn vibration_settles_on_backend_completion_and_preemption_not_a_clock() {
        gamepad::reset();
        gamepad::touch_for_test();
        let mut controller = Controller::with_backend(Box::new(FakeBackend {
            polls: Rc::new(RefCell::new(0)),
            frames: VecDeque::from([
                BackendFrame {
                    changes: vec![ConnectionChange::Connected(raw("a", 0.0, true))],
                    connected: vec![raw("a", 0.0, true)],
                    ..BackendFrame::default()
                },
                BackendFrame {
                    connected: vec![raw("a", 0.0, true)],
                    vibration_completed: vec!["a".to_owned()],
                    ..BackendFrame::default()
                },
            ]),
            vibrations: Rc::new(RefCell::new(Vec::new())),
        }));
        controller.poll(1.0);
        let request = |command_id, duration_ms| VibrationRequest {
            command_id,
            index: 0,
            strong: 1.0,
            weak: 0.5,
            duration_ms,
            start_delay_ms: 40,
        };

        controller.apply_request(&request(1, 1_000));
        assert!(
            gamepad::take_completion_results().is_empty(),
            "accepting the effect does not complete its promise"
        );
        controller.apply_request(&request(2, 2_000));
        assert_eq!(
            gamepad::take_completion_results(),
            [(1, Some("preempted".to_owned()), None)]
        );
        assert_eq!(controller.active_vibrations.get("a"), Some(&2));

        controller.poll(2.0);
        assert_eq!(
            gamepad::take_completion_results(),
            [(2, Some("complete".to_owned()), None)],
            "only the backend's completion event settles the active effect"
        );

        controller.apply_request(&request(3, 1_000));
        controller.apply_request(&VibrationRequest {
            command_id: 4,
            index: 0,
            strong: 0.0,
            weak: 0.0,
            duration_ms: 0,
            start_delay_ms: 0,
        });
        assert_eq!(
            gamepad::take_completion_results(),
            [
                (3, Some("preempted".to_owned()), None),
                (4, Some("complete".to_owned()), None),
            ],
            "reset settles after the backend accepts the physical stop"
        );
    }
}
