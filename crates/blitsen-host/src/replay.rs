//! Fixed-timestep replay of a recorded input trace.
//!
//! The loop hands JavaScript timestamps that come only from the trace, so the
//! same trace produces the same frames on every run, and measures wall-clock
//! cost alongside them. Digests are taken after the frame is finished and are
//! never counted as frame cost.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use blitsen_blitz::BlitzDom;
use blitsen_core::frame::{FRAME_BUCKET_EDGES_MS, FrameHistogram, FrameStage};
use blitsen_core::replay::{FrameDigest, InputTrace};
use blitsen_dom::{DomBackend, LayoutSnapshot};
use blitsen_js::{JsEngine, JsError};
use blitz::dom::DocumentConfig;
use blitz::traits::shell::{ColorScheme, Viewport};
use serde::Serialize;

use crate::alloc::{self, AllocationCounts};
use crate::frame_loop::FrameLoop;
use crate::harness::{
    load_document_harness_with_hooks, record_frame, render_document, visit_elements,
};

/// Digest domains. Bump the version when a digest's inputs change, so a stale
/// golden fails loudly instead of comparing two different questions.
const DOM_DIGEST: &str = "blitsen.dom.v1";
const LAYOUT_DIGEST: &str = "blitsen.layout.v1";
const PIXEL_DIGEST: &str = "blitsen.pixels.v1";

/// Text and shapes whose rasterization depends on the host's fonts and CPU.
///
/// Two machines that agree on this digest agree on pixel output; two that do not
/// cannot be held to each other's golden pixels.
const FINGERPRINT_FIXTURE: &str = r#"<!doctype html><html><head><style>
  html, body { margin: 0; background: #07111c; color: #f7fbff; font: 16px sans-serif }
  p { margin: 8px; letter-spacing: 2px }
  div { width: 111px; height: 13px; border-radius: 6px; background: #72e7f2 }
</style></head><body><p>Blitsen frame fingerprint 0123456789</p><div></div></body></html>"#;

/// One measured frame.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FrameRecord {
    frame: u32,
    timestamp_ms: f64,
    frame_us: u64,
    display_list_us: u64,
    digest_us: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    allocations: Option<AllocationCounts>,
}

/// Summary of a set of measured durations.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HistogramReport {
    count: usize,
    mean_us: u64,
    min_us: u64,
    p50_us: u64,
    p95_us: u64,
    p99_us: u64,
    max_us: u64,
    over_budget: usize,
    buckets: Vec<BucketReport>,
}

/// Frames whose duration fell in one histogram bucket.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BucketReport {
    /// Inclusive upper edge in milliseconds, absent for the overflow bucket.
    #[serde(skip_serializing_if = "Option::is_none")]
    upper_ms: Option<f64>,
    frames: usize,
}

impl HistogramReport {
    fn new(durations: &[Duration]) -> Option<Self> {
        let histogram = FrameHistogram::from_durations(durations)?;
        Some(Self {
            count: histogram.count,
            mean_us: micros(histogram.mean()),
            min_us: micros(histogram.min),
            p50_us: micros(histogram.p50),
            p95_us: micros(histogram.p95),
            p99_us: micros(histogram.p99),
            max_us: micros(histogram.max),
            over_budget: histogram.over_budget,
            buckets: histogram
                .buckets
                .iter()
                .enumerate()
                .map(|(bucket, frames)| BucketReport {
                    upper_ms: FRAME_BUCKET_EDGES_MS.get(bucket).copied(),
                    frames: *frames,
                })
                .collect(),
        })
    }
}

/// Per-stage cost across the whole replay.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StageReport {
    stage: String,
    total_us: u64,
    mean_us: u64,
    p95_us: u64,
    max_us: u64,
    share: f64,
    /// Median allocations the stage made per frame; absent without the audit.
    #[serde(skip_serializing_if = "Option::is_none")]
    allocations: Option<u64>,
}

/// Everything one replay observed.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayReport {
    application: String,
    width: u32,
    height: u32,
    frames: u32,
    frame_duration_ms: f64,
    warmup_frames: u32,
    /// Rasterization environment; pixel digests are only comparable between runs
    /// that report the same one.
    fingerprint: String,
    dom: Vec<String>,
    layout: Vec<String>,
    pixels: Vec<String>,
    histogram: HistogramReport,
    steady: HistogramReport,
    display_list: HistogramReport,
    stages: Vec<StageReport>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    recorded: Vec<String>,
    records: Vec<FrameRecord>,
}

fn micros(duration: Duration) -> u64 {
    duration.as_micros() as u64
}

fn stage_name(stage: FrameStage) -> &'static str {
    match stage {
        FrameStage::Input => "input",
        FrameStage::AsyncResults => "asyncResults",
        FrameStage::Timers => "timers",
        FrameStage::TimerMicrotasks => "timerMicrotasks",
        FrameStage::AnimationFrame => "animationFrame",
        FrameStage::AnimationMicrotasks => "animationMicrotasks",
        FrameStage::Restyle => "restyle",
        FrameStage::Layout => "layout",
        FrameStage::Paint => "paint",
        FrameStage::NativeViewport => "nativeViewport",
        FrameStage::Submit => "submit",
        FrameStage::Present => "present",
    }
}

/// Digests the tree the frame was built from and the geometry it resolved.
///
/// The DOM digest is portable: it holds only what the application wrote. The
/// layout digest carries measured boxes, so text-sized elements make it depend
/// on the host's fonts, exactly like the pixel digest does.
fn digest_document(
    document: &Rc<RefCell<BlitzDom>>,
    layout: LayoutSnapshot,
) -> Result<(String, String), JsError> {
    let mut dom = FrameDigest::new(DOM_DIGEST);
    let mut geometry = FrameDigest::new(LAYOUT_DIGEST);
    let document = document.borrow();
    visit_elements(&document, |element| {
        dom.field(element.tag());
        for attribute in element.attributes() {
            dom.field(&attribute.name.local).field(&attribute.value);
        }
        dom.field(&element.inline_style()?)
            .field(&element.text_content()?);
        let rect = element.bounding_rect(layout)?;
        geometry
            .field(element.tag())
            .number(f64::from(rect.x))
            .number(f64::from(rect.y))
            .number(f64::from(rect.width))
            .number(f64::from(rect.height));
        Ok(())
    })?;
    Ok((dom.finish(), geometry.finish()))
}

fn digest_pixels(pixels: &[u8], width: u32, height: u32) -> String {
    let mut digest = FrameDigest::new(PIXEL_DIGEST);
    digest
        .number(f64::from(width))
        .number(f64::from(height))
        .bytes(pixels);
    digest.finish()
}

/// Digests a fixed fixture to identify this machine's text and raster output.
pub fn fingerprint() -> String {
    let (width, height) = (256, 64);
    let document = Rc::new(RefCell::new(BlitzDom::from_html(
        FINGERPRINT_FIXTURE,
        DocumentConfig {
            viewport: Some(Viewport::new(width, height, 1.0, ColorScheme::Light)),
            ..Default::default()
        },
    )));
    if document.borrow_mut().flush_layout().is_err() {
        return "unavailable".into();
    }
    digest_pixels(&render_document(&document, width, height), width, height)
}

/// Replays `trace` against `entrypoint` and reports digests and frame cost.
pub fn replay<E: JsEngine + Clone + 'static>(
    engine: E,
    entrypoint: &Path,
    trace: InputTrace,
    record_into: Option<&Path>,
    record_frames: &[u32],
) -> Result<ReplayReport, JsError> {
    let (width, height) = (trace.width, trace.height);
    let (engine, document, hooks) = load_document_harness_with_hooks(
        engine,
        entrypoint,
        width,
        height,
        crate::dom_bridge::DocumentMode::TestHarness,
    )?;
    let trace = Rc::new(trace);
    let mut frame_loop = FrameLoop::new_with_hooks(
        engine,
        Rc::clone(&document),
        width,
        height,
        Some(Rc::clone(&trace)),
        hooks,
    );
    let count = trace.frames as usize;
    let mut records = Vec::with_capacity(count);
    let mut frame_times = Vec::with_capacity(count);
    let mut display_lists = Vec::with_capacity(count);
    let mut stage_times: Vec<(FrameStage, Vec<Duration>, Vec<u64>)> = Vec::new();
    let mut dom_digests = Vec::with_capacity(count);
    let mut layout_digests = Vec::with_capacity(count);
    let mut pixel_digests = Vec::with_capacity(count);
    let mut recorded = Vec::new();

    for frame in 1..=trace.frames {
        let before = alloc::snapshot();
        let report = frame_loop.advance(frame, trace.timestamp_ms(frame))?;
        let allocations = alloc::snapshot()
            .zip(before)
            .map(|(after, before)| after.since(before));

        let frame_time: Duration = report.stages.iter().map(|stage| stage.duration).sum();
        if stage_times.is_empty() {
            stage_times = report
                .stages
                .iter()
                .map(|timing| {
                    (
                        timing.stage,
                        Vec::with_capacity(count),
                        Vec::with_capacity(count),
                    )
                })
                .collect();
        }
        for (index, timing) in report.stages.iter().enumerate() {
            stage_times[index].1.push(timing.duration);
            stage_times[index].2.push(timing.allocations);
        }

        let digest_started = std::time::Instant::now();
        let layout = frame_loop
            .layout()
            .ok_or_else(|| JsError::new("frame resolved no layout"))?;
        let (dom, geometry) = digest_document(&document, layout)?;
        pixel_digests.push(digest_pixels(frame_loop.pixels(), width, height));
        dom_digests.push(dom);
        layout_digests.push(geometry);
        let digest_time = digest_started.elapsed();

        if let Some(directory) = record_into
            && (record_frames.is_empty() || record_frames.contains(&frame))
        {
            let path = record_frame(directory, frame, frame_loop.pixels(), width, height)?;
            recorded.push(path.to_string_lossy().into_owned());
        }

        records.push(FrameRecord {
            frame,
            timestamp_ms: trace.timestamp_ms(frame),
            frame_us: micros(frame_time),
            display_list_us: micros(frame_loop.display_list()),
            digest_us: micros(digest_time),
            allocations,
        });
        frame_times.push(frame_time);
        display_lists.push(frame_loop.display_list());
    }

    let missing = || JsError::new("replay measured no frames");
    let total: Duration = frame_times.iter().sum();
    let stages = report_stages(&stage_times, total, alloc::snapshot().is_some());
    Ok(ReplayReport {
        application: trace.application.clone(),
        width,
        height,
        frames: trace.frames,
        frame_duration_ms: trace.frame_duration_ms,
        warmup_frames: trace.warmup_frames,
        fingerprint: fingerprint(),
        dom: dom_digests,
        layout: layout_digests,
        pixels: pixel_digests,
        histogram: HistogramReport::new(&frame_times).ok_or_else(missing)?,
        steady: HistogramReport::new(&frame_times[trace.warmup_frames as usize..])
            .ok_or_else(missing)?,
        display_list: HistogramReport::new(&display_lists).ok_or_else(missing)?,
        stages,
        recorded,
        records,
    })
}

fn report_stages(
    stage_times: &[(FrameStage, Vec<Duration>, Vec<u64>)],
    total: Duration,
    audited: bool,
) -> Vec<StageReport> {
    stage_times
        .iter()
        .filter_map(|(stage, durations, allocations)| {
            let histogram = FrameHistogram::from_durations(durations)?;
            let mut allocations = allocations.clone();
            allocations.sort_unstable();
            Some(StageReport {
                stage: stage_name(*stage).into(),
                total_us: micros(histogram.total),
                mean_us: micros(histogram.mean()),
                p95_us: micros(histogram.p95),
                max_us: micros(histogram.max),
                share: if total.is_zero() {
                    0.0
                } else {
                    histogram.total.as_secs_f64() / total.as_secs_f64()
                },
                allocations: audited.then(|| allocations[allocations.len() / 2]),
            })
        })
        .collect()
}
