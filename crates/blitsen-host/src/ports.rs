//! Entangled message ports, and the routing table between JavaScript contexts.
//!
//! One registry for the process, because a port is not owned by the context that
//! created it: `postMessage` may hand a port to a worker, and from then on the
//! messages queued for it must be delivered on that worker's thread. A per
//! context table could not answer "who owns this port now", so there is one
//! table and each port records its current owner.
//!
//! # Why the queue hangs off the port rather than off the context
//!
//! Transfer is the reason. A port that is transferred takes its undelivered
//! messages with it — that is what the specification's "port message queue"
//! means, and it is the difference between a message sent just before a port was
//! handed to a worker arriving there and being lost. A per-context mailbox would
//! have left those messages behind in the sender.
//!
//! # Delivery
//!
//! Nothing here calls JavaScript. A context drains what it owns at a point of
//! its own choosing — the document does it at the start of the animation-frame
//! stage, exactly where `fetch` completions and socket frames land, and a worker
//! does it at the top of its own turn. That keeps the contract every other
//! off-thread source in this runtime already keeps: a message cannot arrive
//! part-way through a callback.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

/// One JavaScript context that can own ports: the document, or a worker.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
pub struct ContextId(pub u64);

/// One end of an entangled pair.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
pub struct PortId(pub u64);

/// A structured-clone message, in the shape that crosses a thread boundary.
///
/// The value itself is already flattened by the bootstrap's codec: `data` is its
/// JSON record graph and `buffers` the binary payloads that graph refers to by
/// index. Nothing here inspects either, which is what lets a message move
/// between two engines that share no value representation.
#[derive(Debug, Default)]
pub struct Envelope {
    /// The encoded record graph.
    pub data: String,
    /// Binary payloads, referenced positionally from `data`.
    pub buffers: Vec<Vec<u8>>,
    /// Ports transferred with the message, adopted by the receiving context.
    pub ports: Vec<PortId>,
}

/// What a drained port hands its owner.
#[derive(Debug)]
pub enum Delivery {
    /// An ordinary message.
    Message(Envelope),
    /// The worker behind this port threw where nothing could catch it.
    Error(String),
    /// The context on the other end has gone.
    Closed,
}

/// Parks a thread until it has something to do.
///
/// The document does not use one — its loop is the frame loop, which turns for
/// other reasons — but a worker thread with no timers pending would otherwise
/// spin. `wake` is called by whichever thread queued the work.
#[derive(Default)]
pub struct Waker {
    signalled: Mutex<bool>,
    condvar: Condvar,
}

impl Waker {
    /// Marks work as available and releases a parked thread.
    pub fn wake(&self) {
        *lock(&self.signalled) = true;
        self.condvar.notify_all();
    }

    /// Parks until woken or until `timeout` elapses, clearing the signal.
    ///
    /// A wake that arrives before the wait starts is not lost: the flag is
    /// checked first, which is why it is a flag rather than a bare condvar.
    pub fn wait(&self, timeout: Option<Duration>) {
        let mut signalled = lock(&self.signalled);
        if std::mem::take(&mut *signalled) {
            return;
        }
        let mut signalled = match timeout {
            Some(timeout) => self
                .condvar
                .wait_timeout(signalled, timeout)
                .map(|(guard, _)| guard)
                .unwrap_or_else(|error| error.into_inner().0),
            None => self
                .condvar
                .wait(signalled)
                .unwrap_or_else(PoisonError::into_inner),
        };
        *signalled = false;
    }
}

/// Locks without propagating poisoning, for the same reason the network pool
/// does: a panicked context must not stop every other context messaging.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Port {
    /// The other end, until one of them is closed.
    peer: Option<PortId>,
    /// The context that drains this port, or `None` once it is closed.
    owner: Option<ContextId>,
    /// Messages waiting for the owner. Held on the port, so a transfer moves
    /// them with it.
    queue: VecDeque<Delivery>,
    /// Whether the owner has started delivery. A port that has never been
    /// started buffers rather than dropping, which is what lets an application
    /// set `onmessage` a turn after it was handed the port.
    started: bool,
}

/// Every live port in the process, and the contexts waiting on them.
#[derive(Default)]
pub struct PortRegistry {
    ports: Mutex<HashMap<PortId, Port>>,
    wakers: Mutex<HashMap<ContextId, Arc<Waker>>>,
    next_port: AtomicU64,
    next_context: AtomicU64,
}

/// The registry every context in this process routes through.
pub fn registry() -> &'static PortRegistry {
    static REGISTRY: OnceLock<PortRegistry> = OnceLock::new();
    REGISTRY.get_or_init(PortRegistry::default)
}

impl PortRegistry {
    /// Issues an identifier for a context that will own ports.
    pub fn new_context(&self) -> ContextId {
        ContextId(self.next_context.fetch_add(1, Ordering::Relaxed) + 1)
    }

    /// Registers how to wake `context` when one of its ports is written to.
    pub fn attach_waker(&self, context: ContextId, waker: Arc<Waker>) {
        lock(&self.wakers).insert(context, waker);
    }

    /// Creates an entangled pair, owned by the contexts named.
    ///
    /// `MessageChannel` names the same context twice; a worker names the
    /// document and the worker, and the worker's end is owned before its thread
    /// exists so that a message posted from the constructor's own turn is
    /// already queued when the worker starts draining.
    pub fn entangle(&self, first: ContextId, second: ContextId) -> (PortId, PortId) {
        let a = PortId(self.next_port.fetch_add(1, Ordering::Relaxed) + 1);
        let b = PortId(self.next_port.fetch_add(1, Ordering::Relaxed) + 1);
        let mut ports = lock(&self.ports);
        ports.insert(
            a,
            Port {
                peer: Some(b),
                owner: Some(first),
                queue: VecDeque::new(),
                started: false,
            },
        );
        ports.insert(
            b,
            Port {
                peer: Some(a),
                owner: Some(second),
                queue: VecDeque::new(),
                started: false,
            },
        );
        (a, b)
    }

    /// Queues a delivery for the port entangled with `from`.
    ///
    /// A message sent into a closed or disowned pair is discarded rather than
    /// refused, which is what the specification says and what an application
    /// that posts to a worker it has already terminated relies on.
    pub fn post(&self, from: PortId, delivery: Delivery) {
        let mut ports = lock(&self.ports);
        let Some(peer) = ports.get(&from).and_then(|port| port.peer) else {
            return;
        };
        // A transferred port is adopted by the receiving context before the
        // message carrying it is queued, so the port is drainable the moment the
        // message is delivered rather than a turn later. It arrives stopped
        // however it left: the enabled flag is not part of what is transferred,
        // so the receiver's `onmessage` decides when its queue starts moving,
        // and a message sent before the handover waits for that rather than
        // being dispatched at a port nobody is listening to yet.
        if let (Delivery::Message(envelope), Some(owner)) =
            (&delivery, ports.get(&peer).and_then(|port| port.owner))
        {
            let moved = envelope.ports.clone();
            for port in moved {
                if let Some(state) = ports.get_mut(&port) {
                    state.owner = Some(owner);
                    state.started = false;
                }
            }
        }
        let Some(port) = ports.get_mut(&peer) else {
            return;
        };
        port.queue.push_back(delivery);
        let owner = port.owner;
        drop(ports);
        if let Some(owner) = owner {
            self.wake(owner);
        }
    }

    /// Starts delivery on a port, as `start()` and setting `onmessage` do.
    pub fn start(&self, port: PortId) {
        if let Some(state) = lock(&self.ports).get_mut(&port) {
            state.started = true;
        }
    }

    /// Takes everything queued for the started ports `context` owns.
    ///
    /// Returned in the order the ports were written to as far as each port is
    /// concerned; across ports the order is the map's, which the specification
    /// leaves to the implementation.
    pub fn drain(&self, context: ContextId) -> Vec<(PortId, Delivery)> {
        let mut ports = lock(&self.ports);
        let mut drained = Vec::new();
        for (id, port) in ports.iter_mut() {
            if port.owner != Some(context) || !port.started {
                continue;
            }
            while let Some(delivery) = port.queue.pop_front() {
                drained.push((*id, delivery));
            }
        }
        drained
    }

    /// Whether `context` has anything waiting on a started port.
    ///
    /// The document asks this to decide whether the frame loop is owed another
    /// turn, exactly as it asks whether a socket is still open.
    pub fn pending(&self, context: ContextId) -> bool {
        lock(&self.ports)
            .values()
            .any(|port| port.owner == Some(context) && port.started && !port.queue.is_empty())
    }

    /// Detaches one end, telling the other that nothing more is coming.
    pub fn close(&self, port: PortId) {
        let mut ports = lock(&self.ports);
        let Some(state) = ports.remove(&port) else {
            return;
        };
        let Some(peer) = state.peer else {
            return;
        };
        let Some(other) = ports.get_mut(&peer) else {
            return;
        };
        other.peer = None;
        other.queue.push_back(Delivery::Closed);
        let owner = other.owner;
        drop(ports);
        if let Some(owner) = owner {
            self.wake(owner);
        }
    }

    /// Closes every port a context owns, as its teardown does.
    pub fn release(&self, context: ContextId) {
        let owned = lock(&self.ports)
            .iter()
            .filter(|(_, port)| port.owner == Some(context))
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for port in owned {
            self.close(port);
        }
        lock(&self.wakers).remove(&context);
    }

    fn wake(&self, context: ContextId) {
        let waker = lock(&self.wakers).get(&context).map(Arc::clone);
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(data: &str) -> Delivery {
        Delivery::Message(Envelope {
            data: data.to_owned(),
            ..Envelope::default()
        })
    }

    fn messages(drained: Vec<(PortId, Delivery)>) -> Vec<String> {
        drained
            .into_iter()
            .filter_map(|(_, delivery)| match delivery {
                Delivery::Message(envelope) => Some(envelope.data),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_message_is_queued_for_the_other_end_and_drained_once() {
        let registry = PortRegistry::default();
        let one = registry.new_context();
        let two = registry.new_context();
        let (a, b) = registry.entangle(one, two);

        registry.post(a, envelope("first"));
        assert!(
            !registry.pending(two),
            "an unstarted port buffers rather than delivering"
        );
        assert!(registry.drain(two).is_empty());

        registry.start(b);
        assert!(registry.pending(two));
        assert_eq!(messages(registry.drain(two)), ["first"]);
        assert!(
            registry.drain(two).is_empty(),
            "a drained message is handed over once"
        );
        assert!(!registry.pending(two));
        assert!(
            registry.drain(one).is_empty(),
            "a message goes to the peer, never back to the sender"
        );
    }

    #[test]
    fn a_transferred_port_takes_its_undelivered_messages_with_it() {
        let registry = PortRegistry::default();
        let document = registry.new_context();
        let worker = registry.new_context();
        let (main_side, worker_side) = registry.entangle(document, worker);
        let (channel_a, channel_b) = registry.entangle(document, document);
        registry.start(channel_a);
        registry.start(channel_b);

        // Posted while both ends are still the document's, then the receiving
        // end is transferred to the worker before it was ever drained.
        registry.post(channel_a, envelope("queued before transfer"));
        registry.start(worker_side);
        registry.post(
            main_side,
            Delivery::Message(Envelope {
                data: "take this port".to_owned(),
                buffers: Vec::new(),
                ports: vec![channel_b],
            }),
        );

        assert_eq!(
            messages(registry.drain(worker)),
            ["take this port"],
            "the transferred port arrives stopped, so only the message carrying it is delivered"
        );
        assert!(
            registry.drain(document).is_empty(),
            "the document no longer owns the port it gave away"
        );
        // Which is what the receiving context's `onmessage` does.
        registry.start(channel_b);
        assert_eq!(
            messages(registry.drain(worker)),
            ["queued before transfer"],
            "the message queued before the transfer arrives where the port went"
        );
    }

    #[test]
    fn closing_one_end_tells_the_other_and_discards_what_follows() {
        let registry = PortRegistry::default();
        let one = registry.new_context();
        let two = registry.new_context();
        let (a, b) = registry.entangle(one, two);
        registry.start(b);

        registry.close(a);
        assert!(matches!(
            registry.drain(two).as_slice(),
            [(_, Delivery::Closed)]
        ));
        // Nothing panics and nothing is queued: the pair is gone.
        registry.post(a, envelope("into the void"));
        registry.post(b, envelope("also nowhere"));
        assert!(registry.drain(two).is_empty());
        assert!(registry.drain(one).is_empty());
    }

    #[test]
    fn releasing_a_context_closes_every_port_it_owned() {
        let registry = PortRegistry::default();
        let document = registry.new_context();
        let worker = registry.new_context();
        let (main_side, worker_side) = registry.entangle(document, worker);
        registry.start(main_side);
        registry.start(worker_side);

        registry.release(worker);
        assert!(matches!(
            registry.drain(document).as_slice(),
            [(_, Delivery::Closed)]
        ));
        registry.post(main_side, envelope("after the worker went"));
        assert!(registry.drain(document).is_empty());
    }

    #[test]
    fn a_waker_that_was_signalled_before_the_wait_does_not_park() {
        let registry = PortRegistry::default();
        let context = registry.new_context();
        let waker = Arc::new(Waker::default());
        registry.attach_waker(context, Arc::clone(&waker));
        let (a, b) = registry.entangle(registry.new_context(), context);
        registry.start(b);

        registry.post(a, envelope("wake up"));
        // Would block for the full timeout if the signal had been missed.
        let started = std::time::Instant::now();
        waker.wait(Some(Duration::from_secs(30)));
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
