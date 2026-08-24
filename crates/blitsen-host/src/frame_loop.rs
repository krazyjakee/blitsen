//! The headless frame loop, driven by [`blitsen_core::frame::FramePipeline`].
//!
//! Every headless frame Blitsen renders — acceptance snapshots, recorded demos
//! and deterministic replays — turns here, so the pipeline's per-stage
//! instrumentation measures the loop that is actually shipped rather than a
//! test double. The windowed host still delegates its later stages to the Blitz
//! shell; see `docs/M3.md`.
//!
//! Two clocks run side by side. The timestamp handed to JavaScript comes from
//! the fixed timestep, so a replay is reproducible; the stage durations come
//! from [`Instant`], so what is reported is real elapsed work.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyrender::{ImageRenderer as _, PaintScene as _};
use anyrender_vello_cpu::VelloCpuImageRenderer;
use blitsen_blitz::BlitzDom;
use blitsen_core::frame::{FramePipeline, FrameReport, FrameTime, FrameTurn};
use blitsen_core::replay::{InputTrace, TraceInput};
use blitsen_dom::{DomBackend, LayoutSnapshot};
use blitz::dom::util::Color;
use blitz::paint::paint_scene;
use peniko::{Fill, kurbo::Rect};

use crate::dom_error;
use blitsen_js::{JsEngine, JsError};

/// One headless frame turn: input, callbacks, layout and rasterization.
pub(crate) struct FrameLoopTurn<E: JsEngine> {
    engine: E,
    document: Rc<RefCell<BlitzDom>>,
    renderer: VelloCpuImageRenderer,
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    trace: Option<Rc<InputTrace>>,
    frame: u32,
    display_list: Duration,
    layout: Option<LayoutSnapshot>,
    /// The animation-frame callback, resolved once instead of compiled every
    /// frame. Sound only because a loop never outlives the addon call that
    /// created it, so this handle stays inside its scope.
    tick: Option<E::Value>,
}

impl<E: JsEngine> FrameTurn for FrameLoopTurn<E> {
    type Error = JsError;

    fn drain_input(&mut self) -> Result<(), Self::Error> {
        // Shared rather than borrowed: dispatching needs the engine mutably, and
        // cloning the events per frame would allocate on the hot path.
        let Some(trace) = self.trace.clone() else {
            return Ok(());
        };
        trace
            .inputs_for_frame(self.frame)
            .try_for_each(|input| self.dispatch(input))
    }

    fn drain_async_results(&mut self) -> Result<(), Self::Error> {
        // The one handoff point for off-thread work: subresources fetched by the
        // net provider rejoin the document here and nowhere else in the turn.
        self.document.borrow_mut().document_mut().handle_messages();
        Ok(())
    }

    fn run_expired_timers(&mut self, _time: FrameTime) -> Result<(), Self::Error> {
        // Bun owns the timer queue in Phase 1 (S1): `setTimeout` callbacks arrive
        // between addon calls, not inside a turn. Structurally empty, not absent.
        Ok(())
    }

    fn drain_microtasks(&mut self) -> Result<(), Self::Error> {
        // Also Bun's: Node-API exposes no nested microtask checkpoint, so the
        // queue drains when control returns from the addon.
        self.engine.drain_microtasks().map(drop)
    }

    fn run_animation_frames(&mut self, time: FrameTime) -> Result<(), Self::Error> {
        let tick = match &self.tick {
            Some(tick) => tick.clone(),
            None => {
                let tick = self.engine.evaluate_script(
                    "globalThis.__blitsenAnimationFrameTick",
                    "blitsen:frame-loop-tick",
                )?;
                self.tick.insert(tick).clone()
            }
        };
        let timestamp = self.engine.number(time.timestamp.as_secs_f64() * 1_000.0);
        self.engine.call(&tick, None, &[timestamp]).map(drop)
    }

    fn restyle(&mut self) -> Result<(), Self::Error> {
        // Blitz resolves style and layout in one pass; both are measured in
        // `layout`, and splitting them would need a change inside blitz-dom.
        Ok(())
    }

    fn layout(&mut self) -> Result<(), Self::Error> {
        self.layout = Some(
            self.document
                .borrow_mut()
                .flush_layout()
                .map_err(dom_error)?,
        );
        Ok(())
    }

    fn paint(&mut self) -> Result<(), Self::Error> {
        let (width, height) = (self.width, self.height);
        let document = &self.document;
        let display_list = &mut self.display_list;
        // The CPU image renderer builds the display list and rasterizes it in one
        // call, so this stage covers both; the display-list half is timed inside
        // the closure and reported separately.
        self.renderer.reset();
        self.pixels.resize(width as usize * height as usize * 4, 0);
        self.renderer.render(
            |scene| {
                let started = Instant::now();
                let mut document = document.borrow_mut();
                scene.fill(
                    Fill::NonZero,
                    Default::default(),
                    Color::WHITE,
                    Default::default(),
                    &Rect::new(0.0, 0.0, f64::from(width), f64::from(height)),
                );
                paint_scene(
                    scene,
                    document.document_mut().as_mut(),
                    1.0,
                    width,
                    height,
                    0,
                    0,
                );
                *display_list = started.elapsed();
            },
            &mut self.pixels,
        );
        Ok(())
    }

    fn record_native_viewport(&mut self) -> Result<(), Self::Error> {
        // `<blitsen-view>` surfaces composite through blitz-paint's custom-widget
        // seam during `paint`; nothing is recorded separately in a headless turn.
        Ok(())
    }

    fn submit(&mut self) -> Result<(), Self::Error> {
        // No GPU queue: the CPU rasterizer has already written the pixel buffer.
        Ok(())
    }

    fn present(&mut self) -> Result<(), Self::Error> {
        // No swapchain either. Digesting or encoding the finished buffer is the
        // harness's own cost and is measured outside the frame.
        Ok(())
    }
}

impl<E: JsEngine> FrameLoopTurn<E> {
    fn dispatch(&mut self, input: &TraceInput) -> Result<(), JsError> {
        let quote = |value: &str| {
            serde_json::to_string(value).map_err(|error| JsError::new(error.to_string()))
        };
        let script = match input {
            TraceInput::Key {
                event_type,
                key,
                code,
                repeat,
            } => format!(
                "globalThis.__blitsenDispatchKeyboardEvent({}, {{ bubbles: true, cancelable: true, \
                 key: {}, code: {}, repeat: {repeat} }})",
                quote(event_type)?,
                quote(key)?,
                quote(code)?,
            ),
            TraceInput::Pointer { event_type, x, y } => format!(
                "globalThis.__blitsenInjectPointerAt({}, {x}, {y})",
                quote(event_type)?
            ),
        };
        self.engine
            .evaluate_script(&script, "blitsen:replay-input")
            .map(drop)
    }
}

/// A document advancing at a fixed timestep through the frame pipeline.
pub struct FrameLoop<E: JsEngine> {
    pipeline: FramePipeline,
    started: Instant,
    turn: FrameLoopTurn<E>,
}

impl<E: JsEngine> FrameLoop<E> {
    /// Prepares a loop over an already-loaded document.
    pub fn new(
        engine: E,
        document: Rc<RefCell<BlitzDom>>,
        width: u32,
        height: u32,
        trace: Option<Rc<InputTrace>>,
    ) -> Self {
        let mut frame_loop = Self::new_uninstrumented(engine, document, width, height, trace);
        frame_loop.enable_instrumentation();
        frame_loop
    }

    pub(crate) fn new_uninstrumented(
        engine: E,
        document: Rc<RefCell<BlitzDom>>,
        width: u32,
        height: u32,
        trace: Option<Rc<InputTrace>>,
    ) -> Self {
        let started = Instant::now();
        Self {
            pipeline: FramePipeline::new(started),
            started,
            turn: FrameLoopTurn {
                engine,
                document,
                renderer: VelloCpuImageRenderer::new(width, height),
                pixels: Vec::new(),
                width,
                height,
                trace,
                frame: 0,
                display_list: Duration::ZERO,
                layout: None,
                tick: None,
            },
        }
    }

    /// Enables the per-stage measurements consumed by replay reports.
    fn enable_instrumentation(&mut self) {
        self.pipeline.set_instrumentation(true);
        self.pipeline
            .set_allocation_counter(crate::alloc::stage_counter());
    }

    /// Runs one turn for the one-based `frame` at `timestamp_ms` since start.
    pub fn advance(&mut self, frame: u32, timestamp_ms: f64) -> Result<FrameReport, JsError> {
        self.turn.frame = frame;
        let now = self.started + Duration::from_secs_f64(timestamp_ms / 1_000.0);
        self.pipeline.run(now, &mut self.turn)
    }

    /// Rasterized RGBA pixels of the most recent frame.
    pub fn pixels(&self) -> &[u8] {
        &self.turn.pixels
    }

    /// Time spent building the display list in the most recent frame.
    pub fn display_list(&self) -> Duration {
        self.turn.display_list
    }

    /// Layout snapshot resolved by the most recent frame.
    pub fn layout(&self) -> Option<LayoutSnapshot> {
        self.turn.layout
    }
}
