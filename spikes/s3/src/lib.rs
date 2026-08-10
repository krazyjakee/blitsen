use std::{cell::RefCell, sync::Arc, time::Duration};

use napi::{Error, Result, Status};
use napi_derive::napi;
use serde::Serialize;
use wgpu::{
    Color, CommandEncoderDescriptor, CurrentSurfaceTexture, Device, DeviceDescriptor, Instance,
    LoadOp, Operations, PowerPreference, Queue, RenderPassColorAttachment, RenderPassDescriptor,
    RequestAdapterOptions, StoreOp, Surface, SurfaceConfiguration, TextureViewDescriptor,
};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalSize},
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    platform::pump_events::EventLoopExtPumpEvents,
    window::{Window, WindowAttributes, WindowId},
};

struct Harness {
    event_loop: EventLoop<()>,
    app: App,
}

#[derive(Default)]
struct App {
    gpu: Option<Gpu>,
    target_frames: u32,
    presented_frames: u32,
    adapter_name: String,
    backend: String,
    error: Option<String>,
}

struct Gpu {
    window: Arc<Window>,
    surface: Surface<'static>,
    device: Device,
    queue: Queue,
    config: SurfaceConfiguration,
}

#[derive(Serialize)]
struct Summary<'a> {
    addon_loaded_in_bun: bool,
    real_window: bool,
    wgpu_surface: bool,
    adapter: &'a str,
    backend: &'a str,
    presented_frames: u32,
    target_frames: u32,
    error: Option<&'a str>,
}

impl Gpu {
    fn new(window: Arc<Window>) -> std::result::Result<(Self, String, String), String> {
        let size = window.inner_size();
        let instance = Instance::default();
        let surface = instance
            .create_surface(window.clone())
            .map_err(|error| error.to_string())?;
        let adapter = pollster::block_on(instance.request_adapter(&RequestAdapterOptions {
            power_preference: PowerPreference::LowPower,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
        }))
        .map_err(|error| error.to_string())?;
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("Blitsen S3 device"),
            ..Default::default()
        }))
        .map_err(|error| error.to_string())?;
        let config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or_else(|| "adapter cannot present to the winit surface".to_string())?;
        surface.configure(&device, &config);
        Ok((
            Self {
                window,
                surface,
                device,
                queue,
                config,
            },
            info.name,
            format!("{:?}", info.backend),
        ))
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
    }

    fn clear_and_present(&self, frame: u32) -> std::result::Result<(), String> {
        let output = match self.surface.get_current_texture() {
            CurrentSurfaceTexture::Success(output) | CurrentSurfaceTexture::Suboptimal(output) => {
                output
            }
            status => return Err(format!("surface acquisition failed: {status:?}")),
        };
        let view = output
            .texture
            .create_view(&TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("Blitsen S3 clear encoder"),
            });
        let phase = f64::from(frame % 120) / 119.0;
        {
            let _pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("Blitsen S3 clear pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Clear(Color {
                            r: 0.08 + phase * 0.25,
                            g: 0.16,
                            b: 0.34 - phase * 0.20,
                            a: 1.0,
                        }),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        self.queue.submit(Some(encoder.finish()));
        output.present();
        Ok(())
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() || self.error.is_some() {
            return;
        }
        let attributes = WindowAttributes::default()
            .with_title("Blitsen S3 — Bun + winit + wgpu")
            .with_inner_size(LogicalSize::new(320.0, 180.0));
        let result = event_loop
            .create_window(attributes)
            .map(Arc::new)
            .map_err(|error| error.to_string())
            .and_then(Gpu::new);
        match result {
            Ok((gpu, adapter_name, backend)) => {
                self.adapter_name = adapter_name;
                self.backend = backend;
                gpu.window.request_redraw();
                self.gpu = Some(gpu);
            }
            Err(error) => self.error = Some(error),
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(gpu) = &mut self.gpu else {
            return;
        };
        if gpu.window.id() != window_id {
            return;
        }
        match event {
            WindowEvent::Resized(size) => gpu.resize(size),
            WindowEvent::RedrawRequested => match gpu.clear_and_present(self.presented_frames) {
                Ok(()) => {
                    self.presented_frames += 1;
                    if self.presented_frames >= self.target_frames {
                        event_loop.exit();
                    }
                }
                Err(error) => {
                    self.error = Some(error);
                    event_loop.exit();
                }
            },
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

#[napi]
pub fn open_window(target_frames: u32) -> Result<()> {
    HARNESS.with(|slot| {
        if slot.borrow().is_some() {
            return Err(napi_error("S3 harness already started"));
        }
        let mut event_loop = EventLoop::new().map_err(|error| napi_error(error.to_string()))?;
        event_loop.set_control_flow(ControlFlow::Poll);
        let mut app = App {
            target_frames,
            ..Default::default()
        };
        event_loop.pump_app_events(Some(Duration::ZERO), &mut app);
        if let Some(error) = &app.error {
            return Err(napi_error(error.clone()));
        }
        if app.gpu.is_none() {
            return Err(napi_error("winit/wgpu initialization did not complete"));
        }
        *slot.borrow_mut() = Some(Harness { event_loop, app });
        Ok(())
    })
}

#[napi]
pub fn pump_window() -> Result<bool> {
    HARNESS.with(|slot| {
        let mut slot = slot.borrow_mut();
        let harness = slot
            .as_mut()
            .ok_or_else(|| napi_error("S3 harness has not started"))?;
        if let Some(window) = harness.app.gpu.as_ref().map(|gpu| &gpu.window) {
            window.request_redraw();
        }
        harness
            .event_loop
            .pump_app_events(Some(Duration::ZERO), &mut harness.app);
        if let Some(error) = &harness.app.error {
            return Err(napi_error(error.clone()));
        }
        Ok(harness.app.presented_frames >= harness.app.target_frames)
    })
}

#[napi]
pub fn window_stats() -> Result<String> {
    HARNESS.with(|slot| {
        let slot = slot.borrow();
        let harness = slot
            .as_ref()
            .ok_or_else(|| napi_error("S3 harness has not started"))?;
        serde_json::to_string_pretty(&Summary {
            addon_loaded_in_bun: true,
            real_window: harness.app.gpu.is_some(),
            wgpu_surface: harness.app.gpu.is_some(),
            adapter: &harness.app.adapter_name,
            backend: &harness.app.backend,
            presented_frames: harness.app.presented_frames,
            target_frames: harness.app.target_frames,
            error: harness.app.error.as_deref(),
        })
        .map_err(|error| napi_error(error.to_string()))
    })
}
