//! Pointer input: winit's pointer events translated into DOM pointer identity.
//!
//! winit 0.31 unified mice, touchscreens and tablet tools into one set of
//! pointer events, so nothing here has to *receive* touch — it arrives on the
//! same `PointerMoved`/`PointerButton` pair a mouse does, carrying a
//! [`FingerId`](winit::event::FingerId) and a contact force. What is Blitsen's
//! is the translation: which DOM `pointerId` a contact is, what `pointerType`
//! names it, and how hard it is pressing.
//!
//! The state machine that follows from those events — per-pointer buttons,
//! pointer capture, the compatibility mouse events and `click` — is not here.
//! It lives in `dom_bridge/bootstrap/events.js`, because capture retargets an
//! event at a node the DOM chose, and a target the host picked by hit testing
//! would then be the wrong one to do the `click` bookkeeping against.
//!
//! [`dispatch`] is the seam between the two: what the queue drains into.

mod dispatch;

use std::collections::HashMap;

use winit::event::{
    ButtonSource, ElementState, Force, MouseButton, MouseScrollDelta, PointerKind, PointerSource,
    TabletToolKind, WindowEvent,
};
use winit::keyboard::ModifiersState;

/// Which physical device a pointer event came from, in the DOM's vocabulary.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum PointerType {
    Mouse,
    Touch,
    Pen,
}

impl PointerType {
    /// The string the `pointerType` member reports.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Mouse => "mouse",
            Self::Touch => "touch",
            Self::Pen => "pen",
        }
    }
}

/// One physical contact, as far as allocating a DOM `pointerId` is concerned.
///
/// winit hands out a `FingerId` per touch and says outright that the system may
/// reuse it once the contact has ended, so a finger id cannot be a `pointerId`:
/// the DOM requires that a new contact is a new pointer. A tablet tool has no
/// id at all, only a kind, so a pen is one pointer per kind — two pens on one
/// tablet are indistinguishable at this layer, and pretending otherwise would
/// invent an identity winit never reported.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum PointerContact {
    Mouse,
    Finger(usize),
    Tool(TabletToolKind),
}

/// The DOM `pointerId` the mouse always has.
///
/// Not 0: the spec reserves that for a pointer whose identity is unknown, and a
/// library that special-cases the mouse tests for 1.
const MOUSE_POINTER_ID: i64 = 1;

/// CSS pixels used for one abstract line step reported by winit.
///
/// Blitsen exposes wheel events in pixel mode (`deltaMode === 0`), so a platform
/// line delta needs one stable conversion before it reaches both the event and
/// its default scroll action. Pixel deltas are already in that established
/// coordinate space and pass through unchanged.
const WHEEL_CSS_PIXELS_PER_LINE: f64 = 40.0;

/// DOM `pointerId`s, allocated per contact and retired when the contact ends.
///
/// The table holds the whole of what was last dispatched for a contact, not
/// just its id, because a contact can be taken away by something that carries
/// no description of it — a `PointerLeft` with no position, or the surface
/// being destroyed underneath a gesture (see `surface_lifecycle`). Spelling
/// that as a `pointercancel` needs the pointer's type and whether it was
/// primary, and the last event that named them is the only place they existed.
pub(crate) struct PointerIds {
    live: HashMap<PointerContact, PointerDetails>,
    next: i64,
}

impl Default for PointerIds {
    fn default() -> Self {
        Self {
            live: HashMap::new(),
            next: MOUSE_POINTER_ID + 1,
        }
    }
}

impl PointerIds {
    /// Resolves a device into the DOM pointer it is, allocating an id if new.
    ///
    /// Ids are never reused, so a finger the platform re-numbered is still a new
    /// pointer to the DOM — which is the whole point of not keying on `FingerId`.
    pub(crate) fn details_for(
        &mut self,
        identity: PointerIdentity,
        primary: bool,
    ) -> PointerDetails {
        let pointer_id = match identity.contact {
            PointerContact::Mouse => MOUSE_POINTER_ID,
            contact => match self.live.get(&contact) {
                Some(live) => live.pointer_id,
                None => {
                    let id = self.next;
                    self.next += 1;
                    id
                }
            },
        };
        let details = PointerDetails {
            pointer_id,
            pointer_type: identity.pointer_type,
            primary,
            identity,
        };
        // The mouse is deliberately absent: it has a fixed id, it is never
        // "live" in the sense the cancellation path means, and it is the one
        // pointer the platform never takes away without saying so.
        if identity.contact != PointerContact::Mouse {
            self.live.insert(identity.contact, details);
        }
        details
    }

    /// Forgets a contact, so the next one spelled the same way is a new pointer.
    pub(crate) fn retire(&mut self, contact: PointerContact) {
        self.live.remove(&contact);
    }

    /// Reports whether a contact is one this registry has an id out for.
    ///
    /// The mouse is never in the table — it has a fixed id — and is never live
    /// in this sense, which is what keeps it out of the cancellation path.
    pub(crate) fn is_live(&self, contact: PointerContact) -> bool {
        self.live.contains_key(&contact)
    }

    /// Every contact still on the books, in the order they were allocated.
    ///
    /// Ordered so that a caller cancelling all of them dispatches in a stable
    /// sequence rather than whatever order the hash map happens to hold.
    pub(crate) fn live(&self) -> Vec<PointerDetails> {
        let mut live: Vec<_> = self.live.values().copied().collect();
        live.sort_by_key(|details| details.pointer_id);
        live
    }

    /// Forgets every contact, for a document that is being replaced.
    pub(crate) fn clear(&mut self) {
        self.live.clear();
    }
}

/// What a winit pointer source says about the device behind an event.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PointerIdentity {
    pub(crate) contact: PointerContact,
    pub(crate) pointer_type: PointerType,
    /// Contact force normalized to 0..=1, where the device reports one at all.
    ///
    /// `None` is "this device has no pressure sensor", which the DOM spells as
    /// 0.5 while a button is down — a substitution made in JavaScript, beside
    /// the button state it depends on, rather than guessed at here.
    pub(crate) force: Option<f64>,
    pub(crate) tilt_x: f64,
    pub(crate) tilt_y: f64,
    pub(crate) twist: f64,
    pub(crate) tangential_pressure: f64,
}

impl PointerIdentity {
    fn mouse() -> Self {
        Self {
            contact: PointerContact::Mouse,
            pointer_type: PointerType::Mouse,
            force: None,
            tilt_x: 0.0,
            tilt_y: 0.0,
            twist: 0.0,
            tangential_pressure: 0.0,
        }
    }

    fn touch(finger: usize, force: Option<&Force>) -> Self {
        Self {
            contact: PointerContact::Finger(finger),
            pointer_type: PointerType::Touch,
            force: force.map(|force| force.normalized(None)),
            tilt_x: 0.0,
            tilt_y: 0.0,
            twist: 0.0,
            tangential_pressure: 0.0,
        }
    }

    /// A finger with no pressure reading, for tests in neighbouring modules.
    #[cfg(test)]
    pub(crate) fn touch_for_test(finger: usize) -> Self {
        Self::touch(finger, None)
    }

    /// The mouse, for tests in neighbouring modules.
    #[cfg(test)]
    pub(crate) fn mouse_for_test() -> Self {
        Self::mouse()
    }

    fn tool(kind: TabletToolKind, data: &winit::event::TabletToolData) -> Self {
        let tilt = data.tilt.unwrap_or_default();
        Self {
            contact: PointerContact::Tool(kind),
            // A tablet reports which tool is in the hand; the DOM has two names
            // for the whole family, and an eraser is the one it distinguishes.
            pointer_type: PointerType::Pen,
            force: data.force.as_ref().map(|force| force.normalized(None)),
            tilt_x: f64::from(tilt.x),
            tilt_y: f64::from(tilt.y),
            twist: data.twist.map_or(0.0, f64::from),
            tangential_pressure: data.tangential_force.map_or(0.0, f64::from),
        }
    }
}

/// The device behind a `PointerMoved`, or `None` for one winit could not name.
pub(crate) fn identity_of_source(source: &PointerSource) -> Option<PointerIdentity> {
    match source {
        PointerSource::Mouse => Some(PointerIdentity::mouse()),
        PointerSource::Touch { finger_id, force } => {
            Some(PointerIdentity::touch(finger_id.into_raw(), force.as_ref()))
        }
        PointerSource::TabletTool { kind, data } => Some(PointerIdentity::tool(*kind, data)),
        // Unknown is a device the platform could not classify. It is not
        // discarded: a pointer with no name still moves and still presses, and
        // reporting it as a mouse is what `PointerKind` itself does with the
        // Wayland and X11 devices it cannot identify.
        PointerSource::Unknown => Some(PointerIdentity::mouse()),
    }
}

/// The device and DOM button behind a `PointerButton`.
pub(crate) fn identity_of_button(
    button: &ButtonSource,
) -> Option<(PointerIdentity, Option<MouseButton>)> {
    match button {
        ButtonSource::Mouse(button) => Some((PointerIdentity::mouse(), Some(*button))),
        // A finger has one button and it is the primary one, which is what
        // `ButtonSource::mouse_button` says too.
        ButtonSource::Touch { finger_id, force } => Some((
            PointerIdentity::touch(finger_id.into_raw(), force.as_ref()),
            Some(MouseButton::Left),
        )),
        ButtonSource::TabletTool { kind, button, data } => Some((
            PointerIdentity::tool(*kind, data),
            Option::<MouseButton>::from(*button),
        )),
        // A button with no name has no DOM number either. Dropping it is what
        // this match did with every non-mouse source before touch was accepted.
        ButtonSource::Unknown(_) => None,
    }
}

/// The contact a `PointerEntered`/`PointerLeft` refers to.
pub(crate) fn contact_of_kind(kind: PointerKind) -> PointerContact {
    match kind {
        PointerKind::Touch(finger_id) => PointerContact::Finger(finger_id.into_raw()),
        PointerKind::TabletTool(kind) => PointerContact::Tool(kind),
        PointerKind::Mouse | PointerKind::Unknown => PointerContact::Mouse,
    }
}

/// A pointer event held until the frame that will act on it.
#[derive(Clone, Copy)]
pub(crate) enum PendingPointerInput {
    Move {
        physical_x: f64,
        physical_y: f64,
        pointer: PointerDetails,
    },
    Button {
        physical_x: f64,
        physical_y: f64,
        button: MouseButton,
        state: ElementState,
        pointer: PointerDetails,
    },
    /// The platform took the contact away without a release — a touch the system
    /// stopped tracking, which the DOM calls `pointercancel`.
    Cancel {
        physical_x: f64,
        physical_y: f64,
        pointer: PointerDetails,
    },
    Wheel {
        delta_x: f64,
        delta_y: f64,
    },
}

impl PendingPointerInput {
    /// The pointer behind this input, which the wheel alone does not have.
    ///
    /// The routing rule reads this: an input with a pointer goes to the DOM's
    /// pointer dispatcher, and the one without goes to the mouse one, because a
    /// wheel is a `MouseEvent` and its default action is a scroll.
    pub(crate) fn pointer(&self) -> Option<PointerDetails> {
        match self {
            Self::Move { pointer, .. }
            | Self::Button { pointer, .. }
            | Self::Cancel { pointer, .. } => Some(*pointer),
            Self::Wheel { .. } => None,
        }
    }
}

/// One pointer's DOM identity, resolved when the event was queued.
///
/// Resolved there rather than at dispatch because allocating a `pointerId`
/// mutates the registry, and the queue is drained behind a shared borrow.
#[derive(Clone, Copy)]
pub(crate) struct PointerDetails {
    pub(crate) pointer_id: i64,
    pub(crate) pointer_type: PointerType,
    pub(crate) primary: bool,
    pub(crate) identity: PointerIdentity,
}
/// What a winit window event means to the pointer queue.
pub(crate) enum PointerAction {
    /// Queue this input, and remember the position it happened at.
    Queue(PendingPointerInput, Option<(f64, f64)>),
    /// The modifier state changed; there is nothing to dispatch.
    Modifiers(ModifiersState),
    /// The mouse left the window, so its last position is no longer where it is.
    MouseGone,
    /// Not a pointer event, or one with no DOM meaning.
    Ignore,
}

/// Normalizes winit's two wheel units into the pixel-mode DOM contract.
fn wheel_delta_in_css_pixels(delta: &MouseScrollDelta) -> (f64, f64) {
    match delta {
        MouseScrollDelta::LineDelta(x, y) => (
            f64::from(*x) * WHEEL_CSS_PIXELS_PER_LINE,
            f64::from(*y) * WHEEL_CSS_PIXELS_PER_LINE,
        ),
        MouseScrollDelta::PixelDelta(position) => (position.x, position.y),
    }
}

/// Reads one winit window event as the DOM pointer input it is.
///
/// Split out from the queue so the whole translation — including which sources
/// are accepted at all, which is what used to discard every touch — can be
/// exercised against real `WindowEvent`s without a window behind them.
pub(crate) fn classify_pointer_event(
    event: &WindowEvent,
    ids: &mut PointerIds,
    last_position: Option<(f64, f64)>,
) -> PointerAction {
    match event {
        WindowEvent::PointerMoved {
            position,
            primary,
            source,
            ..
        } => {
            let Some(identity) = identity_of_source(source) else {
                return PointerAction::Ignore;
            };
            PointerAction::Queue(
                PendingPointerInput::Move {
                    physical_x: position.x,
                    physical_y: position.y,
                    pointer: ids.details_for(identity, *primary),
                },
                Some((position.x, position.y)),
            )
        }
        WindowEvent::PointerButton {
            position,
            primary,
            button,
            state,
            ..
        } => {
            let Some((identity, Some(button))) = identity_of_button(button) else {
                return PointerAction::Ignore;
            };
            let pointer = ids.details_for(identity, *primary);
            // A contact that has been lifted is finished, and the next one the
            // platform numbers the same way is a different pointer.
            if *state == ElementState::Released && identity.contact != PointerContact::Mouse {
                ids.retire(identity.contact);
            }
            PointerAction::Queue(
                PendingPointerInput::Button {
                    physical_x: position.x,
                    physical_y: position.y,
                    button,
                    state: *state,
                    pointer,
                },
                Some((position.x, position.y)),
            )
        }
        WindowEvent::MouseWheel { delta, .. } => {
            let (delta_x, delta_y) = wheel_delta_in_css_pixels(delta);
            PointerAction::Queue(PendingPointerInput::Wheel { delta_x, delta_y }, None)
        }
        WindowEvent::ModifiersChanged(modifiers) => PointerAction::Modifiers(modifiers.state()),
        // Outside the window the cursor belongs to whatever the pointer is over
        // now, and the position last reported is no longer where it is.
        //
        // For a touch this is also how a cancelled gesture arrives: winit emits
        // `PointerLeft` for a contact the system stopped tracking *without* the
        // release that would normally precede it, so a contact still on the
        // books here is one the DOM has to be told was cancelled.
        WindowEvent::PointerLeft {
            position,
            primary,
            kind,
            ..
        } => {
            let contact = contact_of_kind(*kind);
            if contact == PointerContact::Mouse {
                return PointerAction::MouseGone;
            }
            if !ids.is_live(contact) {
                return PointerAction::Ignore;
            }
            let identity = match kind {
                PointerKind::Touch(finger) => PointerIdentity::touch(finger.into_raw(), None),
                _ => PointerIdentity::mouse(),
            };
            let pointer = ids.details_for(identity, *primary);
            ids.retire(contact);
            let (physical_x, physical_y) = position
                .map(|position| (position.x, position.y))
                .or(last_position)
                .unwrap_or_default();
            PointerAction::Queue(
                PendingPointerInput::Cancel {
                    physical_x,
                    physical_y,
                    pointer,
                },
                None,
            )
        }
        _ => PointerAction::Ignore,
    }
}

#[cfg(test)]
mod tests {
    use winit::dpi::PhysicalPosition;
    use winit::event::{FingerId, TabletToolData};

    use super::*;

    /// The queued input, or a description of what was done with it instead.
    fn queued(action: PointerAction) -> PendingPointerInput {
        match action {
            PointerAction::Queue(input, _) => input,
            PointerAction::Modifiers(_) => panic!("expected a queued pointer input, got modifiers"),
            PointerAction::MouseGone => panic!("expected a queued pointer input, got mouse gone"),
            PointerAction::Ignore => panic!("expected a queued pointer input, got ignore"),
        }
    }

    fn touch_press(finger: usize, primary: bool, state: ElementState) -> WindowEvent {
        WindowEvent::PointerButton {
            device_id: None,
            state,
            position: PhysicalPosition::new(12.0, 34.0),
            primary,
            button: ButtonSource::Touch {
                finger_id: FingerId::from_raw(finger),
                force: Some(Force::Normalized(0.75)),
            },
        }
    }

    fn classified_wheel(delta: MouseScrollDelta) -> (f64, f64) {
        let input = queued(classify_pointer_event(
            &WindowEvent::MouseWheel {
                device_id: None,
                delta,
                phase: winit::event::TouchPhase::Moved,
            },
            &mut PointerIds::default(),
            None,
        ));
        let PendingPointerInput::Wheel { delta_x, delta_y } = input else {
            panic!("a mouse wheel event is queued as a wheel input");
        };
        (delta_x, delta_y)
    }

    #[test]
    fn wheel_lines_use_the_named_pixel_policy_and_preserve_axis_signs() {
        assert_eq!(
            classified_wheel(MouseScrollDelta::LineDelta(-2.0, 1.5)),
            (-80.0, 60.0)
        );
    }

    #[test]
    fn wheel_pixels_pass_through_with_axis_signs_unchanged() {
        assert_eq!(
            classified_wheel(MouseScrollDelta::PixelDelta(PhysicalPosition::new(
                -3.25, 7.5,
            ))),
            (-3.25, 7.5)
        );
    }

    #[test]
    fn a_touch_press_is_queued_as_a_touch_pointer_rather_than_discarded() {
        let mut ids = PointerIds::default();
        let press = queued(classify_pointer_event(
            &touch_press(4, true, ElementState::Pressed),
            &mut ids,
            None,
        ));
        let PendingPointerInput::Button {
            physical_x,
            physical_y,
            button,
            state,
            pointer,
        } = press
        else {
            panic!("a touch press is a button input");
        };
        assert_eq!((physical_x, physical_y), (12.0, 34.0));
        assert_eq!(button, MouseButton::Left);
        assert_eq!(state, ElementState::Pressed);
        assert_eq!(pointer.pointer_type, PointerType::Touch);
        assert!(pointer.primary);
        assert_ne!(pointer.pointer_id, MOUSE_POINTER_ID);
        assert_eq!(pointer.identity.force, Some(0.75));

        // A drag by the same finger is the same pointer …
        let moved = queued(classify_pointer_event(
            &WindowEvent::PointerMoved {
                device_id: None,
                position: PhysicalPosition::new(20.0, 40.0),
                primary: true,
                source: PointerSource::Touch {
                    finger_id: FingerId::from_raw(4),
                    force: None,
                },
            },
            &mut ids,
            None,
        ));
        let PendingPointerInput::Move {
            pointer: dragged, ..
        } = moved
        else {
            panic!("a touch drag is a move input");
        };
        assert_eq!(dragged.pointer_id, pointer.pointer_id);

        // … and a second finger is a second, non-primary one.
        let second = queued(classify_pointer_event(
            &touch_press(5, false, ElementState::Pressed),
            &mut ids,
            None,
        ));
        let PendingPointerInput::Button {
            pointer: second, ..
        } = second
        else {
            panic!("a touch press is a button input");
        };
        assert_ne!(second.pointer_id, pointer.pointer_id);
        assert!(!second.primary);
    }

    #[test]
    fn a_touch_the_system_stops_tracking_is_cancelled_and_a_released_one_is_not() {
        let mut ids = PointerIds::default();
        let pressed = queued(classify_pointer_event(
            &touch_press(1, true, ElementState::Pressed),
            &mut ids,
            None,
        ));
        let PendingPointerInput::Button { pointer, .. } = pressed else {
            panic!("a touch press is a button input");
        };
        let taken_away = WindowEvent::PointerLeft {
            device_id: None,
            position: None,
            primary: true,
            kind: PointerKind::Touch(FingerId::from_raw(1)),
        };
        // Still on the books, so this is a cancellation — at the last position
        // the contact was seen at, since the platform reported none.
        let cancelled = queued(classify_pointer_event(
            &taken_away,
            &mut ids,
            Some((7.0, 9.0)),
        ));
        let PendingPointerInput::Cancel {
            physical_x,
            physical_y,
            pointer: cancelled,
        } = cancelled
        else {
            panic!("an untracked contact is a cancellation");
        };
        assert_eq!((physical_x, physical_y), (7.0, 9.0));
        assert_eq!(cancelled.pointer_id, pointer.pointer_id);

        // A contact that ended with a release is already gone, so the
        // `PointerLeft` that follows it is not a second, cancelling end.
        let mut ids = PointerIds::default();
        classify_pointer_event(&touch_press(1, true, ElementState::Pressed), &mut ids, None);
        classify_pointer_event(
            &touch_press(1, true, ElementState::Released),
            &mut ids,
            None,
        );
        assert!(matches!(
            classify_pointer_event(&taken_away, &mut ids, None),
            PointerAction::Ignore
        ));

        // And the mouse leaving the window is neither: it is a position that is
        // no longer where the pointer is.
        assert!(matches!(
            classify_pointer_event(
                &WindowEvent::PointerLeft {
                    device_id: None,
                    position: None,
                    primary: true,
                    kind: PointerKind::Mouse,
                },
                &mut ids,
                None,
            ),
            PointerAction::MouseGone
        ));
    }

    #[test]
    fn touch_and_pen_sources_carry_a_dom_pointer_identity() {
        let touch = identity_of_source(&PointerSource::Touch {
            finger_id: FingerId::from_raw(7),
            force: Some(Force::Normalized(0.25)),
        })
        .expect("touch is a pointer");
        assert_eq!(touch.contact, PointerContact::Finger(7));
        assert_eq!(touch.pointer_type, PointerType::Touch);
        assert_eq!(touch.force, Some(0.25));

        let calibrated = identity_of_source(&PointerSource::Touch {
            finger_id: FingerId::from_raw(0),
            force: Some(Force::Calibrated {
                force: 1.0,
                max_possible_force: 4.0,
            }),
        })
        .expect("touch is a pointer");
        assert_eq!(calibrated.force, Some(0.25));

        let unsensed = identity_of_source(&PointerSource::Touch {
            finger_id: FingerId::from_raw(1),
            force: None,
        })
        .expect("touch is a pointer");
        assert_eq!(unsensed.force, None);

        let pen = identity_of_source(&PointerSource::TabletTool {
            kind: TabletToolKind::Pen,
            data: TabletToolData {
                force: Some(Force::Normalized(0.5)),
                twist: Some(90),
                ..Default::default()
            },
        })
        .expect("a tablet tool is a pointer");
        assert_eq!(pen.contact, PointerContact::Tool(TabletToolKind::Pen));
        assert_eq!(pen.pointer_type, PointerType::Pen);
        assert_eq!(pen.force, Some(0.5));
        assert_eq!(pen.twist, 90.0);

        assert_eq!(
            identity_of_source(&PointerSource::Mouse).map(|identity| identity.pointer_type),
            Some(PointerType::Mouse)
        );
    }

    #[test]
    fn a_touch_button_is_the_primary_one_and_an_unnamed_button_is_dropped() {
        let (identity, button) = identity_of_button(&ButtonSource::Touch {
            finger_id: FingerId::from_raw(3),
            force: None,
        })
        .expect("a touch press is a pointer press");
        assert_eq!(identity.contact, PointerContact::Finger(3));
        assert_eq!(button, Some(MouseButton::Left));
        assert!(identity_of_button(&ButtonSource::Unknown(9)).is_none());
        assert_eq!(
            identity_of_button(&ButtonSource::Mouse(MouseButton::Right)).map(|(_, button)| button),
            Some(Some(MouseButton::Right))
        );
    }

    #[test]
    fn pointer_ids_are_stable_per_contact_and_never_reused() {
        let mut ids = PointerIds::default();
        let id_for = |ids: &mut PointerIds, contact| match contact {
            PointerContact::Mouse => ids.details_for(PointerIdentity::mouse(), true).pointer_id,
            PointerContact::Finger(finger) => {
                ids.details_for(PointerIdentity::touch(finger, None), true)
                    .pointer_id
            }
            PointerContact::Tool(_) => unreachable!("this test has no tablet in it"),
        };
        assert_eq!(id_for(&mut ids, PointerContact::Mouse), 1);
        let first = id_for(&mut ids, PointerContact::Finger(0));
        let second = id_for(&mut ids, PointerContact::Finger(1));
        assert_ne!(first, second);
        assert_ne!(first, 1);
        // A contact keeps its id for as long as it lasts …
        assert_eq!(id_for(&mut ids, PointerContact::Finger(0)), first);
        assert_eq!(id_for(&mut ids, PointerContact::Mouse), 1);
        // … and the next finger the platform numbers 0 is a different pointer.
        ids.retire(PointerContact::Finger(0));
        assert!(!ids.is_live(PointerContact::Finger(0)));
        let third = id_for(&mut ids, PointerContact::Finger(0));
        assert_ne!(third, first);
        assert_ne!(third, second);
        assert!(ids.is_live(PointerContact::Finger(1)));
    }

    /// Two fingers down, then the surface goes away underneath them.
    ///
    /// The registry is what remembers there were contacts at all, so this is
    /// the assertion that a cancellation can be *spelled* — that everything a
    /// `pointercancel` needs survives an event nobody sent.
    #[test]
    fn live_contacts_are_reported_with_enough_detail_to_cancel_them() {
        let mut ids = PointerIds::default();
        classify_pointer_event(&touch_press(7, true, ElementState::Pressed), &mut ids, None);
        classify_pointer_event(
            &touch_press(8, false, ElementState::Pressed),
            &mut ids,
            None,
        );

        let live = ids.live();
        assert_eq!(live.len(), 2);
        assert!(live[0].pointer_id < live[1].pointer_id);
        assert_eq!(live[0].identity.contact, PointerContact::Finger(7));
        assert_eq!(live[0].pointer_type, PointerType::Touch);
        assert!(live[0].primary);
        assert_eq!(live[1].identity.contact, PointerContact::Finger(8));
        assert!(!live[1].primary);

        // A finger that was lifted is not cancelled a second time.
        classify_pointer_event(
            &touch_press(7, true, ElementState::Released),
            &mut ids,
            None,
        );
        let live = ids.live();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].identity.contact, PointerContact::Finger(8));
    }
}
