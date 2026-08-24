//! Reusable single-thread queues for native bridge command channels.
//!
//! JavaScript and the native window run on the same thread, but they cannot
//! borrow the window session at the same time. A channel records commands for
//! the session to apply after the JavaScript turn and records completions or
//! native events for a later frame. The message vocabulary stays with each
//! capability; only IDs, FIFO storage, draining and reset live here.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

/// One command assigned the channel's monotonically increasing ID.
pub(crate) struct CommandRequest<K> {
    pub(crate) command_id: u64,
    pub(crate) kind: K,
}

/// A FIFO that can be used beside a command channel for an independent stream.
///
/// Gamepad connection changes use this to preserve their existing
/// connections-before-vibration-completions drain order while sharing all
/// queue mechanics with the other bridge capabilities.
pub(crate) struct Queue<T> {
    values: RefCell<VecDeque<T>>,
}

impl<T> Queue<T> {
    pub(crate) const fn new() -> Self {
        Self {
            values: RefCell::new(VecDeque::new()),
        }
    }

    pub(crate) fn push(&self, value: T) {
        self.values.borrow_mut().push_back(value);
    }

    pub(crate) fn extend(&self, values: impl IntoIterator<Item = T>) {
        self.values.borrow_mut().extend(values);
    }

    pub(crate) fn pending(&self) -> bool {
        !self.values.borrow().is_empty()
    }

    pub(crate) fn take(&self) -> Vec<T> {
        self.values.borrow_mut().drain(..).collect()
    }

    pub(crate) fn clear(&self) {
        self.values.borrow_mut().clear();
    }
}

/// The request and message queues common to a native bridge capability.
pub(crate) struct CommandChannel<K, M> {
    next_command_id: Cell<u64>,
    requests: Queue<CommandRequest<K>>,
    messages: Queue<M>,
}

impl<K, M> CommandChannel<K, M> {
    pub(crate) const fn new() -> Self {
        Self {
            next_command_id: Cell::new(1),
            requests: Queue::new(),
            messages: Queue::new(),
        }
    }

    pub(crate) fn request(&self, kind: K) -> u64 {
        let command_id = self.next_command_id.get();
        self.next_command_id.set(command_id.saturating_add(1));
        self.requests.push(CommandRequest { command_id, kind });
        command_id
    }

    pub(crate) fn take_requests(&self) -> Vec<CommandRequest<K>> {
        self.requests.take()
    }

    pub(crate) fn push(&self, message: M) {
        self.messages.push(message);
    }

    pub(crate) fn pending(&self) -> bool {
        self.messages.pending()
    }

    pub(crate) fn take_messages(&self) -> Vec<M> {
        self.messages.take()
    }

    pub(crate) fn reset(&self) {
        self.next_command_id.set(1);
        self.requests.clear();
        self.messages.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_keep_ids_fifo_and_reset_independent() {
        let first = CommandChannel::<&str, &str>::new();
        let second = CommandChannel::<&str, &str>::new();

        assert_eq!(first.request("one"), 1);
        assert_eq!(first.request("two"), 2);
        assert_eq!(second.request("other"), 1);
        first.push("complete one");
        first.push("event");
        second.push("complete other");

        let requests = first.take_requests();
        assert_eq!(
            requests
                .iter()
                .map(|request| (request.command_id, request.kind))
                .collect::<Vec<_>>(),
            [(1, "one"), (2, "two")]
        );
        assert!(first.take_requests().is_empty());
        assert_eq!(first.take_messages(), ["complete one", "event"]);
        assert!(!first.pending());

        first.request("discarded");
        first.push("discarded");
        first.reset();
        assert_eq!(first.request("fresh"), 1);
        assert!(first.take_messages().is_empty());

        assert_eq!(second.take_messages(), ["complete other"]);
        let other = second.take_requests();
        assert_eq!((other[0].command_id, other[0].kind), (1, "other"));
    }
}
