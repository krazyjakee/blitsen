//! Native DOM object installation, against whichever engine is hosting.
//!
//! Nothing here names a JavaScript host. Callbacks recover their engine from
//! the value the engine handed them ([`JsEngine::from_value`]), so no callback
//! holds a captured environment handle.

use std::cell::RefCell;
use std::rc::Rc;

use blitsen_core::{WindowState, WrapperTable};
use blitsen_dom::DomBackend;
use blitsen_js::{ExternalId, JsEngine, JsError, JsType, NativeCall, NativeClass, TypedArrayKind};
use blitz::dom::NodeId;
use serde_json::Value;

use crate::DomRuntime;

mod audio;
mod canvas;
mod command_channel;
mod event_source;
mod fetch;
pub(crate) mod gamepad;
pub(crate) mod hid;
pub(crate) mod input;
mod intl;
// Compiled where there is an application menu to queue requests for — see
// `native_window/menu.rs` — and in the test build everywhere, because the
// public FIFO shape this settles is not a platform decision and a queue only
// two targets could compile would be a queue nothing here checks.
#[cfg(any(target_os = "windows", target_os = "macos", test))]
pub(crate) mod menu;
mod native;
pub(crate) mod notify;
pub(crate) mod tray;
// The thread pool the network runs on. Not a web worker — those are
// [`crate::worker`], and the two were one name for long enough to be worth
// spelling out.
pub(crate) mod net_pool;
mod ops;
mod storage;
mod web_socket;
mod web_url;
pub mod window;
mod window_modes;
mod worker_services;

pub use worker_services::install_worker_services;
use worker_services::{install_text_codec, navigator_state};

// The DOM runtime the application sees, evaluated into the context before any
// document script runs. It is a single closure so the objects can share the
// bridge handle and their wrapper tables privately, which is why the source is
// spliced together here rather than loaded as modules: the fragments below are
// consecutive slices of one scope and are only valid in this order.
const BOOTSTRAP: &str = concat!(
    "\n(() => {\n",
    include_str!("dom_bridge/bootstrap/members.js"),
    include_str!("dom_bridge/bootstrap/prelude.js"),
    include_str!("dom_bridge/bootstrap/events.js"),
    include_str!("dom_bridge/bootstrap/event_target.js"),
    include_str!("dom_bridge/bootstrap/node.js"),
    include_str!("dom_bridge/bootstrap/element.js"),
    include_str!("dom_bridge/bootstrap/cssom.js"),
    include_str!("dom_bridge/bootstrap/forms.js"),
    include_str!("dom_bridge/bootstrap/canvas.js"),
    include_str!("dom_bridge/bootstrap/canvas_context.js"),
    include_str!("dom_bridge/bootstrap/canvas_element.js"),
    include_str!("dom_bridge/bootstrap/text_editing.js"),
    include_str!("dom_bridge/bootstrap/document.js"),
    include_str!("dom_bridge/bootstrap/window_modes.js"),
    include_str!("dom_bridge/bootstrap/range.js"),
    include_str!("dom_bridge/bootstrap/fetch.js"),
    include_str!("dom_bridge/bootstrap/web_socket.js"),
    include_str!("dom_bridge/bootstrap/event_source.js"),
    include_str!("dom_bridge/bootstrap/intl.js"),
    include_str!("dom_bridge/bootstrap/clone.js"),
    include_str!("dom_bridge/bootstrap/messaging.js"),
    include_str!("dom_bridge/bootstrap/audio.js"),
    include_str!("dom_bridge/bootstrap/history.js"),
    include_str!("dom_bridge/bootstrap/url.js"),
    include_str!("dom_bridge/bootstrap/storage.js"),
    include_str!("dom_bridge/bootstrap/command_channel.js"),
    include_str!("dom_bridge/bootstrap/gamepad.js"),
    include_str!("dom_bridge/bootstrap/native.js"),
    include_str!("dom_bridge/bootstrap/transfer.js"),
    include_str!("dom_bridge/bootstrap/globals.js"),
    "})();\n",
);

/// Whether a document receives only application globals or test-only helpers too.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DocumentMode {
    /// An application window or headless production-equivalent document.
    Application,
    /// A test document with bridge counters and synthetic input helpers.
    TestHarness,
}

impl DocumentMode {
    fn is_test_harness(self) -> bool {
        matches!(self, Self::TestHarness)
    }
}

/// Everything that varies between bridge installations.
pub struct InstallOptions {
    width: u32,
    height: u32,
    device_pixel_ratio: f64,
    mode: DocumentMode,
    reader: Option<crate::app::AppReader>,
    storage: Option<crate::storage::LocalStorage>,
}

/// JavaScript callbacks retained by the host without publishing them on the
/// application global object. Test harnesses additionally expose their named
/// synthetic injectors, but still retain this private set so document loading
/// follows the same ownership path in every mode.
pub(crate) struct HostHooks<V> {
    pub(crate) mouse: V,
    pub(crate) pointer: V,
    pub(crate) keyboard: V,
    pub(crate) ime: V,
    pub(crate) locked_pointer_motion: V,
    pub(crate) release_window_modes: V,
    pub(crate) drag: V,
    pub(crate) lifecycle: V,
    pub(crate) animation_frame_tick: V,
    pub(crate) animation_frames_pending: V,
    pub(crate) replay_keyboard: V,
    pub(crate) inject_pointer_at: V,
    pub(crate) window: V,
}

impl<V> HostHooks<V> {
    fn resolve(mut property: impl FnMut(&str) -> Result<V, JsError>) -> Result<Self, JsError> {
        Ok(Self {
            mouse: property("mouse")?,
            pointer: property("pointer")?,
            keyboard: property("keyboard")?,
            ime: property("ime")?,
            locked_pointer_motion: property("lockedPointerMotion")?,
            release_window_modes: property("releaseWindowModes")?,
            drag: property("drag")?,
            lifecycle: property("lifecycle")?,
            animation_frame_tick: property("animationFrameTick")?,
            animation_frames_pending: property("animationFramesPending")?,
            replay_keyboard: property("replayKeyboard")?,
            inject_pointer_at: property("injectPointerAt")?,
            window: property("window")?,
        })
    }
}

/// Observable window state plus the private native-to-DOM dispatch boundary.
pub(crate) struct InstalledDom<V> {
    pub(crate) window_state: Rc<RefCell<WindowState>>,
    pub(crate) host_hooks: HostHooks<V>,
}

impl InstallOptions {
    /// Describes one bridge installation without positional flags.
    pub fn new(
        width: u32,
        height: u32,
        device_pixel_ratio: f64,
        mode: DocumentMode,
        reader: Option<crate::app::AppReader>,
    ) -> Self {
        Self {
            width,
            height,
            device_pixel_ratio,
            mode,
            reader,
            storage: None,
        }
    }

    /// Supplies the durable store for this application realm.
    pub fn with_storage(mut self, storage: crate::storage::LocalStorage) -> Self {
        self.storage = Some(storage);
        self
    }
}

/// Installs the real DOM object graph into a JavaScript environment.
pub fn install<E: JsEngine + 'static>(
    engine: &mut E,
    runtime: DomRuntime,
    options: InstallOptions,
) -> Result<Rc<RefCell<WindowState>>, JsError> {
    Ok(install_with_hooks(engine, runtime, options)?.window_state)
}

/// Installs the DOM and returns the private host callbacks a native window
/// needs. Kept crate-private so the public embedding API retains its original
/// window-state return type and cannot accidentally leak the callbacks.
pub(crate) fn install_with_hooks<E: JsEngine + 'static>(
    engine: &mut E,
    runtime: DomRuntime,
    options: InstallOptions,
) -> Result<InstalledDom<E::StrongRef>, JsError> {
    let InstallOptions {
        width,
        height,
        device_pixel_ratio,
        mode,
        // Issue #125: how `fetch` and a media source read a file the application
        // shipped. `None` is the bare bridge harness, which has no application
        // behind it — and is why `fetch` still refuses a `file:` URL there.
        reader,
        storage,
    } = options;
    let class = Rc::new(engine.register_class(NativeClass::new("BlitsenNode"))?);
    let table = Rc::new(WrapperTable::<NodeId, E::WeakRef>::new());

    let wrapper_runtime = runtime.clone();
    let wrapper_table = Rc::clone(&table);
    let wrapper_class = Rc::clone(&class);
    engine.define_global_function(
        "__blitsenWrap",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let handle = argument(&mut engine, &call, 0, "node handle")?;
            let node = wrapper_runtime.resolve_handle(&handle)?;
            wrapper_table.get_or_create(&mut engine, node, |engine, table_finalizer| {
                wrapper_runtime.retain_handle(&handle)?;
                let finalizer_runtime = wrapper_runtime.clone();
                let finalizer_handle = handle.clone();
                let finalizer = Box::new(move |external| {
                    table_finalizer(external);
                    let _ = finalizer_runtime.release_handle(&finalizer_handle);
                });
                match engine.instantiate(&wrapper_class, ExternalId(node.as_u64()), Some(finalizer))
                {
                    Ok(wrapper) => Ok(wrapper),
                    Err(error) => {
                        let _ = wrapper_runtime.release_handle(&handle);
                        Err(error)
                    }
                }
            })
        }),
    )?;

    let dispatch_runtime = runtime.clone();
    engine.define_global_function(
        "__blitsenDomCall",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let operation = argument(&mut engine, &call, 0, "operation")?;
            let arguments = string_arguments(&mut engine, &call, 1)?;
            let result = ops::dispatch(&dispatch_runtime, &operation, &arguments)?;
            json_value(&mut engine, &result)
        }),
    )?;
    let default_scroll_runtime = runtime.clone();
    engine.define_global_function(
        "__blitsenScrollDefault",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let handle = argument(&mut engine, &call, 0, "scroll target")?;
            let delta_x = argument(&mut engine, &call, 1, "horizontal scroll delta")?
                .parse::<f64>()
                .map_err(|_| JsError::new("invalid horizontal scroll delta"))?;
            let delta_y = argument(&mut engine, &call, 2, "vertical scroll delta")?
                .parse::<f64>()
                .map_err(|_| JsError::new("invalid vertical scroll delta"))?;
            let node = default_scroll_runtime.resolve_handle(&handle)?;
            let mut document = default_scroll_runtime.document.borrow_mut();
            document.flush_layout().map_err(crate::dom_error)?;
            document
                .document_mut()
                .scroll_node_by(node, delta_x, delta_y, |_| {});
            Ok(call.this)
        }),
    )?;
    let viewport_runtime = runtime.clone();
    engine.define_global_function(
        "__blitsenViewportWrite",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let handle = argument(&mut engine, &call, 0, "viewport handle")?;
            let node = viewport_runtime.resolve_handle(&handle)?;
            let pixels = call
                .arguments
                .get(1)
                .ok_or_else(|| JsError::new("viewport surface contents are required"))?;
            let pixels = byte_argument(&mut engine, pixels, "viewport surface contents")?;
            viewport_runtime
                .document
                .borrow_mut()
                .write_native_viewport(node, &pixels)
                .map_err(crate::dom_error)?;
            Ok(call.this)
        }),
    )?;
    canvas::install(engine, runtime.clone())?;
    install_text_codec(engine)?;
    fetch::install(engine, reader.clone())?;
    install_messaging(engine, reader.clone())?;
    audio::install(engine, reader)?;
    web_socket::install(engine)?;
    event_source::install(engine)?;
    intl::install(engine)?;
    storage::install(engine, storage)?;
    gamepad::install(engine)?;
    window_modes::install(engine, mode.is_test_harness())?;
    native::install(engine)?;
    let dev_layout_warnings = std::env::var("BLITSEN_DEV_LAYOUT_WARNINGS").is_ok_and(|value| {
        !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
    });
    let dev_layout_warnings = engine.boolean(dev_layout_warnings);
    engine.set_global("__blitsenDevLayoutWarnings", &dev_layout_warnings)?;
    let navigator = json_value(engine, &navigator_state())?;
    engine.set_global("__blitsenNavigatorState", &navigator)?;
    let test_harness = engine.boolean(mode.is_test_harness());
    engine.set_global("__blitsenTestHarness", &test_harness)?;
    let hooks = engine.evaluate_script(BOOTSTRAP, "blitsen:dom-bootstrap")?;
    let host_hooks = HostHooks::resolve(|name| {
        let hook = engine.get_property(&hooks, name)?;
        engine.retain(&hook)
    })?;

    let document = engine.evaluate_script("globalThis.document", "blitsen:document-value")?;
    let window_state = Rc::new(RefCell::new(WindowState::new(
        width,
        height,
        device_pixel_ratio,
    )));
    window_state.borrow().install(engine, &document)?;
    engine.evaluate_script(
        "globalThis.__blitsenInstallReplacedGlobals()",
        "blitsen:install-replaced-globals",
    )?;
    let preferences_document = runtime.document();
    let resize_state = Rc::clone(&window_state);
    let resize_runtime = runtime;
    engine.define_global_function(
        "__blitsenWindowResize",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let width = argument(&mut engine, &call, 0, "viewport width")?
                .parse::<u32>()
                .map_err(|_| JsError::new("invalid viewport width"))?;
            let height = argument(&mut engine, &call, 1, "viewport height")?
                .parse::<u32>()
                .map_err(|_| JsError::new("invalid viewport height"))?;
            resize_state.borrow_mut().resize(width, height);
            let mut document = resize_runtime.document.borrow_mut();
            let mut viewport = document.document_ref().viewport().clone();
            viewport.window_size = (width, height);
            document.document_mut().set_viewport(viewport);
            drop(document);
            let window = engine.evaluate_script("globalThis", "blitsen:window-resize-target")?;
            resize_state.borrow().sync(&mut engine, &window)?;
            engine.evaluate_script(
                "globalThis.__blitsenDispatchLifecycleEvent('resize')",
                "blitsen:test-window-resize",
            )?;
            Ok(call.this)
        }),
    )?;
    // The system preferences the media features follow, settable from the
    // harness and the native window alike. Takes effect at the next frame
    // boundary, which is where `notifyMediaQueries` reads it back.
    engine.define_global_function(
        "__blitsenMediaPreferences",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let color_scheme = match argument(&mut engine, &call, 0, "colour scheme")?.as_str() {
                "light" => blitsen_dom::ColorScheme::Light,
                "dark" => blitsen_dom::ColorScheme::Dark,
                other => {
                    return Err(JsError::new(format!(
                        "{other:?} is not a colour scheme: light or dark"
                    )));
                }
            };
            let reduced_motion =
                match argument(&mut engine, &call, 1, "motion preference")?.as_str() {
                    "reduce" => true,
                    "no-preference" => false,
                    other => {
                        return Err(JsError::new(format!(
                            "{other:?} is not a motion preference: reduce or no-preference"
                        )));
                    }
                };
            preferences_document
                .borrow_mut()
                .set_media_preferences(blitsen_dom::MediaPreferences {
                    color_scheme,
                    reduced_motion,
                })
                .map_err(crate::dom_error)?;
            Ok(call.this)
        }),
    )?;
    Ok(InstalledDom {
        window_state,
        host_hooks,
    })
}

/// Installs the document's ports, channels and workers.
///
/// The application's files reach a worker through here: a worker loads its
/// script out of the same application the document did, so a context with no
/// files behind it — the bare bridge harness — can hold ports and channels but
/// has no script to start a worker from, and says so at the constructor.
fn install_messaging<E: JsEngine + 'static>(
    engine: &mut E,
    reader: Option<crate::app::AppReader>,
) -> Result<(), JsError> {
    let files = reader.map(|reader| crate::messaging::WorkerFiles {
        source: reader.source(),
        reader: Some(reader),
    });
    let host = Rc::new(crate::messaging::MessagingHost::new(
        crate::ports::registry().new_context(),
        files,
    ));
    crate::messaging::install(engine, &host)
}

/// Reads a required string argument, refusing a value that is not one.
///
/// Deliberately not string coercion: the bootstrap is the only caller, it
/// always passes strings, and a coercing read would turn a bridge bug into
/// `"undefined"` reaching Blitz as an attribute value.
pub(crate) fn argument<E: JsEngine>(
    engine: &mut E,
    call: &NativeCall<E::Value>,
    index: usize,
    name: &str,
) -> Result<String, JsError> {
    string_value(engine, call.argument(index, name)?)
}

/// Reads a byte buffer, refusing any typed array other than the two that hold
/// bytes; `what` names the buffer in the error, as the caller's message did.
fn byte_argument<E: JsEngine>(
    engine: &mut E,
    value: &E::Value,
    what: &str,
) -> Result<Vec<u8>, JsError> {
    let array = engine.to_typed_array(value)?;
    if !matches!(
        array.kind,
        TypedArrayKind::Uint8 | TypedArrayKind::Uint8Clamped
    ) {
        return Err(JsError::new(format!(
            "{what} must be a Uint8Array or Uint8ClampedArray"
        )));
    }
    Ok(array.bytes)
}

fn string_value<E: JsEngine>(engine: &mut E, value: &E::Value) -> Result<String, JsError> {
    if engine.value_type(value)? != JsType::String {
        return Err(JsError::new("bridge argument is not a string"));
    }
    engine.to_string(value)
}

fn string_arguments<E: JsEngine>(
    engine: &mut E,
    call: &NativeCall<E::Value>,
    from: usize,
) -> Result<Vec<String>, JsError> {
    let mut arguments = Vec::with_capacity(call.arguments.len().saturating_sub(from));
    for index in from..call.arguments.len() {
        arguments.push(string_value(engine, &call.arguments[index])?);
    }
    Ok(arguments)
}

pub(crate) fn json_value<E: JsEngine>(engine: &mut E, value: &Value) -> Result<E::Value, JsError> {
    let value = serde_json::to_string(value).map_err(|error| JsError::new(error.to_string()))?;
    engine.string(&value)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use blitsen_blitz::BlitzDom;
    use blitsen_js::JsEngine;
    use blitsen_quickjs::QuickJs;
    use blitz::dom::DocumentConfig;
    use blitz::traits::shell::{ColorScheme, Viewport};

    use super::*;

    type Hooks = HostHooks<<QuickJs as JsEngine>::StrongRef>;

    fn realm() -> (
        QuickJs,
        crate::runtime_services::RuntimeServices<QuickJs>,
        Hooks,
    ) {
        let mut engine = QuickJs::new().expect("an engine");
        let services = crate::runtime_services::RuntimeServices::install(&mut engine)
            .expect("runtime services");
        let dom = BlitzDom::from_html(
            "<!doctype html><html><body></body></html>",
            DocumentConfig {
                viewport: Some(Viewport::new(200, 100, 1.0, ColorScheme::Light)),
                ..Default::default()
            },
        );
        let installed = install_with_hooks(
            &mut engine,
            crate::DomRuntime::new(dom),
            InstallOptions::new(200, 100, 1.0, DocumentMode::TestHarness, None),
        )
        .expect("the DOM bridge installs");
        (engine, services, installed.host_hooks)
    }

    fn number(engine: &mut QuickJs, source: &str) -> f64 {
        let value = engine
            .evaluate_script(source, "blitsen:cached-hook-test")
            .expect("the probe evaluates");
        engine.to_number(&value).expect("the probe is numeric")
    }

    #[test]
    fn host_hooks_are_resolved_once_and_then_retained() {
        let mut lookups = BTreeMap::new();
        let hooks = HostHooks::resolve(|name| {
            *lookups.entry(name.to_owned()).or_insert(0) += 1;
            Ok(name.to_owned())
        })
        .expect("all hooks resolve");

        for _ in 0..4 {
            assert_eq!(hooks.keyboard, "keyboard");
            assert_eq!(hooks.animation_frame_tick, "animationFrameTick");
            assert_eq!(hooks.animation_frames_pending, "animationFramesPending");
        }
        assert_eq!(lookups.values().copied().collect::<Vec<_>>(), vec![1; 13]);
    }

    #[test]
    fn retained_input_hook_parses_json_without_evaluating_it_as_source() {
        let (mut engine, _services, hooks) = realm();
        engine
            .evaluate_script(
                "globalThis.__seenKey = null; document.body.addEventListener('keydown', event => __seenKey = event.key)",
                "blitsen:cached-input-setup",
            )
            .expect("the listener installs");
        let serialized = engine
            .string(r#"["keydown",{"key":"'); throw new Error('compiled') //","code":"KeyA"}]"#)
            .expect("the input is a string");

        let allowed = engine
            .call(&hooks.keyboard, None, &[serialized])
            .expect("the cached hook accepts serialized input");
        assert!(engine.to_boolean(&allowed).expect("the result is boolean"));
        let seen = engine
            .evaluate_script("globalThis.__seenKey", "blitsen:cached-input-result")
            .expect("the observed key is readable");
        assert_eq!(
            engine.to_string(&seen).expect("the key is text"),
            "'); throw new Error('compiled') //"
        );
    }

    #[test]
    fn retained_replay_hooks_preserve_input_fields_and_order() {
        let (mut engine, _services, hooks) = realm();
        engine
            .evaluate_script(
                r#"
                globalThis.__replayed = [];
                for (const type of ["keydown", "pointermove"]) document.addEventListener(type, event => {
                  __replayed.push(type === "keydown"
                    ? [event.type, event.key, event.code, event.repeat, event.bubbles, event.cancelable]
                    : [event.type, event.clientX, event.clientY, event.screenX, event.screenY]);
                });
                "#,
                "blitsen:replay-hook-setup",
            )
            .unwrap();

        let keyboard = engine.retained_value(&hooks.replay_keyboard).unwrap();
        let arguments = crate::frame_loop::replay_keyboard_arguments(
            &mut engine,
            "keydown",
            "'\\\n雪",
            "Quote",
            true,
        )
        .unwrap();
        engine.call(&keyboard, None, &arguments).unwrap();

        let pointer = engine.retained_value(&hooks.inject_pointer_at).unwrap();
        let arguments =
            crate::frame_loop::replay_pointer_arguments(&mut engine, "pointermove", 12.25, -0.0)
                .unwrap();
        engine.call(&pointer, None, &arguments).unwrap();

        let observed = engine
            .evaluate_script("JSON.stringify(__replayed)", "blitsen:replay-hook-result")
            .unwrap();
        assert_eq!(
            engine.to_string(&observed).unwrap(),
            r#"[["keydown","'\\\n雪","Quote",true,true,true],["pointermove",12.25,0,12.25,0]]"#
        );
    }

    #[test]
    fn animation_tick_does_not_repeat_the_turn_pending_query() {
        let (mut engine, _services, hooks) = realm();
        let before = number(
            &mut engine,
            "globalThis.__blitsenDomCallCount('isAnimating')",
        );

        for timestamp in [1.0, 2.0, 3.0] {
            let timestamp = engine.number(timestamp);
            engine
                .call(&hooks.animation_frame_tick, None, &[timestamp])
                .expect("the cached frame tick runs");
            engine
                .call(&hooks.animation_frames_pending, None, &[])
                .expect("the cached pending query runs");
        }

        let after = number(
            &mut engine,
            "globalThis.__blitsenDomCallCount('isAnimating')",
        );
        assert_eq!(after - before, 3.0);
    }

    #[test]
    fn windowed_steady_state_contains_no_script_evaluation() {
        assert!(!include_str!("native_window.rs").contains("evaluate_script"));
        assert!(!include_str!("native_window/input.rs").contains("evaluate_script"));
    }
}
