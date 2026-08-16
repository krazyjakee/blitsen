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

/// Whether this turn should pace a frame, or wait for something to happen.
///
/// The surface answers first, and can answer "no" on its own (issue #146). A
/// window whose surface the platform has taken away — Android backgrounding it,
/// iOS reclaiming it — cannot present anything, and winit will not dispatch a
/// `RedrawRequested` into it either. Left to the other two conditions the loop
/// would spin at 60 Hz for the whole time the application is in the background,
/// because `animation_frames_pending` stays true for precisely as long as the
/// callback that would clear the queue is the one not running: the condition
/// feeds itself. Asking the surface first also skips the script evaluation that
/// answering `animating` costs, which is why that one is a closure.
///
/// What this gives up: a `BLITSEN_STANDALONE_FRAMES` budget advances on turns
/// rather than on painted frames while the surface is gone. On the six desktop
/// targets that cannot arise at all — no desktop winit backend destroys a
/// surface — so it costs the acceptance runs nothing today, and a cadence
/// measured over frames that were never presented would be a fiction anyway.
pub fn paces_a_frame(
    surface_lost: bool,
    forcing_frames: bool,
    animating: impl FnOnce() -> bool,
) -> bool {
    !surface_lost && (forcing_frames || animating())
}

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

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::paces_a_frame;

    #[test]
    fn a_surface_that_is_gone_stops_the_frame_schedule_without_asking_javascript() {
        // The window is there: the usual two conditions decide.
        assert!(paces_a_frame(false, false, || true));
        assert!(paces_a_frame(false, true, || false));
        assert!(!paces_a_frame(false, false, || false));

        // The surface is gone: nothing can be presented, so nothing is paced —
        // including the acceptance-test frame budget, which would otherwise
        // count frames that were never painted.
        assert!(!paces_a_frame(true, false, || true));
        assert!(!paces_a_frame(true, true, || true));

        // And the engine is not asked, because asking costs a script evaluation
        // on every turn of a loop that is meant to have stopped.
        let asked = Cell::new(false);
        assert!(!paces_a_frame(true, false, || {
            asked.set(true);
            true
        }));
        assert!(!asked.get(), "a lost surface asked JavaScript anyway");
    }
}
