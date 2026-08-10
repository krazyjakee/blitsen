//! Deterministic scheduling for one presented application frame.

use std::time::{Duration, Instant};

/// A stage in the fixed frame turn order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameStage {
    /// Translate queued operating-system input into DOM events.
    Input,
    /// Apply completed off-thread work at the single async handoff point.
    AsyncResults,
    /// Run timers whose deadlines have expired.
    Timers,
    /// Drain promise jobs queued by the timer macrotasks.
    TimerMicrotasks,
    /// Invoke callbacks registered for the current animation frame.
    AnimationFrame,
    /// Drain promise jobs queued by animation callbacks.
    AnimationMicrotasks,
    /// Recompute styles for dirty nodes.
    Restyle,
    /// Resolve dirty layout subtrees.
    Layout,
    /// Build the web-content display list.
    Paint,
    /// Record native viewport contents into the same frame.
    NativeViewport,
    /// Submit rendering commands to the GPU.
    Submit,
    /// Present the completed surface.
    Present,
}

/// Optional wall-clock duration for a frame stage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StageTiming {
    /// Stage that was measured.
    pub stage: FrameStage,
    /// Time spent in the stage.
    pub duration: Duration,
}

/// Time values passed to one frame turn.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameTime {
    /// Monotonic time since application start.
    pub timestamp: Duration,
    /// Actual time since the preceding frame turn.
    pub delta: Duration,
}

/// Result of one frame turn.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameReport {
    /// Timing values supplied to application callbacks.
    pub time: FrameTime,
    /// Per-stage measurements, empty when instrumentation is disabled.
    pub stages: Vec<StageTiming>,
}

/// Host operations invoked in the fixed order for every presented frame.
///
/// Timer implementations must drain microtasks after each individual timer
/// macrotask. The pipeline's subsequent microtask stage also guarantees that
/// jobs queued outside a timer callback settle before animation callbacks.
pub trait FrameTurn {
    /// Error propagated out of the frame without running later stages.
    type Error;

    /// Drains and dispatches queued operating-system input.
    fn drain_input(&mut self) -> Result<(), Self::Error>;
    /// Drains the one queue through which off-thread work rejoins the runtime.
    fn drain_async_results(&mut self) -> Result<(), Self::Error>;
    /// Runs every timer expired at `time`, at most once in this turn.
    fn run_expired_timers(&mut self, time: FrameTime) -> Result<(), Self::Error>;
    /// Drains JavaScript microtasks to quiescence.
    fn drain_microtasks(&mut self) -> Result<(), Self::Error>;
    /// Runs the animation callbacks captured for this frame.
    fn run_animation_frames(&mut self, time: FrameTime) -> Result<(), Self::Error>;
    /// Restyles dirty nodes.
    fn restyle(&mut self) -> Result<(), Self::Error>;
    /// Resolves dirty layout.
    fn layout(&mut self) -> Result<(), Self::Error>;
    /// Builds the web-content display list.
    fn paint(&mut self) -> Result<(), Self::Error>;
    /// Records native viewport contents into the current frame.
    fn record_native_viewport(&mut self) -> Result<(), Self::Error>;
    /// Submits rendering commands.
    fn submit(&mut self) -> Result<(), Self::Error>;
    /// Presents the completed frame.
    fn present(&mut self) -> Result<(), Self::Error>;
}

/// Executes one and only one application turn for each host tick.
///
/// The scheduler never synthesizes catch-up turns after an overrun. Instead,
/// callbacks receive the actual elapsed [`FrameTime::delta`] and decide how to
/// advance application state.
#[derive(Debug)]
pub struct FramePipeline {
    started_at: Instant,
    previous_frame: Option<Instant>,
    instrumentation: bool,
}

impl FramePipeline {
    /// Starts a pipeline at the supplied monotonic instant.
    pub fn new(started_at: Instant) -> Self {
        Self {
            started_at,
            previous_frame: None,
            instrumentation: false,
        }
    }

    /// Enables or disables per-stage wall-clock measurements.
    pub fn set_instrumentation(&mut self, enabled: bool) {
        self.instrumentation = enabled;
    }

    /// Runs exactly one frame turn at `now`.
    pub fn run<T: FrameTurn>(
        &mut self,
        now: Instant,
        turn: &mut T,
    ) -> Result<FrameReport, T::Error> {
        let time = FrameTime {
            timestamp: now.saturating_duration_since(self.started_at),
            delta: now.saturating_duration_since(self.previous_frame.unwrap_or(self.started_at)),
        };
        self.previous_frame = Some(now);
        let mut stages = Vec::with_capacity(12);

        macro_rules! stage {
            ($name:expr, $operation:expr) => {{
                let started = Instant::now();
                $operation?;
                if self.instrumentation {
                    stages.push(StageTiming {
                        stage: $name,
                        duration: started.elapsed(),
                    });
                }
            }};
        }

        stage!(FrameStage::Input, turn.drain_input());
        stage!(FrameStage::AsyncResults, turn.drain_async_results());
        stage!(FrameStage::Timers, turn.run_expired_timers(time));
        stage!(FrameStage::TimerMicrotasks, turn.drain_microtasks());
        stage!(FrameStage::AnimationFrame, turn.run_animation_frames(time));
        stage!(FrameStage::AnimationMicrotasks, turn.drain_microtasks());
        stage!(FrameStage::Restyle, turn.restyle());
        stage!(FrameStage::Layout, turn.layout());
        stage!(FrameStage::Paint, turn.paint());
        stage!(FrameStage::NativeViewport, turn.record_native_viewport());
        stage!(FrameStage::Submit, turn.submit());
        stage!(FrameStage::Present, turn.present());

        Ok(FrameReport { time, stages })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct RecordingTurn {
        stages: Vec<FrameStage>,
        times: Vec<FrameTime>,
    }

    impl FrameTurn for RecordingTurn {
        type Error = ();

        fn drain_input(&mut self) -> Result<(), Self::Error> {
            self.stages.push(FrameStage::Input);
            Ok(())
        }
        fn drain_async_results(&mut self) -> Result<(), Self::Error> {
            self.stages.push(FrameStage::AsyncResults);
            Ok(())
        }
        fn run_expired_timers(&mut self, time: FrameTime) -> Result<(), Self::Error> {
            self.stages.push(FrameStage::Timers);
            self.times.push(time);
            Ok(())
        }
        fn drain_microtasks(&mut self) -> Result<(), Self::Error> {
            self.stages
                .push(if self.stages.contains(&FrameStage::AnimationFrame) {
                    FrameStage::AnimationMicrotasks
                } else {
                    FrameStage::TimerMicrotasks
                });
            Ok(())
        }
        fn run_animation_frames(&mut self, time: FrameTime) -> Result<(), Self::Error> {
            self.stages.push(FrameStage::AnimationFrame);
            self.times.push(time);
            Ok(())
        }
        fn restyle(&mut self) -> Result<(), Self::Error> {
            self.stages.push(FrameStage::Restyle);
            Ok(())
        }
        fn layout(&mut self) -> Result<(), Self::Error> {
            self.stages.push(FrameStage::Layout);
            Ok(())
        }
        fn paint(&mut self) -> Result<(), Self::Error> {
            self.stages.push(FrameStage::Paint);
            Ok(())
        }
        fn record_native_viewport(&mut self) -> Result<(), Self::Error> {
            self.stages.push(FrameStage::NativeViewport);
            Ok(())
        }
        fn submit(&mut self) -> Result<(), Self::Error> {
            self.stages.push(FrameStage::Submit);
            Ok(())
        }
        fn present(&mut self) -> Result<(), Self::Error> {
            self.stages.push(FrameStage::Present);
            Ok(())
        }
    }

    #[test]
    fn every_turn_has_one_fixed_stage_order() {
        let started = Instant::now();
        let mut pipeline = FramePipeline::new(started);
        let mut turn = RecordingTurn::default();
        pipeline
            .run(started + Duration::from_millis(16), &mut turn)
            .unwrap();
        assert_eq!(
            turn.stages,
            [
                FrameStage::Input,
                FrameStage::AsyncResults,
                FrameStage::Timers,
                FrameStage::TimerMicrotasks,
                FrameStage::AnimationFrame,
                FrameStage::AnimationMicrotasks,
                FrameStage::Restyle,
                FrameStage::Layout,
                FrameStage::Paint,
                FrameStage::NativeViewport,
                FrameStage::Submit,
                FrameStage::Present,
            ]
        );
    }

    #[test]
    fn overruns_report_honest_delta_without_catch_up_turns() {
        let started = Instant::now();
        let mut pipeline = FramePipeline::new(started);
        let mut turn = RecordingTurn::default();
        pipeline
            .run(started + Duration::from_millis(16), &mut turn)
            .unwrap();
        pipeline
            .run(started + Duration::from_millis(70), &mut turn)
            .unwrap();
        assert_eq!(turn.times.len(), 4);
        assert_eq!(turn.times[0].delta, Duration::from_millis(16));
        assert_eq!(turn.times[2].delta, Duration::from_millis(54));
        assert_eq!(turn.times[2].timestamp, Duration::from_millis(70));
    }

    #[test]
    fn timing_instrumentation_is_opt_in() {
        let started = Instant::now();
        let mut pipeline = FramePipeline::new(started);
        let mut turn = RecordingTurn::default();
        assert!(pipeline.run(started, &mut turn).unwrap().stages.is_empty());
        pipeline.set_instrumentation(true);
        assert_eq!(pipeline.run(started, &mut turn).unwrap().stages.len(), 12);
    }
}
