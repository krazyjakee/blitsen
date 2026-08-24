//! The headless harness core: what the JavaScript test suite drives.
//!
//! Not a public Rust API. It exists so a test can boot a document, run scripts
//! and read back what was painted — through whichever engine is hosting, so the
//! same assertions run against Phase 1 and Phase 2 (issue #90).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use anyrender::{PaintScene as _, render_to_buffer};
use anyrender_vello_cpu::VelloCpuImageRenderer;
use blitsen_blitz::{BlitzDom, resources::LocalResources};
use blitsen_core::{DocumentScript, execute_collected_document_scripts_from};
use blitsen_dom::{DomBackend, LayoutSnapshot, Rect as DomRect};
use blitsen_js::{JsEngine, JsError};
use blitz::dom::{Attribute, DocumentConfig, ElementData, Node, NodeId, util::Color};
use blitz::paint::paint_scene;
use blitz::traits::net::NetProvider;
use blitz::traits::shell::{ColorScheme, Viewport};
use peniko::{Fill, kurbo::Rect};
use serde::Serialize;

#[cfg(target_os = "macos")]
use winit::application::macos::ApplicationHandlerExtMacOS;

use crate::dom_bridge::{DocumentMode, InstallOptions};
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

/// One resolved element in the document's selector order.
pub(crate) struct ElementView<'a> {
    document: &'a BlitzDom,
    id: NodeId,
    node: &'a Node,
    element: &'a ElementData,
}

impl ElementView<'_> {
    pub(crate) fn tag(&self) -> &str {
        self.element.name.local.as_ref()
    }

    pub(crate) fn attributes(&self) -> impl Iterator<Item = &Attribute> {
        self.element.attrs().iter()
    }

    pub(crate) fn inline_style(&self) -> Result<String, JsError> {
        self.document.inline_style_text(self.id).map_err(dom_error)
    }

    pub(crate) fn text_content(&self) -> Result<String, JsError> {
        self.document.text_content(self.id).map_err(dom_error)
    }

    pub(crate) fn bounding_rect(&self, snapshot: LayoutSnapshot) -> Result<DomRect, JsError> {
        self.document
            .bounding_rect(self.id, snapshot)
            .map_err(dom_error)
    }
}

/// Visits every element in the order returned by Blitz's universal selector.
pub(crate) fn visit_elements(
    document: &BlitzDom,
    mut visit: impl FnMut(ElementView<'_>) -> Result<(), JsError>,
) -> Result<(), JsError> {
    let ids = document
        .query_selector_all(document.document(), "*")
        .map_err(dom_error)?;
    for id in ids {
        let node = document
            .document_ref()
            .get_node(id)
            .ok_or_else(|| JsError::new("Blitz returned a stale node"))?;
        let Some(element) = node.element_data() else {
            continue;
        };
        visit(ElementView {
            document,
            id,
            node,
            element,
        })?;
    }
    Ok(())
}

/// Returns the document the last document harness loaded, if one is still live.
///
/// A thread local rather than a returned handle, because the JavaScript suite
/// loads a document in one call and asserts on it in the next.
pub fn active_document_harness() -> Option<ActiveDocumentHarness> {
    ACTIVE_DOCUMENT_HARNESS.with(|active| active.borrow().clone())
}

/// Installs the bridge and runs a document's scripts, as a window does, reading
/// external ones through `loader`.
///
/// Called for a fresh document and for a reload, which is why it begins by
/// disposing whatever the previous document left on the global object. An
/// exported Phase 2 application has no filesystem to read the external scripts
/// from; its loader reads the section appended to the executable instead.
/// Inputs that belong to one document-script execution rather than its engine.
pub(crate) struct WindowScriptOptions<'a> {
    pub(crate) entrypoint: &'a str,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) mode: DocumentMode,
    pub(crate) loader: &'a dyn blitsen_core::ScriptLoader,
    pub(crate) reader: Option<crate::app::AppReader>,
    pub(crate) storage: Option<crate::storage::LocalStorage>,
}

pub(crate) fn execute_window_scripts_from<E: JsEngine + 'static>(
    engine: &mut E,
    runtime: DomRuntime,
    scripts: Vec<DocumentScript>,
    options: WindowScriptOptions<'_>,
) -> Result<dom_bridge::InstalledDom<E::Value>, JsError> {
    let WindowScriptOptions {
        entrypoint,
        width,
        height,
        mode,
        loader,
        reader,
        storage,
    } = options;
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
    let mut install = InstallOptions::new(width, height, 1.0, mode, reader);
    if let Some(storage) = storage {
        install = install.with_storage(storage);
    }
    let installed = dom_bridge::install_with_hooks(engine, runtime, install)?;
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
    let results =
        execute_collected_document_scripts_from(scripts, engine, Path::new(entrypoint), loader)?;
    // A module's evaluation is a promise, so an exception at the top level of a
    // document script is a rejection rather than an `Err` above. Unobserved, it
    // is a blank window and nothing on either stream — the application simply
    // never ran. A classic script's result is not a promise and falls through
    // the optional calls untouched.
    for result in results {
        engine.set_global("__blitsenDocumentModule", &result)?;
        engine.evaluate_script(
            "globalThis.__blitsenDocumentModule?.then?.(undefined, globalThis.reportError); \
             delete globalThis.__blitsenDocumentModule;",
            "blitsen:document-module-result",
        )?;
    }
    engine.evaluate_script(
        "globalThis.__blitsenDispatchLifecycleEvent('DOMContentLoaded')",
        "blitsen:dom-content-loaded",
    )?;
    Ok(installed)
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
    let _window_state = dom_bridge::install(
        &mut engine,
        runtime,
        InstallOptions::new(width, height, 1.0, DocumentMode::TestHarness, None),
    )?;
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
    let mut nodes = Vec::new();
    visit_elements(&document, |element| {
        let attributes = element
            .attributes()
            .map(|attribute| {
                (
                    attribute.name.local.to_string(),
                    attribute.value.to_string(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let layout = element.bounding_rect(snapshot)?;
        let inline_style = element.inline_style()?;
        let scroll = *element.node.scroll_offset();
        nodes.push(HarnessNode {
            handle: element.id.as_u64(),
            parent: element.node.parent.map(|parent| parent.as_u64()),
            tag: element.tag().to_owned(),
            text_content: element.text_content()?,
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
            image: element
                .document
                .image_state(element.id, snapshot)
                .ok()
                .map(|state| HarnessImage {
                    natural_width: state.natural_width,
                    natural_height: state.natural_height,
                    complete: state.complete,
                    errored: state.errored,
                }),
        });
        Ok(())
    })?;
    let mut paint_colors = BTreeMap::<[u8; 4], usize>::new();
    for pixel in pixels.as_chunks::<4>().0 {
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

/// Encodes and writes one numbered frame, returning the path that was written.
pub(crate) fn record_frame(
    directory: &Path,
    frame: u32,
    pixels: &[u8],
    width: u32,
    height: u32,
) -> Result<PathBuf, JsError> {
    let path = directory.join(format!("frame-{frame:05}.png"));
    std::fs::write(&path, encode_png(pixels, width, height)?)
        .map_err(|error| JsError::new(format!("could not record frame {frame}: {error}")))?;
    Ok(path)
}

/// Loads a real entrypoint the way a window does and snapshots the result.
pub fn execute_document_harness<E: JsEngine + Clone + 'static>(
    engine: E,
    entrypoint: &Path,
    width: u32,
    height: u32,
) -> Result<HarnessSnapshot, JsError> {
    // Mirrors a shipped window exactly, including the absence of test-only
    // injection globals, so the fixture guard against them stays meaningful.
    let (_, document) =
        load_document_harness(engine, entrypoint, width, height, DocumentMode::Application)?;
    ACTIVE_DOCUMENT_HARNESS.with(|active| {
        *active.borrow_mut() = Some((Rc::clone(&document), width, height));
    });
    snapshot_and_render(document, width, height).map(|(snapshot, _)| snapshot)
}

/// Parses an entrypoint, installs the bridge and runs its document scripts.
///
/// The directory is opened as an application rather than as loose files, which
/// is the same thing a window does with it (`app::load_document`). It used to
/// read them off disk directly, and that made every headless path — the
/// standalone check an exported application answers, `--replay`, the fixtures —
/// disagree with the window beside it: a module ran under a `file:` identifier
/// instead of an application URL, so `new URL("./data.json", import.meta.url)`
/// named a path `fetch` refuses, and the Phase 1 export failed the read that the
/// Phase 2 one completed (#126, #90).
pub fn load_document_harness<E: JsEngine + Clone + 'static>(
    mut engine: E,
    entrypoint: &Path,
    width: u32,
    height: u32,
    mode: DocumentMode,
) -> Result<(E, Rc<RefCell<BlitzDom>>), JsError> {
    let files = crate::app::AppFiles::directory(entrypoint)?;
    let net_provider = files
        .net_provider()
        .unwrap_or_else(|| Arc::new(LocalResources) as Arc<dyn NetProvider>);
    let loaded = crate::app::load_document(
        &mut engine,
        &files,
        net_provider,
        crate::app::LoadOptions::new(width, height, mode),
    )?;
    engine.evaluate_script(
        "globalThis.__blitsenDispatchLifecycleEvent('load')",
        "blitsen:load",
    )?;
    Ok((engine, loaded.document))
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
    let (mut engine, document) =
        load_document_harness(engine, entrypoint, width, height, DocumentMode::TestHarness)?;
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
            record_frame(directory, frame, frame_loop.pixels(), width, height)?;
        }
    }
    Ok(snapshots)
}

#[cfg(test)]
mod tests {
    use blitsen_quickjs::QuickJs;

    use super::*;

    fn ime_document(html: &str) -> (QuickJs, Rc<RefCell<BlitzDom>>) {
        let dom = BlitzDom::from_html(
            html,
            DocumentConfig {
                viewport: Some(Viewport::new(400, 160, 1.0, ColorScheme::Light)),
                ..Default::default()
            },
        );
        let runtime = DomRuntime::new(dom);
        let document = runtime.document();
        let mut engine = QuickJs::new().expect("a QuickJS runtime");
        let _services = crate::runtime_services::RuntimeServices::install(&mut engine)
            .expect("the embedded runtime services install");
        dom_bridge::install(
            &mut engine,
            runtime,
            InstallOptions::new(400, 160, 1.0, DocumentMode::TestHarness, None),
        )
        .expect("the DOM bridge installs");
        document
            .borrow_mut()
            .flush_layout()
            .expect("the controls lay out");
        (engine, document)
    }

    #[test]
    fn element_view_preserves_tree_and_attribute_order() {
        let mut document = BlitzDom::from_html(
            "<main data-last=2 id=first>one<span class=inner>two</span></main>",
            DocumentConfig {
                viewport: Some(Viewport::new(320, 200, 1.0, ColorScheme::Light)),
                ..Default::default()
            },
        );
        let layout = document.flush_layout().expect("the fixture lays out");
        let mut seen = Vec::new();
        visit_elements(&document, |element| {
            if matches!(element.tag(), "main" | "span") {
                let attributes = element
                    .attributes()
                    .map(|attribute| {
                        (
                            attribute.name.local.to_string(),
                            attribute.value.to_string(),
                        )
                    })
                    .collect::<Vec<_>>();
                seen.push((
                    element.tag().to_owned(),
                    attributes,
                    element.inline_style()?,
                    element.text_content()?,
                    element.bounding_rect(layout)?,
                ));
            }
            Ok(())
        })
        .expect("the elements resolve");

        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].0, "main");
        assert_eq!(
            seen[0].1,
            [
                ("data-last".into(), "2".into()),
                ("id".into(), "first".into())
            ]
        );
        assert_eq!(seen[0].2, "");
        assert_eq!(seen[0].3, "onetwo");
        assert!(seen[0].4.width > 0.0);
        assert_eq!(seen[1].0, "span");
        assert_eq!(seen[1].3, "two");
    }

    #[test]
    fn record_frame_owns_the_filename_png_and_write_error() {
        let directory = tempfile::tempdir().expect("a scratch directory");

        let pixels = [0x12, 0x34, 0x56, 0xff];
        let path = record_frame(directory.path(), 12, &pixels, 1, 1).expect("the frame records");
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("frame-00012.png")
        );
        assert!(
            std::fs::read(&path)
                .expect("the PNG is readable")
                .starts_with(&[0x89, b'P', b'N', b'G'])
        );

        let blocker = directory.path().join("not-a-directory");
        std::fs::write(&blocker, []).expect("the blocker is written");
        let error =
            record_frame(&blocker, 13, &pixels, 1, 1).expect_err("writing below a file fails");
        assert!(error.message().starts_with("could not record frame 13:"));
    }

    #[test]
    fn synthetic_ime_events_keep_dom_order_state_and_is_composing() {
        let (mut engine, _) = ime_document("<input id=field>");
        engine
            .evaluate_script(
                r#"
                globalThis.field = document.getElementById("field");
                globalThis.imeLog = [];
                for (const type of ["compositionstart", "compositionupdate", "beforeinput",
                                    "input", "compositionend"]) {
                  field.addEventListener(type, event => imeLog.push({
                    type: event.type,
                    data: event.data,
                    inputType: event instanceof InputEvent ? event.inputType : "",
                    isComposing: event instanceof InputEvent ? event.isComposing : null,
                    value: field.value,
                  }));
                }
                field.focus();
                "#,
                "blitsen:test-ime-listeners",
            )
            .unwrap();
        for script in [
            r#"__blitsenDispatchImeEvent("preedit", { data: "🙂", cursorStart: 4, cursorEnd: 4 })"#,
            r#"__blitsenDispatchImeEvent("preedit", { data: "🙂🙂", cursorStart: 4, cursorEnd: 8 })"#,
            r#"__blitsenDispatchImeEvent("commit", { data: "🙂" })"#,
        ] {
            engine
                .evaluate_script(script, "blitsen:test-ime-event")
                .unwrap();
        }
        let result = engine
            .evaluate_script(
                "JSON.stringify({ log: imeLog, value: field.value, start: field.selectionStart })",
                "blitsen:test-ime-result",
            )
            .and_then(|value| engine.to_string(&value))
            .unwrap();
        let result: serde_json::Value = serde_json::from_str(&result).unwrap();

        assert_eq!(
            result["log"],
            serde_json::json!([
                {"type":"compositionstart", "data":"", "inputType":"", "isComposing":null, "value":""},
                {"type":"compositionupdate", "data":"🙂", "inputType":"", "isComposing":null, "value":""},
                {"type":"beforeinput", "data":"🙂", "inputType":"insertCompositionText", "isComposing":true, "value":""},
                {"type":"input", "data":"🙂", "inputType":"insertCompositionText", "isComposing":true, "value":"🙂"},
                {"type":"compositionupdate", "data":"🙂🙂", "inputType":"", "isComposing":null, "value":"🙂"},
                {"type":"beforeinput", "data":"🙂🙂", "inputType":"insertCompositionText", "isComposing":true, "value":"🙂"},
                {"type":"input", "data":"🙂🙂", "inputType":"insertCompositionText", "isComposing":true, "value":"🙂🙂"},
                {"type":"beforeinput", "data":"🙂", "inputType":"insertFromComposition", "isComposing":true, "value":"🙂🙂"},
                {"type":"input", "data":"🙂", "inputType":"insertFromComposition", "isComposing":true, "value":"🙂"},
                {"type":"compositionend", "data":"🙂", "inputType":"", "isComposing":null, "value":"🙂"},
            ])
        );
        assert_eq!(result["value"], "🙂");
        assert_eq!(result["start"], 2, "DOM selection offsets remain UTF-16");
    }

    #[test]
    fn focus_change_cancels_preedit_and_readonly_never_starts_one() {
        let (mut engine, _) = ime_document(
            "<input id=field><input id=locked readonly><textarea id=notes></textarea>",
        );
        let result = engine
            .evaluate_script(
                r#"
                const field = document.getElementById("field");
                const locked = document.getElementById("locked");
                const notes = document.getElementById("notes");
                const ended = [];
                field.addEventListener("compositionend", event => ended.push(event.data));
                field.focus();
                __blitsenDispatchImeEvent("preedit", { data: "draft", cursorStart: 5, cursorEnd: 5 });
                notes.focus();
                locked.focus();
                const readonlyHandled = __blitsenDispatchImeEvent("preedit",
                  { data: "blocked", cursorStart: 7, cursorEnd: 7 });
                notes.focus();
                __blitsenDispatchImeEvent("preedit", { data: "ok", cursorStart: 2, cursorEnd: 2 });
                __blitsenDispatchImeEvent("commit", { data: "ok" });
                field.focus();
                __blitsenDispatchImeEvent("preedit",
                  { data: "cancel", cursorStart: 6, cursorEnd: 6 });
                __blitsenDispatchImeEvent("preedit", { data: "" });
                JSON.stringify({ field: field.value, notes: notes.value, locked: locked.value,
                                 ended, readonlyHandled });
                "#,
                "blitsen:test-ime-focus",
            )
            .and_then(|value| engine.to_string(&value))
            .unwrap();
        let result: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(
            result,
            serde_json::json!({
                "field": "",
                "notes": "ok",
                "locked": "",
                "ended": ["", ""],
                "readonlyHandled": false,
            })
        );
    }

    #[test]
    fn text_history_restores_unicode_values_selections_and_ime_transactions() {
        let (mut engine, _) =
            ime_document(r#"<input id=field value="A🙂B"><textarea id=notes>line</textarea>"#);
        let result = engine
            .evaluate_script(
                r#"
                const field = document.getElementById("field");
                const notes = document.getElementById("notes");
                const historyLog = [];
                for (const type of ["beforeinput", "input"]) {
                  notes.addEventListener(type, event => {
                    if (event.inputType.startsWith("history")) historyLog.push({
                      type, inputType: event.inputType, data: event.data,
                      isComposing: event.isComposing, value: notes.value,
                      start: notes.selectionStart, end: notes.selectionEnd,
                      direction: notes.selectionDirection,
                    });
                  });
                }
                const key = (key, options = {}) => __blitsenDispatchKeyboardEvent("keydown",
                  { key, code: `Key${key.toUpperCase()}`, ...options });

                notes.focus();
                notes.value = "A🙂B";
                notes.setSelectionRange(1, 3, "backward");
                __blitsenDispatchImeEvent("preedit",
                  { data: "n", cursorStart: 1, cursorEnd: 1 });
                __blitsenDispatchImeEvent("preedit",
                  { data: "ni", cursorStart: 2, cursorEnd: 2 });
                __blitsenDispatchImeEvent("commit", { data: "你" });
                const committed = { value: notes.value, start: notes.selectionStart,
                                    end: notes.selectionEnd };
                key("z", { ctrlKey: true });
                const undone = { value: notes.value, start: notes.selectionStart,
                                 end: notes.selectionEnd,
                                 direction: notes.selectionDirection };
                key("z", { metaKey: true, shiftKey: true });
                const redone = { value: notes.value, start: notes.selectionStart,
                                 end: notes.selectionEnd };

                field.focus();
                field.setSelectionRange(1, 3, "forward");
                key("界");
                key("z", { ctrlKey: true });
                const inputUndone = { value: field.value, start: field.selectionStart,
                                     end: field.selectionEnd,
                                     direction: field.selectionDirection };
                JSON.stringify({ committed, undone, redone, inputUndone, historyLog });
                "#,
                "blitsen:test-text-history-unicode",
            )
            .and_then(|value| engine.to_string(&value))
            .unwrap();
        let result: serde_json::Value = serde_json::from_str(&result).unwrap();

        assert_eq!(
            result["committed"],
            serde_json::json!({"value":"A你B", "start":2, "end":2})
        );
        assert_eq!(
            result["undone"],
            serde_json::json!({"value":"A🙂B", "start":1, "end":3, "direction":"backward"})
        );
        assert_eq!(
            result["redone"],
            serde_json::json!({"value":"A你B", "start":2, "end":2})
        );
        assert_eq!(
            result["inputUndone"],
            serde_json::json!({"value":"A🙂B", "start":1, "end":3, "direction":"forward"})
        );
        assert_eq!(
            result["historyLog"],
            serde_json::json!([
                {"type":"beforeinput", "inputType":"historyUndo", "data":null,
                 "isComposing":false, "value":"A你B", "start":2, "end":2, "direction":"none"},
                {"type":"input", "inputType":"historyUndo", "data":null,
                 "isComposing":false, "value":"A🙂B", "start":1, "end":3,
                 "direction":"backward"},
                {"type":"beforeinput", "inputType":"historyRedo", "data":null,
                 "isComposing":false, "value":"A🙂B", "start":1, "end":3,
                 "direction":"backward"},
                {"type":"input", "inputType":"historyRedo", "data":null,
                 "isComposing":false, "value":"A你B", "start":2, "end":2, "direction":"none"},
            ])
        );
    }

    #[test]
    fn text_history_has_control_local_bounded_and_controlled_value_boundaries() {
        let (mut engine, _) =
            ime_document("<input id=field><input id=other><textarea id=bounded></textarea>");
        let result = engine
            .evaluate_script(
                r#"
                const field = document.getElementById("field");
                const other = document.getElementById("other");
                const bounded = document.getElementById("bounded");
                const key = (key, options = {}) => __blitsenDispatchKeyboardEvent("keydown",
                  { key, code: `Key${key.toUpperCase()}`, ...options });

                // A controlled component's same-value echo retains history.
                field.addEventListener("input", event => {
                  if (event.inputType === "insertText") field.value = field.value;
                });
                let cancelHistory = true;
                field.addEventListener("beforeinput", event => {
                  if (cancelHistory && event.inputType === "historyUndo") event.preventDefault();
                });
                field.focus();
                key("a");
                key("b");
                key("z", { ctrlKey: true });
                const canceledUndo = field.value;
                cancelHistory = false;
                other.focus();
                field.focus();
                key("z", { ctrlKey: true });
                const focusRetained = field.value;
                key("y", { ctrlKey: true });
                const ctrlYRedone = field.value;
                key("z", { ctrlKey: true });
                key("x");
                key("y", { ctrlKey: true });
                const branchClearedRedo = field.value;
                field.value = "controlled";
                key("z", { ctrlKey: true });
                const replacementClearedHistory = field.value;

                bounded.focus();
                for (let index = 0; index < 105; index++) key("x");
                for (let index = 0; index < 101; index++) key("z", { ctrlKey: true });
                JSON.stringify({ canceledUndo, focusRetained, ctrlYRedone, branchClearedRedo,
                                 replacementClearedHistory, bounded: bounded.value.length });
                "#,
                "blitsen:test-text-history-boundaries",
            )
            .and_then(|value| engine.to_string(&value))
            .unwrap();
        let result: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(
            result,
            serde_json::json!({
                "canceledUndo": "ab",
                "focusRetained": "a",
                "ctrlYRedone": "ab",
                "branchClearedRedo": "ax",
                "replacementClearedHistory": "controlled",
                "bounded": 5,
            })
        );
    }
}
