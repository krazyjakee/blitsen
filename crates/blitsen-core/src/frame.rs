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

/// Optional measurements for a frame stage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StageTiming {
    /// Stage that was measured.
    pub stage: FrameStage,
    /// Time spent in the stage.
    pub duration: Duration,
    /// Heap allocations made during the stage, zero without a counter.
    pub allocations: u64,
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

/// Upper bucket edges in milliseconds. The 16.7 ms edge is the 60 Hz budget, so
/// the count above it is the number of frames a player would have felt.
pub const FRAME_BUCKET_EDGES_MS: [f64; 8] = [1.0, 2.0, 4.0, 8.0, 16.7, 25.0, 33.4, 50.0];

/// Distribution of measured frame durations.
///
/// An average hides the tail, and the tail is what a player feels, so the
/// percentiles and the over-budget bucket counts are reported alongside it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameHistogram {
    /// Number of measured frames.
    pub count: usize,
    /// Sum of every measured frame.
    pub total: Duration,
    /// Fastest measured frame.
    pub min: Duration,
    /// Median frame.
    pub p50: Duration,
    /// 95th percentile frame.
    pub p95: Duration,
    /// 99th percentile frame.
    pub p99: Duration,
    /// Slowest measured frame.
    pub max: Duration,
    /// Frame counts per bucket, one more entry than [`FRAME_BUCKET_EDGES_MS`];
    /// the last holds everything above the final edge.
    pub buckets: [usize; FRAME_BUCKET_EDGES_MS.len() + 1],
    /// Frames that took longer than one 60 Hz frame budget.
    pub over_budget: usize,
}

/// One 60 Hz frame budget.
pub const FRAME_BUDGET: Duration = Duration::from_nanos(16_666_667);

impl FrameHistogram {
    /// Summarizes measured frame durations. Returns `None` for no frames.
    pub fn from_durations(durations: &[Duration]) -> Option<Self> {
        if durations.is_empty() {
            return None;
        }
        let mut sorted = durations.to_vec();
        sorted.sort_unstable();
        let mut buckets = [0; FRAME_BUCKET_EDGES_MS.len() + 1];
        for duration in &sorted {
            let milliseconds = duration.as_secs_f64() * 1_000.0;
            let bucket = FRAME_BUCKET_EDGES_MS
                .iter()
                .position(|edge| milliseconds <= *edge)
                .unwrap_or(FRAME_BUCKET_EDGES_MS.len());
            buckets[bucket] += 1;
        }
        Some(Self {
            count: sorted.len(),
            total: sorted.iter().sum(),
            min: sorted[0],
            p50: percentile(&sorted, 0.50),
            p95: percentile(&sorted, 0.95),
            p99: percentile(&sorted, 0.99),
            max: sorted[sorted.len() - 1],
            buckets,
            over_budget: sorted
                .iter()
                .filter(|duration| **duration > FRAME_BUDGET)
                .count(),
        })
    }

    /// Mean frame duration.
    pub fn mean(&self) -> Duration {
        self.total / self.count as u32
    }
}

/// Nearest-rank percentile of an ascending slice.
fn percentile(sorted: &[Duration], fraction: f64) -> Duration {
    let rank = (fraction * sorted.len() as f64).ceil() as usize;
    sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
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
    allocation_counter: Option<fn() -> u64>,
}

impl FramePipeline {
    /// Starts a pipeline at the supplied monotonic instant.
    pub fn new(started_at: Instant) -> Self {
        Self {
            started_at,
            previous_frame: None,
            instrumentation: false,
            allocation_counter: None,
        }
    }

    /// Enables or disables per-stage wall-clock measurements.
    pub fn set_instrumentation(&mut self, enabled: bool) {
        self.instrumentation = enabled;
    }

    /// Attributes heap allocations to stages, given a monotonic total allocation
    /// count — a counting global allocator, for example. Without one, a stage's
    /// allocation count stays zero.
    pub fn set_allocation_counter(&mut self, counter: Option<fn() -> u64>) {
        self.allocation_counter = counter;
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
        // Uninstrumented frames must not allocate: this runs every frame.
        let mut stages = if self.instrumentation {
            Vec::with_capacity(12)
        } else {
            Vec::new()
        };

        macro_rules! stage {
            ($name:expr, $operation:expr) => {{
                let allocated = self.allocation_counter.map_or(0, |counter| counter());
                let started = Instant::now();
                $operation?;
                if self.instrumentation {
                    stages.push(StageTiming {
                        stage: $name,
                        duration: started.elapsed(),
                        allocations: self
                            .allocation_counter
                            .map_or(0, |counter| counter().saturating_sub(allocated)),
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
    fn histograms_report_the_tail_not_only_the_average() {
        let durations: Vec<_> = (1..=100)
            .map(|frame| {
                // Ninety-nine frames well inside budget and one 40 ms stall: the
                // mean stays under budget while p99 and max do not.
                if frame == 100 {
                    Duration::from_millis(40)
                } else {
                    Duration::from_micros(3_000)
                }
            })
            .collect();
        let histogram = FrameHistogram::from_durations(&durations).unwrap();
        assert_eq!(histogram.count, 100);
        assert_eq!(histogram.min, Duration::from_micros(3_000));
        assert_eq!(histogram.p50, Duration::from_micros(3_000));
        assert_eq!(histogram.p95, Duration::from_micros(3_000));
        assert_eq!(histogram.p99, Duration::from_micros(3_000));
        assert_eq!(histogram.max, Duration::from_millis(40));
        assert!(histogram.mean() < FRAME_BUDGET);
        assert_eq!(histogram.over_budget, 1);
        assert_eq!(histogram.buckets[2], 99);
        assert_eq!(histogram.buckets[7], 1);
        assert_eq!(FrameHistogram::from_durations(&[]), None);
    }

    #[test]
    fn timing_instrumentation_is_opt_in() {
        let started = Instant::now();
        let mut pipeline = FramePipeline::new(started);
        let mut turn = RecordingTurn::default();
        assert!(pipeline.run(started, &mut turn).unwrap().stages.is_empty());
        pipeline.set_instrumentation(true);
        let stages = pipeline.run(started, &mut turn).unwrap().stages;
        assert_eq!(stages.len(), 12);
        assert!(stages.iter().all(|stage| stage.allocations == 0));
    }

    #[test]
    fn allocations_are_attributed_to_the_stage_that_made_them() {
        // One counted allocation per stage entered, so each stage must report
        // exactly the ones its own operation made.
        fn counter() -> u64 {
            COUNTED.with(|counted| counted.get())
        }
        thread_local! {
            static COUNTED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
        }

        #[derive(Default)]
        struct AllocatingTurn(RecordingTurn);
        macro_rules! counted {
            ($name:ident($($argument:ident: $type:ty),*)) => {
                fn $name(&mut self $(, $argument: $type)*) -> Result<(), ()> {
                    COUNTED.with(|counted| counted.set(counted.get() + 1));
                    self.0.$name($($argument),*)
                }
            };
        }
        impl FrameTurn for AllocatingTurn {
            type Error = ();
            counted!(drain_input());
            counted!(drain_async_results());
            counted!(run_expired_timers(time: FrameTime));
            counted!(drain_microtasks());
            counted!(run_animation_frames(time: FrameTime));
            counted!(restyle());
            counted!(layout());
            counted!(paint());
            counted!(record_native_viewport());
            counted!(submit());
            counted!(present());
        }

        let started = Instant::now();
        let mut pipeline = FramePipeline::new(started);
        pipeline.set_instrumentation(true);
        pipeline.set_allocation_counter(Some(counter));
        let report = pipeline
            .run(started, &mut AllocatingTurn::default())
            .unwrap();
        assert!(report.stages.iter().all(|stage| stage.allocations == 1));
    }
}
