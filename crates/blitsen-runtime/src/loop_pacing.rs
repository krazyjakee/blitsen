//! Frame pacing for active windows, and the headless frame budget CI uses.
//!
//! The Phase 1 launcher paces itself with `Bun.sleep` against a 60 Hz schedule
//! that never drifts forward on a slow frame. This is the same schedule, and it
//! honours the same environment variables, so `test:standalone` measures the
//! two hosts the same way. A window without animation or an acceptance-test
//! frame budget blocks in winit instead, so this pacer creates no idle wakeups.

use std::time::{Duration, Instant};

/// One frame at 60 Hz.
const FRAME: Duration = Duration::from_nanos(16_666_667);

/// Keeps the loop to a frame schedule, and stops it when CI asked for a count.
pub struct Pacer {
    started: Instant,
    next_frame: Instant,
    frames: u32,
    limit: u32,
    warmup: u32,
}

impl Pacer {
    /// Reads the frame budget from the environment, as the Phase 1 loop does.
    ///
    /// `BLITSEN_STANDALONE_FRAMES` stops after that many frames and reports the
    /// cadence; `BLITSEN_STANDALONE_WARMUP_FRAMES` excludes the first frames
    /// from the measurement, because the first ones pay for surface creation.
    pub fn from_environment() -> Self {
        let count = |name: &str| {
            std::env::var(name)
                .ok()
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(0)
        };
        let now = Instant::now();
        Self {
            started: now,
            next_frame: now,
            frames: 0,
            limit: count("BLITSEN_STANDALONE_FRAMES"),
            warmup: count("BLITSEN_STANDALONE_WARMUP_FRAMES"),
        }
    }

    /// Counts the frame just pumped and reports whether the budget is spent.
    pub fn finished(&mut self) -> bool {
        self.frames += 1;
        if self.frames == self.warmup {
            self.started = Instant::now();
        }
        self.limit > 0 && self.frames >= self.limit + self.warmup
    }

    /// Whether an acceptance-test frame budget requires regular turns even
    /// when the application itself has no animation frame pending.
    pub fn forcing_frames(&self) -> bool {
        self.limit > 0
    }

    /// Sleeps until the next frame, or until a timer comes due first.
    ///
    /// A pending timer shortens the wait rather than lengthening the frame: a
    /// `setTimeout(f, 4)` has to run inside this frame, not after the next one.
    pub fn wait(&mut self, next_timer: Option<Duration>) {
        self.next_frame += FRAME;
        let now = Instant::now();
        // A frame that overran does not schedule a catch-up burst; the schedule
        // resets to now, exactly as the Phase 1 launcher does.
        if self.next_frame + FRAME < now {
            self.next_frame = now;
        }
        let until_frame = self.next_frame.saturating_duration_since(now);
        let wait = match next_timer {
            Some(timer) => until_frame.min(timer),
            None => until_frame,
        };
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
    }

    /// Prints the measured cadence when a frame count was requested.
    pub fn report(&self) {
        if self.limit == 0 {
            return;
        }
        let elapsed = self.started.elapsed().as_secs_f64().max(f64::MIN_POSITIVE);
        let cadence = f64::from(self.limit) / elapsed;
        println!(
            "Blitsen native frame check passed ({} frames at {cadence:.1} fps)",
            self.limit
        );
    }
}
