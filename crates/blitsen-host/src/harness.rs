//! The headless harness core: what the JavaScript test suite drives.
//!
//! Not a public Rust API. It exists so a test can boot a document, run scripts
//! and read back what was painted — through whichever engine is hosting, so the
//! same assertions run against Phase 1 and Phase 2 (issue #90).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use anyrender::{PaintScene as _, render_to_buffer};
use anyrender_vello_cpu::VelloCpuImageRenderer;
use blitsen_blitz::{BlitzDom, resources::LocalResources};
use blitsen_core::{
    DocumentScript, ScriptDocument, WindowState, execute_collected_document_scripts_from,
};
use blitsen_dom::{DomBackend, LayoutSnapshot};
use blitsen_js::{JsEngine, JsError};
use blitz::dom::{DocumentConfig, util::Color};
use blitz::paint::paint_scene;
use blitz::traits::net::NetProvider;
use blitz::traits::shell::{ColorScheme, Viewport};
use peniko::{Fill, kurbo::Rect};
use serde::Serialize;

#[cfg(target_os = "macos")]
use winit::application::macos::ApplicationHandlerExtMacOS;

use crate::{DomRuntime, dom_bridge, dom_error, frame_loop};

/// The document the last `execute_document_harness` call left loaded, and the
/// viewport it was loaded at.
pub type ActiveDocumentHarness = (Rc<RefCell<BlitzDom>>, u32, u32);

thread_local! {
    static ACTIVE_DOCUMENT_HARNESS: RefCell<Option<ActiveDocumentHarness>> =
        const { RefCell::new(None) };
}

/// One frame of the tree and the pixels rendered from it.
#[derive(Serialize)]
pub struct HarnessSnapshot {
    nodes: Vec<HarnessNode>,
    invalidation: HarnessInvalidation,
    paint_colors: Vec<HarnessPaintColor>,
}

/// What the last frame had to restyle and lay out again.
#[derive(Serialize)]
pub struct HarnessInvalidation {
    restyled_nodes: usize,
    relaid_out_nodes: usize,
    full_document: bool,
}

/// One colour in the rendered frame, and how much of it there is.
#[derive(Serialize)]
pub struct HarnessPaintColor {
    rgba: String,
    pixels: usize,
}

/// One element as Blitz holds it, after the document's scripts have run.
#[derive(Serialize)]
pub struct HarnessNode {
    handle: u64,
    parent: Option<u64>,
    tag: String,
    text_content: String,
    attributes: BTreeMap<String, String>,
    inline_style: String,
    scroll_x: f64,
    scroll_y: f64,
    layout: HarnessLayout,
    /// Present only on `<img>`, where loading state is observable.
    #[serde(skip_serializing_if = "Option::is_none")]
    image: Option<HarnessImage>,
}

/// Observable loading state of an `<img>`.
#[derive(Serialize)]
pub struct HarnessImage {
    natural_width: u32,
    natural_height: u32,
    complete: bool,
    errored: bool,
}

/// A node's border box in CSS pixels.
#[derive(Serialize)]
pub struct HarnessLayout {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

/// Returns the document the last document harness loaded, if one is still live.
///
/// A thread local rather than a returned handle, because the JavaScript suite
/// loads a document in one call and asserts on it in the next.
pub fn active_document_harness() -> Option<ActiveDocumentHarness> {
    ACTIVE_DOCUMENT_HARNESS.with(|active| active.borrow().clone())
}

/// Installs the bridge and runs a document's scripts, as a window does.
///
/// Called for a fresh document and for a reload, which is why it begins by
/// disposing whatever the previous document left on the global object.
pub fn execute_window_scripts<E: JsEngine + 'static>(
    engine: &mut E,
    runtime: DomRuntime,
    scripts: Vec<DocumentScript>,
    entrypoint: &str,
    width: u32,
    height: u32,
    test_harness: bool,
) -> Result<Rc<RefCell<WindowState>>, JsError> {
    execute_window_scripts_from(
        engine,
        runtime,
        scripts,
        entrypoint,
        width,
        height,
        test_harness,
        &blitsen_core::LocalScripts,
        None,
    )
}

/// Installs the bridge and runs a document's scripts, reading external ones
/// through `loader`.
///
/// An exported Phase 2 application has no filesystem to read them from; its
/// loader reads the section appended to the executable instead.
#[allow(clippy::too_many_arguments)]
pub fn execute_window_scripts_from<E: JsEngine + 'static>(
    engine: &mut E,
    runtime: DomRuntime,
    scripts: Vec<DocumentScript>,
    entrypoint: &str,
    width: u32,
    height: u32,
    test_harness: bool,
    loader: &dyn blitsen_core::ScriptLoader,
    reader: Option<crate::app::AppReader>,
) -> Result<Rc<RefCell<WindowState>>, JsError> {
    let module_root = Path::new(entrypoint)
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_string_lossy();
    let module_root =
        serde_json::to_string(&module_root).map_err(|error| JsError::new(error.to_string()))?;
    // Evicting the module cache is the one part of a reload that belongs to the
    // host rather than to the document. Phase 1's module loader is Bun's
    // `require` cache; Phase 2's is Blitsen's own registry, cleared through
    // `__blitsenModuleReset`. Whichever exists is used, and neither is required
    // — a host with no module cache has nothing to evict.
    let cleanup = r#"(() => {
              globalThis.__blitsenDisposeContext?.();
              const baseline = globalThis.__blitsenRuntimeBaseline;
              if (baseline) for (const key of Reflect.ownKeys(globalThis)) {
                if (!baseline.has(key)) try { delete globalThis[key]; } catch {}
              }
              const reloadRoot = __BLITSEN_RELOAD_ROOT__;
              globalThis.__blitsenModuleReset?.(reloadRoot);
              const builtin = globalThis.process?.getBuiltinModule;
              if (!builtin) return;
              const reloadRequire = builtin.call(globalThis.process, "module")
                .createRequire(reloadRoot + "/index.html");
              for (const cached of Object.keys(reloadRequire.cache ?? {})) {
                if (cached === reloadRoot || cached.startsWith(reloadRoot + "/")) delete reloadRequire.cache[cached];
              }
            })()"#
    .replace("__BLITSEN_RELOAD_ROOT__", &module_root);
    engine.evaluate_script(&cleanup, "blitsen:dispose-document-context")?;
    let window_state =
        dom_bridge::install(engine, runtime, width, height, 1.0, test_harness, reader)?;
    engine.evaluate_script(
        r#"(() => {
              if (!globalThis.__blitsenRuntimeBaseline) {
                const baseline = new Set(Reflect.ownKeys(globalThis));
                Object.defineProperty(globalThis, "__blitsenRuntimeBaseline", { value: baseline });
                baseline.add("__blitsenRuntimeBaseline");
              }
            })()"#,
        "blitsen:capture-runtime-globals",
    )?;
    execute_collected_document_scripts_from(scripts, engine, Path::new(entrypoint), loader)?;
    engine.evaluate_script(
        "globalThis.__blitsenDispatchLifecycleEvent('DOMContentLoaded')",
        "blitsen:dom-content-loaded",
    )?;
    Ok(window_state)
}

/// Boots a document at a fixed viewport, installs the bridge, and runs `script`.
///
/// The starting point of every harness below: a laid-out document behind a live
/// bridge, with the script's own mutations already applied.
fn boot_harness_document<E: JsEngine + 'static>(
    mut engine: E,
    html: &str,
    script: &str,
    identifier: &str,
    width: u32,
    height: u32,
) -> Result<(Rc<RefCell<BlitzDom>>, E), JsError> {
    let runtime = DomRuntime::new(BlitzDom::from_html(
        html,
        DocumentConfig {
            viewport: Some(Viewport::new(width, height, 1.0, ColorScheme::Light)),
            ..Default::default()
        },
    ));
    let document = runtime.document();
    document.borrow_mut().flush_layout().map_err(dom_error)?;
    let _window_state = dom_bridge::install(&mut engine, runtime, width, height, 1.0, true, None)?;
    engine.evaluate_script(script, identifier)?;
    Ok((document, engine))
}

/// Boots markup, runs one script, and returns the tree and its PNG.
pub fn execute_bridge_harness<E: JsEngine + 'static>(
    engine: E,
    html: String,
    script: String,
    width: Option<u32>,
    height: Option<u32>,
) -> Result<(HarnessSnapshot, Vec<u8>), JsError> {
    let width = width.unwrap_or(800);
    let height = height.unwrap_or(600);
    let (document, _engine) =
        boot_harness_document(engine, &html, &script, "harness-script.js", width, height)?;
    snapshot_and_render(document, width, height)
}

/// Advances a booted document through a fixed sequence of animation frames.
pub fn execute_animation_harness<E: JsEngine + 'static>(
    engine: E,
    html: String,
    script: String,
    frames: u32,
    width: u32,
    height: u32,
) -> Result<Vec<HarnessSnapshot>, JsError> {
    let (document, mut engine) = boot_harness_document(
        engine,
        &html,
        &script,
        "animation-harness-script.js",
        width,
        height,
    )?;

    let mut snapshots = Vec::with_capacity(frames as usize);
    for frame in 1..=frames {
        let timestamp = f64::from(frame) * (1_000.0 / 60.0);
        engine
            .evaluate_script(
                &format!("globalThis.__blitsenAnimationFrameTick({timestamp})"),
                "blitsen:animation-harness-tick",
            )
            .and_then(|_| engine.drain_microtasks().map(|_| ()))?;
        snapshots.push(snapshot_and_render(Rc::clone(&document), width, height)?.0);
    }
    Ok(snapshots)
}

/// Lays out, rasterizes, and serializes one frame.
pub fn snapshot_and_render(
    document: Rc<RefCell<BlitzDom>>,
    width: u32,
    height: u32,
) -> Result<(HarnessSnapshot, Vec<u8>), JsError> {
    let layout = document.borrow_mut().flush_layout().map_err(dom_error)?;
    let pixels = render_document(&document, width, height);
    let snapshot = snapshot_document(&document, layout, &pixels)?;
    Ok((snapshot, encode_png(&pixels, width, height)?))
}

/// Rasterizes the current layout to RGBA pixels on a white background.
pub fn render_document(document: &Rc<RefCell<BlitzDom>>, width: u32, height: u32) -> Vec<u8> {
    let mut document = document.borrow_mut();
    render_to_buffer::<VelloCpuImageRenderer, _>(
        |scene| {
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
        },
        width,
        height,
    )
}

/// Serializes the tree and the frame that was just rasterized from it.
///
/// Takes the layout token rather than flushing: a second flush would clear the
/// invalidation counters this snapshot is reporting.
pub fn snapshot_document(
    document: &Rc<RefCell<BlitzDom>>,
    snapshot: LayoutSnapshot,
    pixels: &[u8],
) -> Result<HarnessSnapshot, JsError> {
    let (invalidation_metrics, full_document) = document.borrow().last_frame_invalidation();
    let invalidation = HarnessInvalidation {
        restyled_nodes: invalidation_metrics.restyled_nodes,
        relaid_out_nodes: invalidation_metrics.relaid_out_nodes,
        full_document,
    };

    let document = document.borrow();
    let ids = document
        .query_selector_all(document.document(), "*")
        .map_err(dom_error)?;
    let mut nodes = Vec::with_capacity(ids.len());
    for id in ids {
        let node = document
            .document_ref()
            .get_node(id)
            .ok_or_else(|| JsError::new("Blitz returned a stale node"))?;
        let Some(element) = node.element_data() else {
            continue;
        };
        let attributes = element
            .attrs()
            .iter()
            .map(|attribute| {
                (
                    attribute.name.local.to_string(),
                    attribute.value.to_string(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let layout = document.bounding_rect(id, snapshot).map_err(dom_error)?;
        let inline_style = document.inline_style_text(id).map_err(dom_error)?;
        let scroll = *node.scroll_offset();
        nodes.push(HarnessNode {
            handle: id.as_u64(),
            parent: node.parent.map(|parent| parent.as_u64()),
            tag: element.name.local.to_string(),
            text_content: document.text_content(id).map_err(dom_error)?,
            inline_style,
            attributes,
            scroll_x: scroll.x,
            scroll_y: scroll.y,
            layout: HarnessLayout {
                x: layout.x,
                y: layout.y,
                width: layout.width,
                height: layout.height,
            },
            image: document
                .image_state(id, snapshot)
                .ok()
                .map(|state| HarnessImage {
                    natural_width: state.natural_width,
                    natural_height: state.natural_height,
                    complete: state.complete,
                    errored: state.errored,
                }),
        });
    }
    let mut paint_colors = BTreeMap::<[u8; 4], usize>::new();
    for pixel in pixels.chunks_exact(4) {
        *paint_colors
            .entry([pixel[0], pixel[1], pixel[2], pixel[3]])
            .or_default() += 1;
    }
    let mut paint_colors: Vec<_> = paint_colors
        .into_iter()
        .map(|(rgba, pixels)| HarnessPaintColor {
            rgba: format!(
                "#{:02x}{:02x}{:02x}{:02x}",
                rgba[0], rgba[1], rgba[2], rgba[3]
            ),
            pixels,
        })
        .collect();
    paint_colors.sort_unstable_by(|left, right| {
        right
            .pixels
            .cmp(&left.pixels)
            .then_with(|| left.rgba.cmp(&right.rgba))
    });
    paint_colors.truncate(16);
    Ok(HarnessSnapshot {
        nodes,
        invalidation,
        paint_colors,
    })
}

/// Encodes RGBA pixels as a PNG.
pub fn encode_png(pixels: &[u8], width: u32, height: u32) -> Result<Vec<u8>, JsError> {
    let mut png = Vec::new();
    let mut encoder = png::Encoder::new(&mut png, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|error| JsError::new(error.to_string()))?;
    writer
        .write_image_data(pixels)
        .map_err(|error| JsError::new(error.to_string()))?;
    drop(writer);
    Ok(png)
}

/// Loads a real entrypoint the way a window does and snapshots the result.
pub fn execute_document_harness<E: JsEngine + 'static>(
    engine: E,
    entrypoint: &Path,
    width: u32,
    height: u32,
) -> Result<HarnessSnapshot, JsError> {
    // Mirrors a shipped window exactly, injection surface included, so the
    // fixture guard against test-only globals leaking stays meaningful.
    let (_, document) = load_document_harness(engine, entrypoint, width, height, false)?;
    ACTIVE_DOCUMENT_HARNESS.with(|active| {
        *active.borrow_mut() = Some((Rc::clone(&document), width, height));
    });
    snapshot_and_render(document, width, height).map(|(snapshot, _)| snapshot)
}

/// Parses an entrypoint, installs the bridge and runs its document scripts.
pub fn load_document_harness<E: JsEngine + 'static>(
    mut engine: E,
    entrypoint: &Path,
    width: u32,
    height: u32,
    test_harness: bool,
) -> Result<(E, Rc<RefCell<BlitzDom>>), JsError> {
    let source = std::fs::read_to_string(entrypoint).map_err(|error| {
        JsError::new(format!("could not read {}: {error}", entrypoint.display()))
    })?;
    let root = entrypoint.parent().unwrap_or_else(|| Path::new("."));
    let runtime = DomRuntime::new(BlitzDom::from_html(
        &source,
        DocumentConfig {
            base_url: Some(format!("file://{}/", root.display())),
            net_provider: Some(Arc::new(LocalResources) as Arc<dyn NetProvider>),
            viewport: Some(Viewport::new(width, height, 1.0, ColorScheme::Light)),
            ..Default::default()
        },
    ));
    let document = runtime.document();
    let scripts = document.borrow().document_scripts().map_err(dom_error)?;
    execute_window_scripts(
        &mut engine,
        runtime,
        scripts,
        &entrypoint.to_string_lossy(),
        width,
        height,
        test_harness,
    )?;
    engine.evaluate_script(
        "globalThis.__blitsenDispatchLifecycleEvent('load')",
        "blitsen:load",
    )?;
    Ok((engine, document))
}

/// Runs a loaded entrypoint through the frame pipeline, optionally recording
/// every rendered frame as a PNG.
pub fn execute_document_animation_harness<E: JsEngine + Clone + 'static>(
    engine: E,
    entrypoint: &Path,
    setup_script: &str,
    frames: u32,
    width: u32,
    height: u32,
    record_into: Option<&Path>,
) -> Result<Vec<HarnessSnapshot>, JsError> {
    let (mut engine, document) = load_document_harness(engine, entrypoint, width, height, true)?;
    engine.evaluate_script(setup_script, "document-animation-setup.js")?;

    let mut frame_loop =
        frame_loop::FrameLoop::new(engine, Rc::clone(&document), width, height, None);
    let mut snapshots = Vec::with_capacity(frames as usize);
    for frame in 1..=frames {
        frame_loop.advance(frame, f64::from(frame) * (1_000.0 / 60.0))?;
        let layout = frame_loop
            .layout()
            .ok_or_else(|| JsError::new("frame resolved no layout"))?;
        snapshots.push(snapshot_document(&document, layout, frame_loop.pixels())?);
        if let Some(directory) = record_into {
            let png = encode_png(frame_loop.pixels(), width, height)?;
            std::fs::write(directory.join(format!("frame-{frame:05}.png")), &png).map_err(
                |error| JsError::new(format!("could not record frame {frame}: {error}")),
            )?;
        }
    }
    Ok(snapshots)
}
