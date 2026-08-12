//! The headless harness surface: what the JavaScript test suite drives.
//!
//! These are `#[napi]` exports rather than a public Rust API; they exist so a
//! test can boot a document, run scripts and read back what was painted.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use anyrender::{PaintScene as _, render_to_buffer};
use anyrender_vello_cpu::VelloCpuImageRenderer;
use base64::Engine as _;
use blitsen_blitz::{BlitzDom, resources::LocalResources};
use blitsen_core::{
    DocumentScript, ScriptDocument, WindowState, WrapperTable, execute_collected_document_scripts,
};
use blitsen_dom::{DomBackend, LayoutSnapshot};
use blitsen_js::{
    ExternalId, JsEngine, JsError, JsType, NativeClass, NativeMethod, TypedArray, TypedArrayKind,
};
use blitz::dom::{DocumentConfig, NodeId, util::Color};
use blitz::paint::paint_scene;
use blitz::traits::net::NetProvider;
use blitz::traits::shell::{ColorScheme, Viewport};
use napi::{Env, Status, sys};
use napi_derive::napi;
use peniko::{Fill, kurbo::Rect};
use serde::Serialize;

#[cfg(target_os = "macos")]
use winit::application::macos::ApplicationHandlerExtMacOS;

use crate::{
    DomRuntime, NodeApiEngine, NodeWeakRef, check, dom_bridge, dom_error, frame_loop, napi_error,
    raw, replay,
};

type ActiveDocumentHarness = (Rc<RefCell<BlitzDom>>, u32, u32);

thread_local! {
    static ACTIVE_DOCUMENT_HARNESS: RefCell<Option<ActiveDocumentHarness>> =
        const { RefCell::new(None) };
}

#[derive(Serialize)]
pub(crate) struct HarnessSnapshot {
    nodes: Vec<HarnessNode>,
    invalidation: HarnessInvalidation,
    paint_colors: Vec<HarnessPaintColor>,
}

#[derive(Serialize)]
pub(crate) struct HarnessInvalidation {
    restyled_nodes: usize,
    relaid_out_nodes: usize,
    full_document: bool,
}

#[derive(Serialize)]
pub(crate) struct HarnessPaintColor {
    rgba: String,
    pixels: usize,
}

#[derive(Serialize)]
pub(crate) struct HarnessNode {
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

#[derive(Serialize)]
pub(crate) struct HarnessImage {
    natural_width: u32,
    natural_height: u32,
    complete: bool,
    errored: bool,
}

#[derive(Serialize)]
pub(crate) struct HarnessLayout {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

pub(crate) fn execute_window_scripts(
    engine: &mut NodeApiEngine,
    runtime: DomRuntime,
    scripts: Vec<DocumentScript>,
    entrypoint: &str,
    width: u32,
    height: u32,
    test_harness: bool,
) -> napi::Result<Rc<RefCell<WindowState>>> {
    let module_root = Path::new(entrypoint)
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_string_lossy();
    let module_root = serde_json::to_string(&module_root)
        .map_err(|error| napi_error(JsError::new(error.to_string())))?;
    let cleanup = r#"(() => {
              globalThis.__blitsenDisposeContext?.();
              const baseline = globalThis.__blitsenRuntimeBaseline;
              if (baseline) for (const key of Reflect.ownKeys(globalThis)) {
                if (!baseline.has(key)) try { delete globalThis[key]; } catch {}
              }
              const reloadRoot = __BLITSEN_RELOAD_ROOT__;
              const reloadRequire = process.getBuiltinModule("module").createRequire(reloadRoot + "/index.html");
              for (const cached of Object.keys(reloadRequire.cache ?? {})) {
                if (cached === reloadRoot || cached.startsWith(reloadRoot + "/")) delete reloadRequire.cache[cached];
              }
            })()"#
    .replace("__BLITSEN_RELOAD_ROOT__", &module_root);
    engine
        .evaluate_script(&cleanup, "blitsen:dispose-document-context")
        .map_err(napi_error)?;
    let window_state = dom_bridge::install(engine, runtime, width, height, 1.0, test_harness)
        .map_err(napi_error)?;
    engine
        .evaluate_script(
            r#"(() => {
              if (!globalThis.__blitsenRuntimeBaseline) {
                const baseline = new Set(Reflect.ownKeys(globalThis));
                Object.defineProperty(globalThis, "__blitsenRuntimeBaseline", { value: baseline });
                baseline.add("__blitsenRuntimeBaseline");
              }
            })()"#,
            "blitsen:capture-runtime-globals",
        )
        .map_err(napi_error)?;
    execute_collected_document_scripts(scripts, engine, Path::new(entrypoint))
        .map_err(napi_error)?;
    engine
        .evaluate_script(
            "globalThis.__blitsenDispatchLifecycleEvent('DOMContentLoaded')",
            "blitsen:dom-content-loaded",
        )
        .map_err(napi_error)?;
    Ok(window_state)
}

/// Boots a document at a fixed viewport, installs the bridge, and runs `script`.
///
/// The starting point of every harness below: a laid-out document behind a live
/// bridge, with the script's own mutations already applied.
fn boot_harness_document(
    env: Env,
    html: &str,
    script: &str,
    identifier: &str,
    width: u32,
    height: u32,
) -> napi::Result<(Rc<RefCell<BlitzDom>>, NodeApiEngine)> {
    let runtime = DomRuntime::new(BlitzDom::from_html(
        html,
        DocumentConfig {
            viewport: Some(Viewport::new(width, height, 1.0, ColorScheme::Light)),
            ..Default::default()
        },
    ));
    let document = runtime.document();
    document.borrow_mut().flush_layout().map_err(dom_error)?;
    let mut engine = NodeApiEngine::new(env);
    let _window_state =
        dom_bridge::install(&mut engine, runtime, width, height, 1.0, true).map_err(napi_error)?;
    engine
        .evaluate_script(script, identifier)
        .map_err(napi_error)?;
    Ok((document, engine))
}

pub(crate) fn execute_bridge_harness(
    env: Env,
    html: String,
    script: String,
    width: Option<u32>,
    height: Option<u32>,
) -> napi::Result<(HarnessSnapshot, Vec<u8>)> {
    let width = width.unwrap_or(800);
    let height = height.unwrap_or(600);
    let (document, _engine) =
        boot_harness_document(env, &html, &script, "harness-script.js", width, height)?;
    snapshot_and_render(document, width, height)
}

pub(crate) fn execute_animation_harness(
    env: Env,
    html: String,
    script: String,
    frames: u32,
    width: u32,
    height: u32,
) -> napi::Result<Vec<HarnessSnapshot>> {
    let (document, mut engine) = boot_harness_document(
        env,
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
            .and_then(|_| engine.drain_microtasks().map(|_| ()))
            .map_err(napi_error)?;
        snapshots.push(snapshot_and_render(Rc::clone(&document), width, height)?.0);
    }
    Ok(snapshots)
}

pub(crate) fn snapshot_and_render(
    document: Rc<RefCell<BlitzDom>>,
    width: u32,
    height: u32,
) -> napi::Result<(HarnessSnapshot, Vec<u8>)> {
    let layout = document.borrow_mut().flush_layout().map_err(dom_error)?;
    let pixels = render_document(&document, width, height);
    let snapshot = snapshot_document(&document, layout, &pixels)?;
    Ok((snapshot, encode_png(&pixels, width, height)?))
}

pub(crate) fn render_document(
    document: &Rc<RefCell<BlitzDom>>,
    width: u32,
    height: u32,
) -> Vec<u8> {
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
pub(crate) fn snapshot_document(
    document: &Rc<RefCell<BlitzDom>>,
    snapshot: LayoutSnapshot,
    pixels: &[u8],
) -> napi::Result<HarnessSnapshot> {
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
        let node = document.document_ref().get_node(id).ok_or_else(|| {
            napi::Error::new(Status::GenericFailure, "Blitz returned a stale node")
        })?;
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

pub(crate) fn encode_png(pixels: &[u8], width: u32, height: u32) -> napi::Result<Vec<u8>> {
    let mut png = Vec::new();
    let mut encoder = png::Encoder::new(&mut png, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|error| napi::Error::new(Status::GenericFailure, error.to_string()))?;
    writer
        .write_image_data(pixels)
        .map_err(|error| napi::Error::new(Status::GenericFailure, error.to_string()))?;
    drop(writer);
    Ok(png)
}

pub(crate) fn execute_document_harness(
    env: Env,
    entrypoint: &Path,
    width: u32,
    height: u32,
) -> napi::Result<HarnessSnapshot> {
    // Mirrors a shipped window exactly, injection surface included, so the
    // fixture guard against test-only globals leaking stays meaningful.
    let (_, document) = load_document_harness(env, entrypoint, width, height, false)?;
    ACTIVE_DOCUMENT_HARNESS.with(|active| {
        *active.borrow_mut() = Some((Rc::clone(&document), width, height));
    });
    snapshot_and_render(document, width, height).map(|(snapshot, _)| snapshot)
}

/// Parses an entrypoint, installs the bridge and runs its document scripts.
pub(crate) fn load_document_harness(
    env: Env,
    entrypoint: &Path,
    width: u32,
    height: u32,
    test_harness: bool,
) -> napi::Result<(NodeApiEngine, Rc<RefCell<BlitzDom>>)> {
    let source = std::fs::read_to_string(entrypoint).map_err(|error| {
        napi::Error::new(
            Status::GenericFailure,
            format!("could not read {}: {error}", entrypoint.display()),
        )
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
    let mut engine = NodeApiEngine::new(env);
    execute_window_scripts(
        &mut engine,
        runtime,
        scripts,
        &entrypoint.to_string_lossy(),
        width,
        height,
        test_harness,
    )?;
    engine
        .evaluate_script(
            "globalThis.__blitsenDispatchLifecycleEvent('load')",
            "blitsen:load",
        )
        .map_err(napi_error)?;
    Ok((engine, document))
}

pub(crate) fn execute_document_animation_harness(
    env: Env,
    entrypoint: &Path,
    setup_script: &str,
    frames: u32,
    width: u32,
    height: u32,
    record_into: Option<&Path>,
) -> napi::Result<Vec<HarnessSnapshot>> {
    let (mut engine, document) = load_document_harness(env, entrypoint, width, height, true)?;
    engine
        .evaluate_script(setup_script, "document-animation-setup.js")
        .map_err(napi_error)?;

    let mut frame_loop =
        frame_loop::FrameLoop::new(engine, Rc::clone(&document), width, height, None);
    let mut snapshots = Vec::with_capacity(frames as usize);
    for frame in 1..=frames {
        frame_loop.advance(frame, f64::from(frame) * (1_000.0 / 60.0))?;
        let layout = frame_loop
            .layout()
            .ok_or_else(|| napi::Error::new(Status::GenericFailure, "frame resolved no layout"))?;
        snapshots.push(snapshot_document(&document, layout, frame_loop.pixels())?);
        if let Some(directory) = record_into {
            let png = encode_png(frame_loop.pixels(), width, height)?;
            std::fs::write(directory.join(format!("frame-{frame:05}.png")), &png).map_err(
                |error| {
                    napi::Error::new(
                        Status::GenericFailure,
                        format!("could not record frame {frame}: {error}"),
                    )
                },
            )?;
        }
    }
    Ok(snapshots)
}

/// Boots Blitz headlessly, runs JavaScript DOM mutations, and returns the Rust
/// tree state as JSON for cross-platform CI assertions.
#[napi]
pub fn run_bridge_harness(
    env: Env,
    html: String,
    script: String,
    width: Option<u32>,
    height: Option<u32>,
) -> napi::Result<String> {
    let (snapshot, _) = execute_bridge_harness(env, html, script, width, height)?;
    serde_json::to_string(&snapshot)
        .map_err(|error| napi::Error::new(Status::GenericFailure, error.to_string()))
}

/// Advances a document through a deterministic sequence of animation frames.
#[napi]
pub fn run_animation_harness(
    env: Env,
    html: String,
    script: String,
    frames: Option<u32>,
    width: Option<u32>,
    height: Option<u32>,
) -> napi::Result<String> {
    let frames = frames.unwrap_or(3);
    if frames > 10_000 {
        return Err(napi::Error::new(
            Status::InvalidArg,
            "animation harness is limited to 10000 frames",
        ));
    }
    let snapshots = execute_animation_harness(
        env,
        html,
        script,
        frames,
        width.unwrap_or(800),
        height.unwrap_or(600),
    )?;
    serde_json::to_string(&snapshots)
        .map_err(|error| napi::Error::new(Status::GenericFailure, error.to_string()))
}

/// Loads a real HTML entrypoint and executes its collected script elements.
#[napi]
pub fn run_document_scripts_harness(
    env: Env,
    entrypoint: String,
    width: Option<u32>,
    height: Option<u32>,
) -> napi::Result<String> {
    let snapshot = execute_document_harness(
        env,
        Path::new(&entrypoint),
        width.unwrap_or(800),
        height.unwrap_or(600),
    )?;
    serde_json::to_string(&snapshot)
        .map_err(|error| napi::Error::new(Status::GenericFailure, error.to_string()))
}

/// Evaluates a script against the most recently loaded document harness.
#[napi]
pub fn evaluate_document_harness(env: Env, script: String) -> napi::Result<()> {
    let active = ACTIVE_DOCUMENT_HARNESS.with(|active| active.borrow().is_some());
    if !active {
        return Err(napi::Error::new(
            Status::GenericFailure,
            "no document harness is active",
        ));
    }
    NodeApiEngine::new(env)
        .evaluate_script(&script, "document-harness-evaluation.js")
        .map(|_| ())
        .map_err(napi_error)
}

/// Snapshots the most recently loaded document after the host event loop has advanced.
#[napi]
pub fn snapshot_document_harness() -> napi::Result<String> {
    let snapshot = ACTIVE_DOCUMENT_HARNESS.with(|active| {
        let active = active.borrow();
        let (document, width, height) = active.as_ref().ok_or_else(|| {
            napi::Error::new(Status::GenericFailure, "no document harness is active")
        })?;
        snapshot_and_render(Rc::clone(document), *width, *height).map(|(snapshot, _)| snapshot)
    })?;
    serde_json::to_string(&snapshot)
        .map_err(|error| napi::Error::new(Status::GenericFailure, error.to_string()))
}

/// Serializes the tree of the most recently loaded document harness.
///
/// Backs the layout conformance corpus, whose framework cases are the markup a
/// real bundle actually built rather than a hand-written imitation of it. The
/// serialization happens after the document's scripts have run, so what comes
/// back is the rendered tree, not the near-empty root element the bundle ships.
#[napi]
pub fn capture_document_harness_html() -> napi::Result<String> {
    ACTIVE_DOCUMENT_HARNESS.with(|active| {
        let active = active.borrow();
        let (document, _, _) = active.as_ref().ok_or_else(|| {
            napi::Error::new(Status::GenericFailure, "no document harness is active")
        })?;
        let document = document.borrow();
        let root = document
            .document_element()
            .ok_or_else(|| napi::Error::new(Status::GenericFailure, "document has no root"))?;
        document.inner_html(root).map_err(dom_error)
    })
}

/// Loads a real HTML entrypoint and advances its animation loop at 60 Hz.
#[napi]
pub fn run_document_animation_harness(
    env: Env,
    entrypoint: String,
    setup_script: String,
    frames: Option<u32>,
    width: Option<u32>,
    height: Option<u32>,
) -> napi::Result<String> {
    let frames = frames.unwrap_or(60);
    if frames > 10_000 {
        return Err(napi::Error::new(
            Status::InvalidArg,
            "document animation harness is limited to 10000 frames",
        ));
    }
    let snapshots = execute_document_animation_harness(
        env,
        Path::new(&entrypoint),
        &setup_script,
        frames,
        width.unwrap_or(800),
        height.unwrap_or(600),
        None,
    )?;
    serde_json::to_string(&snapshots)
        .map_err(|error| napi::Error::new(Status::GenericFailure, error.to_string()))
}

/// Advances a document's animation loop and writes every rendered frame as a PNG.
///
/// Backs the recorded demos in the documentation. The frames are the same ones the
/// acceptance harness asserts on, so a published recording cannot drift away from
/// what the tests actually verify.
#[napi]
pub fn record_document_animation_harness(
    env: Env,
    entrypoint: String,
    setup_script: String,
    directory: String,
    frames: Option<u32>,
    width: Option<u32>,
    height: Option<u32>,
) -> napi::Result<u32> {
    let frames = frames.unwrap_or(60);
    if frames > 10_000 {
        return Err(napi::Error::new(
            Status::InvalidArg,
            "document animation harness is limited to 10000 frames",
        ));
    }
    let directory = PathBuf::from(directory);
    std::fs::create_dir_all(&directory).map_err(|error| {
        napi::Error::new(
            Status::GenericFailure,
            format!("could not create {}: {error}", directory.display()),
        )
    })?;
    execute_document_animation_harness(
        env,
        Path::new(&entrypoint),
        &setup_script,
        frames,
        width.unwrap_or(800),
        height.unwrap_or(600),
        Some(&directory),
    )?;
    Ok(frames)
}

/// Replays a recorded input trace at a fixed timestep.
///
/// Deterministic by construction: JavaScript only ever sees timestamps derived
/// from the trace, never the wall clock, while the wall clock measures what each
/// frame actually cost. The returned report carries a digest sequence to compare
/// against a golden and a frame-time histogram to record.
#[napi]
pub fn replay_document_frames(
    env: Env,
    entrypoint: String,
    trace: String,
    record_into: Option<String>,
    record_frames: Option<Vec<u32>>,
) -> napi::Result<String> {
    let trace = blitsen_core::replay::InputTrace::from_json(&trace)
        .map_err(|error| napi::Error::new(Status::InvalidArg, error.to_string()))?;
    let directory = record_into.map(PathBuf::from);
    if let Some(directory) = &directory {
        std::fs::create_dir_all(directory).map_err(|error| {
            napi::Error::new(
                Status::GenericFailure,
                format!("could not create {}: {error}", directory.display()),
            )
        })?;
    }
    let report = replay::replay(
        env,
        Path::new(&entrypoint),
        trace,
        directory.as_deref(),
        &record_frames.unwrap_or_default(),
    )?;
    serde_json::to_string(&report)
        .map_err(|error| napi::Error::new(Status::GenericFailure, error.to_string()))
}

/// Digests a fixed text-and-shape fixture to identify this machine's rasterizer.
///
/// Pixel-level goldens only mean anything between runs that agree on this, since
/// installed fonts and CPU feature detection both change the bytes that come out.
#[napi]
pub fn render_environment_fingerprint() -> String {
    replay::fingerprint()
}

/// Renders the post-JavaScript frame as a base64-encoded PNG.
#[napi]
pub fn render_bridge_harness_png(
    env: Env,
    html: String,
    script: String,
    width: Option<u32>,
    height: Option<u32>,
) -> napi::Result<String> {
    let (_, png) = execute_bridge_harness(env, html, script, width, height)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(png))
}

/// Exercises the real Node-API weak-reference and finalizer identity path.
#[napi]
pub fn wrapper_identity_smoke(env: Env) -> napi::Result<bool> {
    let mut engine = NodeApiEngine::new(env);
    let class = engine
        .register_class(NativeClass::new("IdentityNode"))
        .map_err(napi_error)?;
    let table = WrapperTable::<NodeId, NodeWeakRef>::new();
    let raw_env = engine.raw_env();
    let weak_map_works = Env::from_raw(raw_env).run_in_scope(|| {
        let node = NodeId::from_u64(1);
        let first = table
            .get_or_create(&mut engine, node, |engine, finalizer| {
                engine.instantiate(&class, ExternalId(node.as_u64()), Some(finalizer))
            })
            .map_err(napi_error)?;
        let second = table
            .get_or_create(&mut engine, node, |_, _| {
                Err(JsError::new("identity table created a duplicate wrapper"))
            })
            .map_err(napi_error)?;
        let mut strictly_equal = false;
        check(
            unsafe {
                sys::napi_strict_equals(raw_env, raw(&first), raw(&second), &mut strictly_equal)
            },
            "compare wrapper identity",
        )
        .map_err(napi_error)?;
        if !strictly_equal {
            return Ok(false);
        }
        engine
            .set_global("__blitsenIdentityFirst", &first)
            .and_then(|_| engine.set_global("__blitsenIdentitySecond", &second))
            .map_err(napi_error)?;
        engine
            .evaluate_script(
                "(() => { const identityMap = new WeakMap([[__blitsenIdentityFirst, 42]]); return identityMap.get(__blitsenIdentitySecond) === 42; })()",
                "blitsen:identity-weak-map",
            )
            .and_then(|value| engine.to_boolean(&value))
            .map_err(napi_error)
    })?;
    if !weak_map_works {
        return Ok(false);
    }

    for slot in 2..=100_001_u64 {
        Env::from_raw(raw_env).run_in_scope(|| {
            let node = NodeId::from_u64(slot);
            table
                .get_or_create(&mut engine, node, |engine, finalizer| {
                    engine.instantiate(&class, ExternalId(node.as_u64()), Some(finalizer))
                })
                .map(|_| ())
                .map_err(napi_error)
        })?;
    }
    if table.len() != 100_001 {
        return Ok(false);
    }
    engine
        .evaluate_script(
            "delete globalThis.__blitsenIdentityFirst; delete globalThis.__blitsenIdentitySecond; Bun.gc(true); Bun.gc(true)",
            "blitsen:identity-gc",
        )
        .map_err(napi_error)?;
    table.prune_collected(&mut engine).map_err(napi_error)?;
    Ok(table.is_empty())
}

/// Runs the load-bearing Node-API subset used by the trait implementation.
///
/// This is exported for the Bun compatibility test and is not public package API.
#[napi]
pub fn node_api_smoke(env: Env) -> napi::Result<bool> {
    let mut engine = NodeApiEngine::new(env);
    let string = engine.string("42").map_err(napi_error)?;
    if engine.to_number(&string).map_err(napi_error)? != 42.0 {
        return Ok(false);
    }
    let one = engine.number(1.0);
    let two = engine.number(2.0);
    let array = engine.array(&[one, two]).map_err(napi_error)?;
    if engine.to_array(&array).map_err(napi_error)?.len() != 2 {
        return Ok(false);
    }
    let typed = TypedArray::new(TypedArrayKind::Uint8, vec![1, 2, 3]).map_err(napi_error)?;
    let typed = engine.typed_array(&typed).map_err(napi_error)?;
    if engine.to_typed_array(&typed).map_err(napi_error)?.bytes != [1, 2, 3] {
        return Ok(false);
    }
    let result = engine
        .evaluate_script("21 * 2", "smoke.js")
        .and_then(|value| engine.to_number(&value))
        .map_err(napi_error)?;
    if result != 42.0 {
        return Ok(false);
    }

    let identity = engine
        .define_function("identity", Box::new(|call| Ok(call.arguments[0])))
        .map_err(napi_error)?;
    let argument = engine.string("callback").map_err(napi_error)?;
    let result = engine
        .call(&identity, None, &[argument])
        .and_then(|value| engine.to_string(&value))
        .map_err(napi_error)?;
    if result != "callback" {
        return Ok(false);
    }

    let class = engine
        .register_class(NativeClass::new("SmokeNode").with_method(NativeMethod::new(
            "identity",
            Box::new(|call| Ok(call.this)),
        )))
        .map_err(napi_error)?;
    let instance = engine
        .instantiate(&class, ExternalId(42), None)
        .map_err(napi_error)?;
    if engine.external_id(&instance).map_err(napi_error)? != ExternalId(42) {
        return Ok(false);
    }
    let method = engine
        .get_property(&instance, "identity")
        .map_err(napi_error)?;
    engine
        .call(&method, Some(&instance), &[])
        .map_err(napi_error)?;
    let weak = engine.downgrade(&instance).map_err(napi_error)?;
    if engine.upgrade(&weak).map_err(napi_error)?.is_none() {
        return Ok(false);
    }

    let global_value = engine.string("visible").map_err(napi_error)?;
    engine
        .set_global("__blitsenSmoke", &global_value)
        .map_err(napi_error)?;
    let global_result = engine
        .evaluate_script("globalThis.__blitsenSmoke", "global-smoke.js")
        .and_then(|value| engine.to_string(&value))
        .map_err(napi_error)?;
    if global_result != "visible" {
        return Ok(false);
    }

    let document = engine.object().map_err(napi_error)?;
    let mut window_state = WindowState::new(800, 600, 2.0);
    let window = window_state
        .install(&mut engine, &document)
        .map_err(napi_error)?;
    let window_check = engine
        .evaluate_script(
            "window === globalThis && window.document !== undefined && innerWidth === 800 && innerHeight === 600 && devicePixelRatio === 2 && !('location' in window) && !('history' in window) && !('navigator' in window) && !('localStorage' in window)",
            "window-smoke.js",
        )
        .and_then(|value| engine.to_boolean(&value))
        .map_err(napi_error)?;
    if !window_check {
        return Ok(false);
    }
    window_state.resize(1024, 768);
    window_state
        .sync(&mut engine, &window)
        .map_err(napi_error)?;
    let resized = engine
        .evaluate_script(
            "innerWidth === 1024 && innerHeight === 768",
            "resize-smoke.js",
        )
        .and_then(|value| engine.to_boolean(&value))
        .map_err(napi_error)?;
    if !resized {
        return Ok(false);
    }

    let throwing = engine
        .define_function(
            "throwing",
            Box::new(|_| Err(JsError::new("native callback failed"))),
        )
        .map_err(napi_error)?;
    let error = match engine.call(&throwing, None, &[]) {
        Ok(_) => return Ok(false),
        Err(error) => error,
    };
    if !error.message().contains("native callback failed") {
        return Ok(false);
    }

    let module = engine
        .evaluate_module("export const answer = 42", "smoke-module.js")
        .map_err(napi_error)?;
    if engine.value_type(&module).map_err(napi_error)? != JsType::Object {
        return Ok(false);
    }
    engine.drain_microtasks().map_err(napi_error)?;
    engine.pump_event_loop().map_err(napi_error)?;
    Ok(true)
}
