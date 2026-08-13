//! What an embedded engine does not bring with it (issue #87).
//!
//! Phase 1 runs inside Bun, which supplies timers, a microtask checkpoint, a
//! console, a clock and the Web IDL globals the DOM bootstrap throws. Phase 2
//! drops Bun's runtime entirely, so a bare JavaScriptCore context has an
//! ECMAScript heap and nothing else. This module is the difference, and it is
//! deliberately no larger than that difference: the compatibility policy in
//! `COMPATIBILITY.md` still applies, so nothing is added merely because some
//! other runtime has it.
//!
//! The timer queue lives here rather than in JavaScript because the outer loop
//! has to know when the next timer is due in order to sleep until it, and
//! because a macrotask boundary is where the engine's microtask checkpoint
//! belongs (TECH.md §6). I/O does not: `fetch`, `WebSocket` and audio decoding
//! already run on the shared tokio pool in `dom_bridge::worker` and rejoin the
//! main thread at one defined point in the frame, which is the same on both
//! hosts.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime};

use blitsen_js::timers::{TimerId, TimerQueue};
use blitsen_js::{JsEngine, JsError, JsType};

use crate::dom_bridge::argument;

const BOOTSTRAP: &str = include_str!("runtime_services/bootstrap.js");

/// Timers, clock, console and the Web IDL globals an embedded host must supply.
///
/// Install before the DOM bridge: the bridge's prelude captures `setTimeout`
/// and friends from the global object as it loads.
pub struct RuntimeServices<E: JsEngine> {
    timers: Rc<RefCell<TimerQueue<E::Value>>>,
    clock: Instant,
}

impl<E: JsEngine + 'static> RuntimeServices<E> {
    /// Installs the services into a context that has none of them.
    pub fn install(engine: &mut E) -> Result<Self, JsError> {
        let timers = Rc::new(RefCell::new(TimerQueue::new()));
        let clock = Instant::now();
        let origin = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64()
            * 1_000.0;

        engine.define_global_function(
            "__blitsenNow",
            Box::new(move |call| {
                let mut engine = E::from_value(&call.this);
                Ok(engine.number(clock.elapsed().as_secs_f64() * 1_000.0))
            }),
        )?;

        engine.define_global_function(
            "__blitsenTimeOrigin",
            Box::new(move |call| {
                let mut engine = E::from_value(&call.this);
                Ok(engine.number(origin))
            }),
        )?;

        engine.define_global_function(
            "__blitsenConsoleWrite",
            Box::new(move |call| {
                let mut engine = E::from_value(&call.this);
                let level = argument(&mut engine, &call, 0, "console level")?;
                let text = argument(&mut engine, &call, 1, "console text")?;
                // Diagnostics go to stderr; only `console.log` is program
                // output, which is the split a shell pipeline expects.
                if level == "log" {
                    println!("{text}");
                } else {
                    eprintln!("{text}");
                }
                Ok(call.this)
            }),
        )?;

        Self::install_timers(engine, &timers, clock)?;
        engine.evaluate_script(BOOTSTRAP, "blitsen:runtime-services")?;
        Ok(Self { timers, clock })
    }

    fn install_timers(
        engine: &mut E,
        timers: &Rc<RefCell<TimerQueue<E::Value>>>,
        clock: Instant,
    ) -> Result<(), JsError> {
        for (name, repeating) in [
            ("__blitsenSetTimeout", false),
            ("__blitsenSetInterval", true),
        ] {
            let queue = Rc::clone(timers);
            engine.define_global_function(
                name,
                Box::new(move |call| {
                    let mut engine = E::from_value(&call.this);
                    let callback = call.argument(0, "timer callback")?.clone();
                    if engine.value_type(&callback)? != JsType::Function {
                        return Err(JsError::new("timer callback must be a function"));
                    }
                    let delay = match call.arguments.get(1) {
                        Some(value) => engine.to_number(value)?,
                        None => 0.0,
                    };
                    let delay = Duration::from_secs_f64(delay.max(0.0) / 1_000.0);
                    let arguments = call.arguments.iter().skip(2).cloned().collect();
                    let now = clock.elapsed();
                    let mut queue = queue.borrow_mut();
                    let id = if repeating {
                        queue.set_interval(now, delay, callback, arguments)
                    } else {
                        queue.set_timeout(now, delay, callback, arguments)
                    };
                    Ok(engine.number(f64::from(id)))
                }),
            )?;
        }

        let queue = Rc::clone(timers);
        engine.define_global_function(
            "__blitsenClearTimer",
            Box::new(move |call| {
                let mut engine = E::from_value(&call.this);
                let id = engine.to_number(call.argument(0, "timer id")?)?;
                // A handle that was never issued is ignored, as the web
                // specifies; only a real identifier can cancel anything.
                if id.is_finite() && id >= 0.0 && id <= f64::from(TimerId::MAX) {
                    queue.borrow_mut().clear(id as TimerId);
                }
                Ok(call.this)
            }),
        )
    }

    /// Runs every timer due at the start of this turn, then drains microtasks.
    ///
    /// Returns how many callbacks ran. Timers armed *by* those callbacks are
    /// not run here: they are scheduled from the clock as it reads after the
    /// callback returned, which is strictly later than the deadline this turn
    /// selected on, so a `setTimeout(f, 0)` chain advances one turn at a time
    /// instead of starving the frame.
    ///
    /// An exception in a timer callback is reported and the remaining timers
    /// still run, because one broken callback must not stop the clock for the
    /// rest of the application.
    pub fn run_expired_timers(&self, engine: &mut E) -> Result<usize, JsError> {
        let turn = self.clock.elapsed();
        let mut ran = 0;
        loop {
            let Some(task) = self.timers.borrow_mut().begin_next_expired(turn) else {
                break;
            };
            let outcome = engine.call_macrotask(task.callback(), None, task.arguments());
            self.timers.borrow_mut().finish(self.clock.elapsed(), task);
            ran += 1;
            if let Err(error) = outcome {
                eprintln!("Uncaught exception in timer callback: {error}");
            }
        }
        Ok(ran)
    }

    /// How long until the earliest pending timer, or `None` when there is none.
    pub fn next_timer_delay(&self) -> Option<Duration> {
        let now = self.clock.elapsed();
        self.timers
            .borrow()
            .next_deadline()
            .map(|deadline| deadline.saturating_sub(now))
    }

    /// Milliseconds since the services were installed, on the same monotonic
    /// clock `performance.now()` reports.
    pub fn now_ms(&self) -> f64 {
        self.clock.elapsed().as_secs_f64() * 1_000.0
    }
}
