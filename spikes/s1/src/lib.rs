use std::{
    cell::RefCell,
    collections::VecDeque,
    thread,
    time::{Duration, Instant},
};

use napi::{Error, Result, Status};
use napi_derive::napi;
use serde::Serialize;
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    platform::pump_events::EventLoopExtPumpEvents,
    window::{Window, WindowAttributes, WindowId},
};

#[derive(Clone)]
struct SyntheticInput {
    scheduled: Instant,
}

struct Harness {
    event_loop: EventLoop<SyntheticInput>,
    app: App,
}

#[derive(Default)]
struct App {
    window: Option<Window>,
    pending_inputs: VecDeque<Instant>,
    pump_times: Vec<Instant>,
    paint_times: Vec<Instant>,
    input_to_paint_ms: Vec<f64>,
    expected_samples: usize,
    period_ms: f64,
}

#[derive(Serialize)]
struct Summary {
    bun_drives_winit: bool,
    expected_period_ms: f64,
    pump_samples: usize,
    paint_callbacks: usize,
    input_samples: usize,
    pump_interval_mean_ms: f64,
    pump_interval_stddev_ms: f64,
    pump_interval_p50_ms: f64,
    pump_interval_p95_ms: f64,
    pump_interval_p99_ms: f64,
    paint_interval_mean_ms: f64,
    paint_interval_stddev_ms: f64,
    paint_interval_p50_ms: f64,
    paint_interval_p95_ms: f64,
    paint_interval_p99_ms: f64,
    input_to_paint_mean_ms: f64,
    input_to_paint_p50_ms: f64,
    input_to_paint_p95_ms: f64,
    input_to_paint_p99_ms: f64,
    input_to_paint_max_ms: f64,
}

impl ApplicationHandler<SyntheticInput> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            let attributes = WindowAttributes::default()
                .with_title("Blitsen S1")
                .with_inner_size(LogicalSize::new(96.0, 96.0));
            self.window = Some(event_loop.create_window(attributes).unwrap());
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: SyntheticInput) {
        self.pending_inputs.push_back(event.scheduled);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if self.window.as_ref().map(Window::id) != Some(window_id) {
            return;
        }
        match event {
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                self.paint_times.push(now);
                while let Some(scheduled) = self.pending_inputs.pop_front() {
                    self.input_to_paint_ms
                        .push(now.duration_since(scheduled).as_secs_f64() * 1_000.0);
                }
            }
            WindowEvent::CloseRequested => event_loop.exit(),
            _ => {}
        }
    }
}

thread_local! {
    static HARNESS: RefCell<Option<Harness>> = const { RefCell::new(None) };
}

fn napi_error(message: impl Into<String>) -> Error {
    Error::new(Status::GenericFailure, message.into())
}

fn start_scheduler(proxy: EventLoopProxy<SyntheticInput>, samples: usize, period: Duration) {
    thread::spawn(move || {
        let origin = Instant::now() + Duration::from_millis(100);
        for index in 0..samples {
            let scheduled = origin + period.mul_f64(index as f64);
            if let Some(delay) = scheduled.checked_duration_since(Instant::now()) {
                thread::sleep(delay);
            }
            if proxy.send_event(SyntheticInput { scheduled }).is_err() {
                return;
            }
        }
    });
}

#[napi]
pub fn start_fallback(samples: u32, period_micros: u32) -> Result<()> {
    HARNESS.with(|slot| {
        if slot.borrow().is_some() {
            return Err(napi_error("S1 harness already started"));
        }
        let mut event_loop = EventLoop::<SyntheticInput>::with_user_event()
            .build()
            .map_err(|error| napi_error(error.to_string()))?;
        event_loop.set_control_flow(ControlFlow::Poll);
        let proxy = event_loop.create_proxy();
        let period = Duration::from_micros(u64::from(period_micros));
        let mut app = App {
            expected_samples: samples as usize,
            period_ms: period.as_secs_f64() * 1_000.0,
            ..Default::default()
        };
        event_loop.pump_app_events(Some(Duration::ZERO), &mut app);
        if app.window.is_none() {
            return Err(napi_error(
                "winit did not create a window during initial pump",
            ));
        }
        start_scheduler(proxy, samples as usize, period);
        *slot.borrow_mut() = Some(Harness { event_loop, app });
        Ok(())
    })
}

#[napi]
pub fn pump_winit() -> Result<bool> {
    HARNESS.with(|slot| {
        let mut slot = slot.borrow_mut();
        let harness = slot
            .as_mut()
            .ok_or_else(|| napi_error("S1 harness has not started"))?;
        harness.app.pump_times.push(Instant::now());
        // A real animation host requests a frame every turn, independently of input.
        // Rendering stays inside winit's synchronous RedrawRequested callback.
        if let Some(window) = &harness.app.window {
            window.request_redraw();
        }
        harness
            .event_loop
            .pump_app_events(Some(Duration::ZERO), &mut harness.app);
        Ok(harness.app.input_to_paint_ms.len() >= harness.app.expected_samples)
    })
}

fn intervals_ms(times: &[Instant]) -> Vec<f64> {
    times
        .windows(2)
        .map(|pair| pair[1].duration_since(pair[0]).as_secs_f64() * 1_000.0)
        .collect()
}

fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len().max(1) as f64
}

fn stddev(values: &[f64]) -> f64 {
    let average = mean(values);
    (values
        .iter()
        .map(|value| (value - average).powi(2))
        .sum::<f64>()
        / values.len().max(1) as f64)
        .sqrt()
}

fn percentile(values: &[f64], percentile: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut values = values.to_vec();
    values.sort_by(f64::total_cmp);
    let index = ((values.len() - 1) as f64 * percentile).round() as usize;
    values[index]
}

#[napi]
pub fn fallback_stats() -> Result<String> {
    HARNESS.with(|slot| {
        let slot = slot.borrow();
        let harness = slot
            .as_ref()
            .ok_or_else(|| napi_error("S1 harness has not started"))?;
        let intervals = intervals_ms(&harness.app.pump_times);
        let paint_intervals = intervals_ms(&harness.app.paint_times);
        let latency = &harness.app.input_to_paint_ms;
        let summary = Summary {
            bun_drives_winit: true,
            expected_period_ms: harness.app.period_ms,
            pump_samples: harness.app.pump_times.len(),
            paint_callbacks: harness.app.paint_times.len(),
            input_samples: latency.len(),
            pump_interval_mean_ms: mean(&intervals),
            pump_interval_stddev_ms: stddev(&intervals),
            pump_interval_p50_ms: percentile(&intervals, 0.50),
            pump_interval_p95_ms: percentile(&intervals, 0.95),
            pump_interval_p99_ms: percentile(&intervals, 0.99),
            paint_interval_mean_ms: mean(&paint_intervals),
            paint_interval_stddev_ms: stddev(&paint_intervals),
            paint_interval_p50_ms: percentile(&paint_intervals, 0.50),
            paint_interval_p95_ms: percentile(&paint_intervals, 0.95),
            paint_interval_p99_ms: percentile(&paint_intervals, 0.99),
            input_to_paint_mean_ms: mean(latency),
            input_to_paint_p50_ms: percentile(latency, 0.50),
            input_to_paint_p95_ms: percentile(latency, 0.95),
            input_to_paint_p99_ms: percentile(latency, 0.99),
            input_to_paint_max_ms: latency.iter().copied().reduce(f64::max).unwrap_or(0.0),
        };
        serde_json::to_string_pretty(&summary).map_err(|error| napi_error(error.to_string()))
    })
}
