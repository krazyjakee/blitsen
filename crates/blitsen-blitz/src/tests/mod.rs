//! Shared fixtures for the backend tests, and the topic modules that use them.

mod boundary;
mod canvas;
mod cursor;
mod forms;
mod images;
mod ranges;
mod stylesheets;
mod surfaces;
mod text;
mod ua;
mod viewport;

use anyrender::recording::RenderCommand;
use anyrender::{Paint, Scene};
use anyrender_vello_cpu::VelloCpuImageRenderer;
use std::sync::{Arc, Mutex};

use blitsen_dom::{DomBackend, DomError, DomName, ImageState, LayoutSnapshot, LinkState, NodeKind};
use blitz::dom::DocumentConfig;
use blitz::traits::net::{NetHandler, NetProvider, Request};
use blitz::traits::shell::{ColorScheme, Viewport};
use kurbo::{BezPath, Point, Shape as _};

use super::BlitzDom;
use super::resources::LocalResources;

/// Base URL of the checked-in subresource fixtures.
///
/// `file:` keeps the tests on the synchronous provider, which is the same
/// path a headless harness takes.
fn fixtures_url() -> String {
    format!("file://{}/fixtures/", env!("CARGO_MANIFEST_DIR"))
}

/// The value the cascade resolved for a property of an element with an id.
fn resolved(dom: &BlitzDom, snapshot: LayoutSnapshot, id: &str, property: &str) -> String {
    let node = dom
        .get_element_by_id(id)
        .expect("query")
        .expect("element exists");
    dom.resolved_style(node, property, snapshot)
        .expect("resolved style")
        .expect("property is resolvable")
}

/// Renders a document and returns straight-alpha RGBA8 rows.
fn render(dom: &mut BlitzDom, width: u32, height: u32) -> Vec<u8> {
    anyrender::render_to_buffer::<VelloCpuImageRenderer, _>(
        |scene| {
            blitz_paint::paint_scene(scene, dom.document_mut().as_mut(), 1.0, width, height, 0, 0);
        },
        width,
        height,
    )
}

fn pixel(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let start = ((y * width + x) * 4) as usize;
    pixels[start..start + 4].try_into().expect("rgba8 pixel")
}

/// Bounding box `(x, y, width, height)` of everything the frame painted.
///
/// The fixture font draws each letter as a solid em block, so the box of a
/// text run is the run's exact metrics — which is what makes "did the web
/// font actually get used" answerable from pixels alone.
fn inked_bounds(pixels: &[u8], width: u32) -> Option<(u32, u32, u32, u32)> {
    let inked = pixels
        .chunks_exact(4)
        .enumerate()
        .filter(|(_, pixel)| pixel[3] > 0)
        .map(|(index, _)| (index as u32 % width, index as u32 / width));
    inked
        .fold(None, |bounds: Option<[u32; 4]>, (x, y)| {
            Some(match bounds {
                Some([left, top, right, bottom]) => {
                    [left.min(x), top.min(y), right.max(x), bottom.max(y)]
                }
                None => [x, y, x, y],
            })
        })
        .map(|[left, top, right, bottom]| (left, top, right - left + 1, bottom - top + 1))
}

/// A provider that answers nothing until it is told to.
///
/// Every other subresource in these tests resolves before `fetch` returns,
/// which is precisely the case where an in-flight state can never be
/// observed. Deferring reinstates the asynchrony a real window has.
type HeldRequest = (Request, Box<dyn NetHandler>);

#[derive(Clone, Default)]
struct DeferredResources(Arc<Mutex<Vec<HeldRequest>>>);

impl NetProvider for DeferredResources {
    fn fetch(&self, _doc_id: usize, request: Request, handler: Box<dyn NetHandler>) {
        self.0
            .lock()
            .expect("deferred requests")
            .push((request, handler));
    }
}

impl DeferredResources {
    fn deliver(&self) {
        let held = self
            .0
            .lock()
            .expect("deferred requests")
            .drain(..)
            .collect::<Vec<_>>();
        for (request, handler) in held {
            LocalResources.fetch(0, request, handler);
        }
    }
}

/// A document whose relative URLs resolve against the checked-in fixtures.
fn fixture_document(html: &str, provider: Option<Arc<dyn NetProvider>>) -> BlitzDom {
    BlitzDom::from_html(
        &format!("<style>html, body {{ margin: 0 }}</style>{html}"),
        DocumentConfig {
            base_url: Some(fixtures_url()),
            net_provider: provider,
            viewport: Some(Viewport::new(400, 200, 1.0, ColorScheme::Light)),
            ..Default::default()
        },
    )
}

fn backend() -> BlitzDom {
    BlitzDom::from_html(
        r#"<style>.wide { width: 240px }</style><body><main id="host"><p id="x">old</p></main></body>"#,
        DocumentConfig {
            viewport: Some(Viewport::new(800, 600, 1.0, ColorScheme::Light)),
            ..Default::default()
        },
    )
}

fn viewport_document(body: &str, scale: f32) -> BlitzDom {
    BlitzDom::from_html(
        &format!("<style>html, body {{ margin: 0 }}</style><body>{body}</body>"),
        DocumentConfig {
            viewport: Some(Viewport::new(400, 300, scale, ColorScheme::Light)),
            ..Default::default()
        },
    )
}
