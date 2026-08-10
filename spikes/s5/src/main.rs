use std::sync::Arc;

use anyrender::recording::RenderCommand;
use anyrender::{Paint, PaintRef, PaintScene, ResourceId, Scene, WindowRenderer};
use anyrender_vello::VelloWindowRenderer;
use blitz_dom::node::ComputedStyles;
use blitz_dom::{DocumentConfig, Widget};
use blitz_html::HtmlDocument;
use blitz_paint::paint_scene;
use blitz_traits::shell::{ColorScheme, Viewport};
use peniko::kurbo::{Affine, Rect};
use peniko::{Fill, ImageBrush, ImageSampler};
use wgpu_context::DeviceHandle;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowAttributes, WindowId};

const WIDTH: u32 = 480;
const HEIGHT: u32 = 320;

#[derive(Clone)]
struct TextureAndHandle {
    texture: wgpu::Texture,
    handle: ResourceId,
}

struct ActiveRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    displayed: Option<TextureAndHandle>,
    next: Option<TextureAndHandle>,
}

enum RendererState {
    Active(Box<ActiveRenderer>),
    Suspended,
}

struct AppTexture {
    state: RendererState,
    frames: u64,
}

struct SolidWidget;

impl Widget for SolidWidget {
    fn connected(&mut self) {}

    fn disconnected(&mut self) {}

    fn handle_event(&mut self, _event: &blitz_traits::events::UiEvent) {}

    fn paint(
        &mut self,
        _context: &mut dyn anyrender::RenderContext,
        _styles: &ComputedStyles,
        width: u32,
        height: u32,
        _scale: f64,
    ) -> Scene {
        assert_eq!((width, height), (WIDTH, HEIGHT));
        let mut scene = Scene::new();
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            PaintRef::Solid(peniko::Color::from_rgb8(0, 204, 51)),
            None,
            &Rect::from_origin_size((0.0, 0.0), (width as f64, height as f64)),
        );
        scene
    }
}

impl AppTexture {
    fn new() -> Self {
        Self {
            state: RendererState::Suspended,
            frames: 0,
        }
    }

    fn render(
        &mut self,
        context: &mut dyn anyrender::RenderContext,
        width: u32,
        height: u32,
    ) -> Option<ResourceId> {
        let RendererState::Active(active) = &mut self.state else {
            return None;
        };

        if active
            .next
            .as_ref()
            .is_some_and(|item| item.texture.width() != width || item.texture.height() != height)
        {
            let stale = active.next.take().unwrap();
            context.unregister_resource(stale.handle);
        }

        if active.next.is_none() {
            let texture = active.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Blitsen S5 app texture"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let handle = context
                .try_register_custom_resource(Box::new(texture.clone()))
                .expect("Vello must accept a texture from its own wgpu device");
            active.next = Some(TextureAndHandle { texture, handle });
        }

        let next = active.next.as_ref().unwrap();
        let next_handle = next.handle;
        let view = next
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = active
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Blitsen S5 app encoder"),
            });
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Blitsen S5 app pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.0,
                            g: 0.8,
                            b: 0.2,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                multiview_mask: None,
                occlusion_query_set: None,
            });
        }
        active.queue.submit(Some(encoder.finish()));
        std::mem::swap(&mut active.next, &mut active.displayed);
        Some(next_handle)
    }
}

impl Widget for AppTexture {
    fn connected(&mut self) {}

    fn disconnected(&mut self) {}

    fn can_create_surfaces(&mut self, context: &mut dyn anyrender::RenderContext) {
        let device_handle = context
            .renderer_specific_context()
            .and_then(|value| value.downcast::<DeviceHandle>().ok())
            .expect("Vello must expose its wgpu DeviceHandle");
        let info = device_handle.adapter.get_info();
        println!("S5_DEVICE name={:?} backend={:?}", info.name, info.backend);
        self.state = RendererState::Active(Box::new(ActiveRenderer {
            device: device_handle.device.clone(),
            queue: device_handle.queue.clone(),
            displayed: None,
            next: None,
        }));
    }

    fn destroy_surfaces(&mut self) {
        self.state = RendererState::Suspended;
    }

    fn requires_redraw(&self) -> bool {
        self.frames < 4
    }

    fn handle_event(&mut self, _event: &blitz_traits::events::UiEvent) {}

    fn paint(
        &mut self,
        context: &mut dyn anyrender::RenderContext,
        _styles: &ComputedStyles,
        width: u32,
        height: u32,
        _scale: f64,
    ) -> Scene {
        self.frames += 1;
        let mut scene = Scene::new();
        if let Some(image) = self.render(context, width, height) {
            scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                PaintRef::Resource(ImageBrush {
                    image,
                    sampler: ImageSampler::default(),
                }),
                None,
                &Rect::from_origin_size((0.0, 0.0), (width as f64, height as f64)),
            );
            println!(
                "S5_FRAME frame={} layout={}x{} resource={:?}",
                self.frames, width, height, image
            );
        }
        scene
    }
}

struct Harness {
    window: Option<Arc<dyn Window>>,
    renderer: VelloWindowRenderer,
    document: HtmlDocument,
    render_calls: u64,
}

impl Harness {
    fn redraw(&mut self) {
        let window = self.window.as_ref().unwrap();
        let size = window.surface_size();
        let scale = window.scale_factor() as f32;
        self.document.set_viewport(Viewport::new(
            size.width,
            size.height,
            scale,
            ColorScheme::Light,
        ));
        self.document.resolve(0.0);
        self.renderer.render(|scene| {
            paint_scene(
                scene,
                &mut self.document,
                scale as f64,
                size.width,
                size.height,
                0,
                0,
            )
        });
        self.render_calls += 1;
        println!(
            "S5_SURFACE_FRAME render_call={} surface_path=single-render-single-present",
            self.render_calls
        );
    }
}

impl ApplicationHandler for Harness {
    fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = WindowAttributes::default()
            .with_title("Blitsen S5 composite")
            .with_surface_size(LogicalSize::new(640.0, 480.0));
        let window: Arc<dyn Window> = Arc::from(event_loop.create_window(attributes).unwrap());
        let size = window.surface_size();
        let scale = window.scale_factor() as f32;
        self.document.set_viewport(Viewport::new(
            size.width,
            size.height,
            scale,
            ColorScheme::Light,
        ));
        self.document.resolve(0.0);
        self.renderer
            .resume(Arc::new(window.clone()), size.width, size.height, || {});
        assert!(self.renderer.complete_resume());
        self.renderer.set_size(size.width, size.height);
        self.window = Some(window.clone());
        self.redraw();
        window.request_redraw();
    }

    fn destroy_surfaces(&mut self, _event_loop: &dyn ActiveEventLoop) {
        self.document.destroy_surfaces();
        self.renderer.suspend();
    }

    fn resumed(&mut self, _event_loop: &dyn ActiveEventLoop) {}

    fn window_event(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                self.document.destroy_surfaces();
                self.renderer.suspend();
                self.window = None;
                event_loop.exit();
            }
            WindowEvent::SurfaceResized(size) => {
                self.renderer.set_size(size.width, size.height);
                self.redraw();
            }
            WindowEvent::RedrawRequested => {
                self.redraw();
                if self.render_calls < 4 {
                    self.window.as_ref().unwrap().request_redraw();
                } else {
                    self.document.destroy_surfaces();
                    self.renderer.suspend();
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }
}

fn html() -> String {
    format!(
        r#"<!doctype html>
        <html><head><title>Blitsen S5 composite</title><style>
        html, body {{ margin: 0; width: 100%; height: 100%; background: white; }}
        #stage {{ position: relative; width: 640px; height: 480px; }}
        #below {{ position: absolute; left: 40px; top: 40px; width: 220px; height: 220px;
                  background: rgb(230, 30, 30); z-index: -1; }}
        #app {{ position: absolute; left: 80px; top: 80px; width: {WIDTH}px; height: {HEIGHT}px;
                z-index: 0; }}
        #above {{ position: absolute; left: 400px; top: 260px; width: 200px; height: 140px;
                  background: rgb(30, 60, 230); z-index: 1; }}
        </style></head><body><div id="stage">
          <div id="below"></div><object id="app"></object><div id="above"></div>
        </div></body></html>"#
    )
}

fn document_with_widget(widget: Box<dyn Widget>) -> HtmlDocument {
    let mut document = HtmlDocument::from_html(&html(), DocumentConfig::default());
    let app = document.query_selector("#app").unwrap().unwrap();
    document.mutate().set_custom_widget(app, widget);
    document
}

fn verify_z_order() {
    let mut document = document_with_widget(Box::new(SolidWidget));
    document.set_viewport(Viewport::new(640, 480, 1.0, ColorScheme::Light));
    document.resolve(0.0);
    let mut scene = Scene::new();
    paint_scene(&mut scene, &mut document, 1.0, 640, 480, 0, 0);

    let colors: Vec<[u8; 4]> = scene
        .commands
        .iter()
        .filter_map(|command| match command {
            RenderCommand::Fill(fill) => match fill.brush {
                Paint::Solid(color) => {
                    let rgba = color.to_rgba8();
                    Some([rgba.r, rgba.g, rgba.b, rgba.a])
                }
                _ => None,
            },
            _ => None,
        })
        .collect();
    let position = |target| colors.iter().position(|color| *color == target).unwrap();
    let below = position([230, 30, 30, 255]);
    let app = position([0, 204, 51, 255]);
    let above = position([30, 60, 230, 255]);
    assert!(
        below < app && app < above,
        "unexpected paint order: {colors:?}"
    );
    println!("S5_Z_ORDER below={below} app={app} above={above}");
}

fn main() {
    verify_z_order();
    let document = document_with_widget(Box::new(AppTexture::new()));

    let event_loop = EventLoop::builder().build().unwrap();
    event_loop.set_control_flow(ControlFlow::Wait);
    event_loop
        .run_app(Harness {
            window: None,
            renderer: VelloWindowRenderer::new(),
            document,
            render_calls: 0,
        })
        .unwrap();
}
