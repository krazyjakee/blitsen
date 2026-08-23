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
    ) -> Result<(), String>;
}

pub(crate) struct Controller {
    backend: Box<dyn Backend>,
    registry: Registry,
}

impl Controller {
    pub(crate) fn with_backend(backend: Box<dyn Backend>) -> Self {
        Self {
            backend,
            registry: Registry::default(),
        }
    }

    pub(crate) fn platform() -> Self {
        Self::with_backend(platform_backend())
    }

    /// Polls exactly once for this redraw and publishes one atomic registry view.
    pub(crate) fn poll(&mut self, now_ms: f64) {
        let messages = self.registry.apply(self.backend.poll(), now_ms);
        gamepad::publish(self.registry.snapshots(), messages);
    }

    pub(crate) fn apply_requests(&mut self) -> bool {
        let requests = gamepad::take_requests();
        let any = !requests.is_empty();
        for request in requests {
            let result = self.apply_request(&request);
            gamepad::complete(request.command_id, result);
        }
        any
    }

    fn apply_request(&mut self, request: &VibrationRequest) -> Result<(), (&'static str, String)> {
        let Some((key, vibration)) = self.registry.key_for_index(request.index) else {
            return Err((
                "NotFoundError",
                format!("gamepad slot {} is not connected", request.index),
            ));
        };
        if !vibration {
            return Err((
                "NotSupportedError",
                format!("gamepad slot {} has no vibration actuator", request.index),
            ));
        }
        self.backend
            .vibrate(key, request.strong, request.weak, request.duration_ms)
            .map_err(|message| ("OperationError", message))
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
    ) -> Result<(), String> {
        Err("the gamepad backend is unavailable".to_owned())
    }
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
struct GilrsBackend {
    gilrs: gilrs::Gilrs,
    known: std::collections::HashMap<String, gilrs::GamepadId>,
    effects: std::collections::HashMap<String, gilrs::ff::Effect>,
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
impl GilrsBackend {
    fn new() -> Result<Self, String> {
        let gilrs = gilrs::GilrsBuilder::new()
            .with_default_filters(true)
            .with_force_feedback(true)
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            gilrs,
            known: std::collections::HashMap::new(),
            effects: std::collections::HashMap::new(),
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
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
impl Backend for GilrsBackend {
    fn poll(&mut self) -> BackendFrame {
        use gilrs::EventType;

        let mut changes = Vec::new();
        while let Some(event) = self.gilrs.next_event() {
            let key = Self::key(event.id);
            match event.event {
                EventType::Connected => {
                    self.known.insert(key.clone(), event.id);
                    changes.push(ConnectionChange::Connected(Self::snapshot(
                        event.id,
                        &self.gilrs.gamepad(event.id),
                    )));
                }
                EventType::Disconnected => {
                    self.known.remove(&key);
                    self.effects.remove(&key);
                    changes.push(ConnectionChange::Disconnected(key));
                }
                _ => {}
            }
        }
        // Some backends enumerate already-connected devices without queuing an
        // initial event. Add those in backend-id order so startup is stable.
        let mut connected = self.gilrs.gamepads().collect::<Vec<_>>();
        connected.sort_by_key(|(id, _)| usize::from(*id));
        for (id, gamepad) in &connected {
            let key = Self::key(*id);
            if !self.known.contains_key(&key) {
                self.known.insert(key, *id);
                changes.push(ConnectionChange::Connected(Self::snapshot(*id, gamepad)));
            }
        }
        let connected = connected
            .into_iter()
            .map(|(id, gamepad)| Self::snapshot(id, &gamepad))
            .collect();
        self.gilrs.inc();
        BackendFrame { changes, connected }
    }

    fn vibrate(
        &mut self,
        key: &str,
        strong: f64,
        weak: f64,
        duration_ms: u32,
    ) -> Result<(), String> {
        use gilrs::ff::{BaseEffect, BaseEffectType, EffectBuilder, Repeat, Replay, Ticks};

        if let Some(effect) = self.effects.remove(key) {
            let _ = effect.stop();
        }
        if duration_ms == 0 || (strong == 0.0 && weak == 0.0) {
            return Ok(());
        }
        let id = *self
            .known
            .get(key)
            .ok_or_else(|| "the gamepad disconnected before vibration started".to_owned())?;
        let ticks = Ticks::from_ms(duration_ms.max(1));
        let magnitude = |value: f64| (value.clamp(0.0, 1.0) * f64::from(u16::MAX)).round() as u16;
        let mut builder = EffectBuilder::new();
        builder
            .add_effect(BaseEffect {
                kind: BaseEffectType::Strong {
                    magnitude: magnitude(strong),
                },
                scheduling: Replay {
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
                    play_for: ticks,
                    ..Default::default()
                },
                ..Default::default()
            })
            .gamepads(&[id])
            .repeat(Repeat::For(ticks));
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

    struct FakeBackend {
        polls: Rc<RefCell<usize>>,
        frames: VecDeque<BackendFrame>,
        vibrations: Rc<RefCell<Vec<(String, f64, f64, u32)>>>,
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
        ) -> Result<(), String> {
            self.vibrations
                .borrow_mut()
                .push((key.to_owned(), strong, weak, duration_ms));
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
    fn polling_is_injected_and_does_no_work_until_a_frame_asks() {
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
        assert_eq!(*polls.borrow(), 1, "one frame is exactly one backend poll");
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
            },
            10.0,
        );
        assert_eq!(
            first.iter().map(|event| event.kind).collect::<Vec<_>>(),
            ["connected", "connected"]
        );
        assert_eq!(
            first
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
            },
            20.0,
        );
        assert_eq!(disconnected[0].kind, "disconnected");
        assert_eq!(disconnected[0].gamepad.index, 0);
        assert!(!disconnected[0].gamepad.connected);
        assert!(registry.snapshots()[0].is_none());

        let reconnected = registry.apply(
            BackendFrame {
                changes: vec![ConnectionChange::Connected(raw("a", 0.5, true))],
                connected: vec![raw("a", 0.5, true), raw("b", 0.25, false)],
            },
            30.0,
        );
        assert_eq!(
            reconnected[0].gamepad.index, 0,
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
            },
            40.0,
        );
        let replacement = registry.apply(
            BackendFrame {
                changes: vec![ConnectionChange::Connected(raw("c", 0.0, false))],
                connected: vec![raw("b", 0.25, false), raw("c", 0.0, false)],
            },
            50.0,
        );
        assert_eq!(replacement[0].gamepad.index, 0, "the lowest hole is reused");
        let displaced = registry.apply(
            BackendFrame {
                changes: vec![ConnectionChange::Connected(raw("a", 0.5, true))],
                connected: vec![
                    raw("a", 0.5, true),
                    raw("b", 0.25, false),
                    raw("c", 0.0, false),
                ],
            },
            60.0,
        );
        assert_eq!(
            displaced[0].gamepad.index, 2,
            "a reconnect never evicts the device occupying its preferred slot"
        );
    }

    #[test]
    fn vibration_targets_the_registry_identity_and_refuses_unsupported_slots() {
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
            }]),
            vibrations: Rc::clone(&vibrations),
        }));
        controller.poll(1.0);
        controller
            .apply_request(&VibrationRequest {
                command_id: 1,
                index: 0,
                strong: 0.8,
                weak: 0.3,
                duration_ms: 250,
            })
            .expect("the actuator starts");
        assert_eq!(&*vibrations.borrow(), &[("a".to_owned(), 0.8, 0.3, 250)]);
        assert_eq!(
            controller
                .apply_request(&VibrationRequest {
                    command_id: 2,
                    index: 1,
                    strong: 1.0,
                    weak: 1.0,
                    duration_ms: 10,
                })
                .unwrap_err()
                .0,
            "NotSupportedError"
        );
    }
}
