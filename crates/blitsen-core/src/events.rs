//! Runtime-neutral DOM event propagation and listener bookkeeping.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::Hash;
use std::rc::Rc;
use std::time::Duration;

/// DOM event phase values exposed to JavaScript.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum EventPhase {
    /// Event is not currently being dispatched.
    None = 0,
    /// Root-to-parent capture traversal.
    Capturing = 1,
    /// Both capture and non-capture listeners on the target.
    AtTarget = 2,
    /// Parent-to-root bubble traversal.
    Bubbling = 3,
}

/// Mutable event state shared by every listener in one dispatch.
#[derive(Clone, Debug)]
pub struct DomEvent<T> {
    event_type: String,
    target: T,
    current_target: Option<T>,
    phase: EventPhase,
    bubbles: bool,
    cancelable: bool,
    default_prevented: bool,
    propagation_stopped: bool,
    immediate_propagation_stopped: bool,
    time_stamp: Duration,
}

impl<T: Copy> DomEvent<T> {
    /// Creates an event ready for synchronous dispatch.
    pub fn new(
        event_type: impl Into<String>,
        target: T,
        bubbles: bool,
        cancelable: bool,
        time_stamp: Duration,
    ) -> Self {
        Self {
            event_type: event_type.into(),
            target,
            current_target: None,
            phase: EventPhase::None,
            bubbles,
            cancelable,
            default_prevented: false,
            propagation_stopped: false,
            immediate_propagation_stopped: false,
            time_stamp,
        }
    }

    /// Event type without an `on` prefix.
    pub fn event_type(&self) -> &str {
        &self.event_type
    }
    /// Original dispatch target.
    pub fn target(&self) -> T {
        self.target
    }
    /// Target whose listener list is currently being invoked.
    pub fn current_target(&self) -> Option<T> {
        self.current_target
    }
    /// Current propagation phase.
    pub fn phase(&self) -> EventPhase {
        self.phase
    }
    /// Whether the event traverses ancestors after its target.
    pub fn bubbles(&self) -> bool {
        self.bubbles
    }
    /// Whether `preventDefault` can cancel the event.
    pub fn cancelable(&self) -> bool {
        self.cancelable
    }
    /// Whether a listener successfully cancelled the event.
    pub fn default_prevented(&self) -> bool {
        self.default_prevented
    }
    /// Monotonic time since application start.
    pub fn time_stamp(&self) -> Duration {
        self.time_stamp
    }

    /// Cancels the default action when the event is cancelable.
    pub fn prevent_default(&mut self) {
        if self.cancelable {
            self.default_prevented = true;
        }
    }

    /// Prevents traversal to another target without skipping peers here.
    pub fn stop_propagation(&mut self) {
        self.propagation_stopped = true;
    }

    /// Skips all remaining listeners and propagation targets.
    pub fn stop_immediate_propagation(&mut self) {
        self.propagation_stopped = true;
        self.immediate_propagation_stopped = true;
    }

    fn finish(&mut self) {
        self.current_target = None;
        self.phase = EventPhase::None;
    }
}

/// Stable registry identifier for one listener registration.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ListenerId(u64);

#[derive(Clone)]
struct Listener<C> {
    id: ListenerId,
    callback: C,
    capture: bool,
    once: bool,
}

/// Listener lists and the complete capture/target/bubble algorithm.
pub struct EventRegistry<T, C> {
    listeners: HashMap<(T, String), Vec<Listener<C>>>,
    next_id: u64,
}

impl<T, C> Default for EventRegistry<T, C> {
    fn default() -> Self {
        Self {
            listeners: HashMap::new(),
            next_id: 1,
        }
    }
}

impl<T: Copy + Eq + Hash, C: Clone> EventRegistry<T, C> {
    /// Creates an empty listener registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a listener and returns its stable registration identifier.
    pub fn add(
        &mut self,
        target: T,
        event_type: impl Into<String>,
        callback: C,
        capture: bool,
        once: bool,
    ) -> ListenerId {
        let id = ListenerId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.listeners
            .entry((target, event_type.into()))
            .or_default()
            .push(Listener {
                id,
                callback,
                capture,
                once,
            });
        id
    }

    /// Removes a listener by registration identifier.
    pub fn remove(&mut self, id: ListenerId) -> bool {
        for listeners in self.listeners.values_mut() {
            if let Some(index) = listeners.iter().position(|listener| listener.id == id) {
                listeners.remove(index);
                return true;
            }
        }
        false
    }

    /// Dispatches along a connected root-to-target path.
    ///
    /// `invoke` receives a cloned callback so it may freely mutate `registry`.
    /// Exceptions are passed to `report` per listener and never terminate the
    /// remaining listener sequence.
    pub fn dispatch<E>(
        registry: &Rc<RefCell<Self>>,
        mut event: DomEvent<T>,
        path: &[T],
        mut invoke: impl FnMut(C, &mut DomEvent<T>) -> Result<(), E>,
        mut report: impl FnMut(E),
    ) -> DomEvent<T> {
        if path.is_empty() || path.last().copied() != Some(event.target) {
            event.finish();
            return event;
        }

        for target in &path[..path.len() - 1] {
            Self::invoke_target(
                registry,
                &mut event,
                *target,
                EventPhase::Capturing,
                true,
                &mut invoke,
                &mut report,
            );
            if event.propagation_stopped {
                event.finish();
                return event;
            }
        }

        let target = event.target;
        let target_ids = Self::listener_ids(registry, target, event.event_type());
        Self::invoke_ids(
            registry,
            &mut event,
            target,
            EventPhase::AtTarget,
            true,
            &target_ids,
            &mut invoke,
            &mut report,
        );
        Self::invoke_ids(
            registry,
            &mut event,
            target,
            EventPhase::AtTarget,
            false,
            &target_ids,
            &mut invoke,
            &mut report,
        );

        if event.bubbles && !event.propagation_stopped {
            for target in path[..path.len() - 1].iter().rev() {
                Self::invoke_target(
                    registry,
                    &mut event,
                    *target,
                    EventPhase::Bubbling,
                    false,
                    &mut invoke,
                    &mut report,
                );
                if event.propagation_stopped {
                    break;
                }
            }
        }
        event.finish();
        event
    }

    fn invoke_target<E>(
        registry: &Rc<RefCell<Self>>,
        event: &mut DomEvent<T>,
        target: T,
        phase: EventPhase,
        capture: bool,
        invoke: &mut impl FnMut(C, &mut DomEvent<T>) -> Result<(), E>,
        report: &mut impl FnMut(E),
    ) {
        let ids = Self::listener_ids(registry, target, event.event_type());
        Self::invoke_ids(
            registry, event, target, phase, capture, &ids, invoke, report,
        );
    }

    fn listener_ids(registry: &Rc<RefCell<Self>>, target: T, event_type: &str) -> Vec<ListenerId> {
        registry
            .borrow()
            .listeners
            .get(&(target, event_type.to_owned()))
            .map(|listeners| {
                listeners
                    .iter()
                    .map(|listener| listener.id)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    }

    #[allow(clippy::too_many_arguments)]
    fn invoke_ids<E>(
        registry: &Rc<RefCell<Self>>,
        event: &mut DomEvent<T>,
        target: T,
        phase: EventPhase,
        capture: bool,
        ids: &[ListenerId],
        invoke: &mut impl FnMut(C, &mut DomEvent<T>) -> Result<(), E>,
        report: &mut impl FnMut(E),
    ) {
        event.current_target = Some(target);
        event.phase = phase;
        for id in ids {
            if event.immediate_propagation_stopped {
                break;
            }
            let listener = registry
                .borrow()
                .listeners
                .get(&(target, event.event_type.clone()))
                .and_then(|listeners| listeners.iter().find(|listener| listener.id == *id))
                .cloned();
            let Some(listener) = listener else {
                continue;
            };
            if listener.capture != capture {
                continue;
            }
            if listener.once {
                registry.borrow_mut().remove(*id);
            }
            if let Err(error) = invoke(listener.callback, event) {
                report(error);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    enum Action {
        Record(&'static str),
        Throw(&'static str),
        Stop(&'static str),
        StopImmediate(&'static str),
        Prevent(&'static str),
        Remove(&'static str, ListenerId),
        Add(&'static str),
    }

    #[test]
    fn dispatches_capture_target_and_bubble_with_exception_isolation() {
        let registry = Rc::new(RefCell::new(EventRegistry::new()));
        registry
            .borrow_mut()
            .add("root", "click", Action::Record("root-capture"), true, false);
        registry.borrow_mut().add(
            "parent",
            "click",
            Action::Throw("parent-throw"),
            true,
            false,
        );
        registry.borrow_mut().add(
            "target",
            "click",
            Action::Record("target-capture"),
            true,
            false,
        );
        registry.borrow_mut().add(
            "target",
            "click",
            Action::Prevent("target-bubble"),
            false,
            false,
        );
        registry.borrow_mut().add(
            "parent",
            "click",
            Action::Record("parent-bubble"),
            false,
            false,
        );
        let mut order = Vec::new();
        let mut errors = Vec::new();
        let result = EventRegistry::dispatch(
            &registry,
            DomEvent::new("click", "target", true, true, Duration::from_millis(12)),
            &["root", "parent", "target"],
            |action, event| match action {
                Action::Record(name) => {
                    order.push((name, event.phase()));
                    Ok(())
                }
                Action::Throw(name) => {
                    order.push((name, event.phase()));
                    Err(name)
                }
                Action::Prevent(name) => {
                    order.push((name, event.phase()));
                    event.prevent_default();
                    Ok(())
                }
                _ => unreachable!(),
            },
            |error| errors.push(error),
        );
        assert_eq!(
            order,
            [
                ("root-capture", EventPhase::Capturing),
                ("parent-throw", EventPhase::Capturing),
                ("target-capture", EventPhase::AtTarget),
                ("target-bubble", EventPhase::AtTarget),
                ("parent-bubble", EventPhase::Bubbling),
            ]
        );
        assert_eq!(errors, ["parent-throw"]);
        assert!(result.default_prevented());
        assert_eq!(result.phase(), EventPhase::None);
        assert_eq!(result.current_target(), None);
    }

    #[test]
    fn listener_mutation_once_and_propagation_flags_follow_dom_rules() {
        let registry = Rc::new(RefCell::new(EventRegistry::new()));
        let removed =
            registry
                .borrow_mut()
                .add("target", "x", Action::Record("removed"), false, false);
        registry.borrow_mut().add(
            "target",
            "x",
            Action::Remove("remove", removed),
            true,
            false,
        );
        registry
            .borrow_mut()
            .add("target", "x", Action::Add("add"), true, false);
        registry
            .borrow_mut()
            .add("target", "x", Action::Record("once"), false, true);
        registry
            .borrow_mut()
            .add("root", "x", Action::Stop("root-stop"), false, false);

        let run = |order: &mut Vec<&'static str>| {
            let registry_for_callback = Rc::clone(&registry);
            EventRegistry::dispatch(
                &registry,
                DomEvent::new("x", "target", true, false, Duration::ZERO),
                &["root", "target"],
                |action, event| {
                    match action {
                        Action::Record(name) => order.push(name),
                        Action::Remove(name, id) => {
                            order.push(name);
                            registry_for_callback.borrow_mut().remove(id);
                        }
                        Action::Add(name) => {
                            order.push(name);
                            registry_for_callback.borrow_mut().add(
                                "target",
                                "x",
                                Action::Record("added"),
                                false,
                                false,
                            );
                        }
                        Action::Stop(name) => {
                            order.push(name);
                            event.stop_propagation();
                        }
                        _ => unreachable!(),
                    }
                    Ok::<_, ()>(())
                },
                |_| {},
            )
        };

        let mut first = Vec::new();
        run(&mut first);
        assert_eq!(first, ["remove", "add", "once", "root-stop"]);
        let mut second = Vec::new();
        run(&mut second);
        assert_eq!(second, ["remove", "add", "added", "root-stop"]);
    }

    #[test]
    fn stop_immediate_skips_peers_and_ancestors() {
        let registry = Rc::new(RefCell::new(EventRegistry::new()));
        registry
            .borrow_mut()
            .add("target", "x", Action::StopImmediate("first"), false, false);
        registry
            .borrow_mut()
            .add("target", "x", Action::Record("peer"), false, false);
        registry
            .borrow_mut()
            .add("root", "x", Action::Record("ancestor"), false, false);
        let mut order = Vec::new();
        EventRegistry::dispatch(
            &registry,
            DomEvent::new("x", "target", true, false, Duration::ZERO),
            &["root", "target"],
            |action, event| {
                match action {
                    Action::StopImmediate(name) => {
                        order.push(name);
                        event.stop_immediate_propagation();
                    }
                    Action::Record(name) => order.push(name),
                    _ => unreachable!(),
                }
                Ok::<_, ()>(())
            },
            |_| {},
        );
        assert_eq!(order, ["first"]);
    }
}
