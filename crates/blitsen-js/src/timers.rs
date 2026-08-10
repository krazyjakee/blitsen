//! Runtime-neutral storage and deadline ordering for web timers.

use std::collections::HashMap;
use std::time::Duration;

/// Numeric handle returned by `setTimeout` and `setInterval`.
pub type TimerId = u32;

/// One timer macrotask removed from the queue for invocation by a host.
pub struct TimerTask<V> {
    id: TimerId,
    callback: V,
    arguments: Vec<V>,
    interval: Option<Duration>,
    nesting: u8,
}

impl<V> TimerTask<V> {
    /// Timer's stable public identifier.
    pub fn id(&self) -> TimerId {
        self.id
    }

    /// Callable JavaScript value.
    pub fn callback(&self) -> &V {
        &self.callback
    }

    /// Arguments forwarded unchanged to the callback.
    pub fn arguments(&self) -> &[V] {
        &self.arguments
    }
}

struct QueuedTimer<V> {
    deadline: Duration,
    sequence: u64,
    task: TimerTask<V>,
}

/// Ordered timers shared by every JavaScript-engine implementation.
///
/// Delays are clamped to four milliseconds after five nested timer tasks,
/// matching browser behavior. Repeating timers are scheduled from the turn in
/// which their callback actually ran, so an overrun never creates a catch-up
/// burst.
pub struct TimerQueue<V> {
    timers: HashMap<TimerId, QueuedTimer<V>>,
    next_id: TimerId,
    next_sequence: u64,
    active: Option<(TimerId, u8, bool)>,
}

impl<V> Default for TimerQueue<V> {
    fn default() -> Self {
        Self {
            timers: HashMap::new(),
            next_id: 1,
            next_sequence: 0,
            active: None,
        }
    }
}

impl<V> TimerQueue<V> {
    /// Creates an empty timer queue.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a one-shot timer.
    pub fn set_timeout(
        &mut self,
        now: Duration,
        delay: Duration,
        callback: V,
        arguments: Vec<V>,
    ) -> TimerId {
        self.schedule(now, delay, None, callback, arguments)
    }

    /// Registers a repeating timer.
    pub fn set_interval(
        &mut self,
        now: Duration,
        delay: Duration,
        callback: V,
        arguments: Vec<V>,
    ) -> TimerId {
        self.schedule(now, delay, Some(delay), callback, arguments)
    }

    /// Cancels a timeout or interval. Unknown identifiers are ignored.
    pub fn clear(&mut self, id: TimerId) {
        self.timers.remove(&id);
        if let Some((active_id, _, cancelled)) = &mut self.active
            && *active_id == id
        {
            *cancelled = true;
        }
    }

    /// Returns the earliest pending deadline.
    pub fn next_deadline(&self) -> Option<Duration> {
        self.timers.values().map(|timer| timer.deadline).min()
    }

    /// Removes the next expired timer in deadline and registration order.
    pub fn begin_next_expired(&mut self, now: Duration) -> Option<TimerTask<V>> {
        let id = self
            .timers
            .iter()
            .filter(|(_, timer)| timer.deadline <= now)
            .min_by_key(|(_, timer)| (timer.deadline, timer.sequence))
            .map(|(id, _)| *id)?;
        let queued = self.timers.remove(&id).expect("selected timer exists");
        self.active = Some((id, queued.task.nesting, false));
        Some(queued.task)
    }

    /// Completes a timer macrotask and rearms a non-cancelled interval.
    pub fn finish(&mut self, now: Duration, task: TimerTask<V>) {
        let active = self.active.take();
        let cancelled = active.is_some_and(|(id, _, cancelled)| id == task.id && cancelled);
        if cancelled {
            return;
        }
        let Some(interval) = task.interval else {
            return;
        };
        let nesting = active.map_or(task.nesting, |(_, nesting, _)| nesting);
        let delay = if nesting >= 5 {
            interval.max(Duration::from_millis(4))
        } else {
            interval
        };
        self.insert(now.saturating_add(delay), task);
    }

    fn schedule(
        &mut self,
        now: Duration,
        delay: Duration,
        interval: Option<Duration>,
        callback: V,
        arguments: Vec<V>,
    ) -> TimerId {
        let id = self.allocate_id();
        let nesting = self.active_nesting().saturating_add(1);
        let delay = if self.active_nesting() >= 5 {
            delay.max(Duration::from_millis(4))
        } else {
            delay
        };
        self.insert(
            now.saturating_add(delay),
            TimerTask {
                id,
                callback,
                arguments,
                interval,
                nesting,
            },
        );
        id
    }

    fn insert(&mut self, deadline: Duration, task: TimerTask<V>) {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.timers.insert(
            task.id,
            QueuedTimer {
                deadline,
                sequence,
                task,
            },
        );
    }

    fn allocate_id(&mut self) -> TimerId {
        loop {
            let id = self.next_id;
            self.next_id = self.next_id.wrapping_add(1).max(1);
            if !self.timers.contains_key(&id)
                && self.active.is_none_or(|(active, _, _)| active != id)
            {
                return id;
            }
        }
    }

    fn active_nesting(&self) -> u8 {
        self.active.map_or(0, |(_, nesting, _)| nesting)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timers_are_ordered_and_arguments_are_preserved() {
        let mut queue = TimerQueue::new();
        let late = queue.set_timeout(Duration::ZERO, Duration::from_millis(20), "late", vec!["2"]);
        let early = queue.set_timeout(
            Duration::ZERO,
            Duration::from_millis(10),
            "early",
            vec!["1"],
        );
        assert_eq!(
            queue
                .begin_next_expired(Duration::from_millis(9))
                .map(|task| task.id()),
            None
        );
        let task = queue.begin_next_expired(Duration::from_millis(20)).unwrap();
        assert_eq!(task.id(), early);
        assert_eq!(task.callback(), &"early");
        assert_eq!(task.arguments(), &["1"]);
        queue.finish(Duration::from_millis(20), task);
        assert_eq!(
            queue
                .begin_next_expired(Duration::from_millis(20))
                .unwrap()
                .id(),
            late
        );
    }

    #[test]
    fn intervals_do_not_catch_up_and_can_cancel_themselves() {
        let mut queue = TimerQueue::new();
        let id = queue.set_interval(Duration::ZERO, Duration::from_millis(10), (), vec![]);
        let task = queue
            .begin_next_expired(Duration::from_millis(100))
            .unwrap();
        queue.finish(Duration::from_millis(100), task);
        assert_eq!(queue.next_deadline(), Some(Duration::from_millis(110)));
        let task = queue
            .begin_next_expired(Duration::from_millis(110))
            .unwrap();
        queue.clear(id);
        queue.finish(Duration::from_millis(110), task);
        assert_eq!(queue.next_deadline(), None);
    }

    #[test]
    fn deeply_nested_timers_use_the_browser_four_millisecond_clamp() {
        let mut queue = TimerQueue::new();
        let mut task = {
            queue.set_timeout(Duration::ZERO, Duration::ZERO, (), vec![]);
            queue.begin_next_expired(Duration::ZERO).unwrap()
        };
        for _ in 0..4 {
            queue.set_timeout(Duration::ZERO, Duration::ZERO, (), vec![]);
            queue.finish(Duration::ZERO, task);
            task = queue.begin_next_expired(Duration::ZERO).unwrap();
        }
        queue.set_timeout(Duration::ZERO, Duration::ZERO, (), vec![]);
        queue.finish(Duration::ZERO, task);
        assert_eq!(queue.next_deadline(), Some(Duration::from_millis(4)));
    }
}
