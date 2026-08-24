//! Native DOM object installation, against whichever engine is hosting.
//!
//! Nothing here names a JavaScript host. Callbacks recover their engine from
//! the value the engine handed them ([`JsEngine::from_value`]), which is the
//! whole of what the Phase 1 addon previously used a captured `napi_env` for.

use std::cell::RefCell;
use std::rc::Rc;

use blitsen_core::{WindowState, WrapperTable};
use blitsen_dom::DomBackend;
use blitsen_js::{
    ExternalId, JsEngine, JsError, JsType, NativeCall, NativeClass, TypedArray, TypedArrayKind,
};
use blitz::dom::NodeId;
use serde_json::{Value, json};

use crate::DomRuntime;

mod audio;
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
mod canvas;
mod net_pool;
mod ops;
mod storage;
mod web_socket;
mod web_url;
pub mod window;
mod window_modes;

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
    include_str!("dom_bridge/bootstrap/gamepad.js"),
    include_str!("dom_bridge/bootstrap/native.js"),
    include_str!("dom_bridge/bootstrap/transfer.js"),
    include_str!("dom_bridge/bootstrap/globals.js"),
    "})();\n",
);

/// The process-wide network pool, for the rest of the host.
///
/// `fetch`, `WebSocket` and the dev server are all a socket being waited on, and
/// one pool is what keeps them from being three sets of parked threads.
pub(crate) fn net_runtime() -> Result<&'static tokio::runtime::Runtime, JsError> {
    net_pool::runtime()
}

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
) -> Result<InstalledDom<E::Value>, JsError> {
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
            let pixels = engine.to_typed_array(pixels)?;
            if !matches!(
                pixels.kind,
                TypedArrayKind::Uint8 | TypedArrayKind::Uint8Clamped
            ) {
                return Err(JsError::new(
                    "viewport surface contents must be a Uint8Array or Uint8ClampedArray",
                ));
            }
            viewport_runtime
                .document
                .borrow_mut()
                .write_native_viewport(node, &pixels.bytes)
                .map_err(crate::dom_error)?;
            Ok(call.this)
        }),
    )?;
    canvas::install(engine, runtime.clone())?;
    install_text_codec(engine)?;
    install_fetch(engine, reader.clone())?;
    install_messaging(engine, reader.clone())?;
    install_audio(engine, reader)?;
    install_web_socket(engine)?;
    install_event_source(engine)?;
    install_intl(engine)?;
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
    let host_hooks = HostHooks::resolve(|name| engine.get_property(&hooks, name))?;

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

/// Installs what a worker's global scope needs from this module.
///
/// A worker gets `fetch` and the UTF-8 codec under it, and nothing else from
/// here: there is no document on its thread and no DOM object may cross to it.
/// URL resolution is a host function of its own because the document's goes
/// through a DOM operation — `resolveUrl` on the tree — which a worker has no
/// tree to ask.
pub fn install_worker_services<E: JsEngine + 'static>(
    engine: &mut E,
    reader: Option<crate::app::AppReader>,
) -> Result<(), JsError> {
    install_text_codec(engine)?;
    install_fetch(engine, reader)?;
    // `Intl` is a language global rather than a document one, so a worker has
    // the same one — and formatting a table of numbers off the main thread is
    // exactly the work a worker is for.
    install_intl(engine)?;
    // The same three facts the document's `navigator` states. A worker has one
    // in a browser, and library code reaches for it to decide what it is running
    // on — Monaco's platform detection gives up without it.
    let navigator = json_value(engine, &navigator_state())?;
    engine.set_global("__blitsenNavigatorState", &navigator)?;
    engine.define_global_function(
        "__blitsenResolveUrl",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let base = argument(&mut engine, &call, 0, "base URL")?;
            let relative = argument(&mut engine, &call, 1, "URL")?;
            let resolved = web_url::resolve(&base, &relative).map_err(JsError::new)?;
            json_value(&mut engine, &resolved)
        }),
    )
}

/// The three facts `navigator` is allowed to state about this machine.
///
/// Identity, never capability: see COMPATIBILITY.md for why the rest of the
/// interface stays absent. The user-agent string names Blitsen rather than
/// impersonating a browser, because an application that sniffs it deserves a
/// true answer more than it deserves a code path written for someone else.
fn navigator_state() -> Value {
    let platform = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", _) => "MacIntel".to_owned(),
        ("windows", _) => "Win32".to_owned(),
        (os, arch) => format!("{}{} {arch}", os[..1].to_uppercase(), &os[1..]),
    };
    // POSIX locales are `en_GB.UTF-8`; BCP 47 is `en-GB`.
    let language = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|name| std::env::var(name).ok())
        .map(|locale| {
            locale
                .split(['.', '@'])
                .next()
                .unwrap_or_default()
                .replace('_', "-")
        })
        .filter(|locale| !locale.is_empty() && locale != "C" && locale != "POSIX")
        .unwrap_or_else(|| "en-US".to_owned());
    json!({
        "userAgent": format!("Blitsen/{} ({platform})", blitsen_core::RELEASE_VERSION),
        "platform": platform,
        "language": language,
    })
}

/// Installs the UTF-8 conversions the body classes need.
///
/// `TextEncoder` and `TextDecoder` are Web IDL, not ECMAScript: relying on the
/// host's would make the request and response bodies change shape under the
/// Phase 2 engine.
fn install_text_codec<E: JsEngine + 'static>(engine: &mut E) -> Result<(), JsError> {
    engine.define_global_function(
        "__blitsenUtf8Encode",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let text = argument(&mut engine, &call, 0, "text")?;
            let bytes = TypedArray::new(TypedArrayKind::Uint8, text.into_bytes())?;
            engine.typed_array(&bytes)
        }),
    )?;
    engine.define_global_function(
        "__blitsenUtf8Decode",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let bytes = call
                .arguments
                .first()
                .ok_or_else(|| JsError::new("missing bytes"))
                .and_then(|value| engine.to_typed_array(value))?;
            // Lossy unless the caller asked otherwise, which is what a body
            // wants: a malformed byte becomes U+FFFD rather than losing the
            // response. `new TextDecoder("utf-8", { fatal: true })` is the one
            // caller that asked to be told instead, and only this side can
            // tell — by the time the string exists the evidence is gone.
            let fatal = match call.arguments.get(1) {
                Some(value) => engine.to_boolean(value)?,
                None => false,
            };
            if fatal {
                let text = String::from_utf8(bytes.bytes)
                    .map_err(|error| JsError::new(format!("invalid UTF-8: {error}")))?;
                return engine.string(&text);
            }
            engine.string(&String::from_utf8_lossy(&bytes.bytes))
        }),
    )
}

/// Installs the audio graph the bootstrap's Web Audio classes call through.
///
/// Nothing here opens a device: the host is created, and the context inside it
/// is not built until an application constructs an `AudioContext`.
/// `BLITSEN_AUDIO_OFFLINE` makes that context an offline one, which is how the
/// harness asserts on rendered samples rather than on the calls that were made.
fn install_audio<E: JsEngine + 'static>(
    engine: &mut E,
    reader: Option<crate::app::AppReader>,
) -> Result<(), JsError> {
    let offline = std::env::var("BLITSEN_AUDIO_OFFLINE").is_ok_and(|value| value == "1");
    let host = Rc::new(audio::AudioHost::new(offline, reader));

    let call_host = Rc::clone(&host);
    engine.define_global_function(
        "__blitsenAudioCall",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let operation = argument(&mut engine, &call, 0, "audio operation")?;
            let arguments = string_arguments(&mut engine, &call, 1)?;
            let result = call_host.dispatch(&operation, &arguments)?;
            json_value(&mut engine, &result)
        }),
    )?;

    let decode_host = Rc::clone(&host);
    engine.define_global_function(
        "__blitsenAudioDecode",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let bytes = call
                .arguments
                .first()
                .ok_or_else(|| JsError::new("missing encoded audio"))
                .and_then(|value| engine.to_typed_array(value))?;
            let id = decode_host.start_decode(bytes.bytes)?;
            Ok(engine.number(id as f64))
        }),
    )?;

    let load_host = Rc::clone(&host);
    engine.define_global_function(
        "__blitsenAudioLoad",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let url = argument(&mut engine, &call, 0, "audio source")?;
            let id = load_host.start_load(&url)?;
            Ok(engine.number(id as f64))
        }),
    )?;

    let poll_host = Rc::clone(&host);
    engine.define_global_function(
        "__blitsenAudioPoll",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            json_value(&mut engine, &poll_host.poll())
        }),
    )?;

    let pending_host = Rc::clone(&host);
    engine.define_global_function(
        "__blitsenAudioPending",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            Ok(engine.boolean(pending_host.pending()))
        }),
    )?;

    let channel_host = Rc::clone(&host);
    engine.define_global_function(
        "__blitsenAudioChannel",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let buffer = argument(&mut engine, &call, 0, "audio buffer id")?
                .parse::<u64>()
                .map_err(|_| JsError::new("invalid audio buffer id"))?;
            let index = argument(&mut engine, &call, 1, "channel index")?
                .parse::<usize>()
                .map_err(|_| JsError::new("invalid channel index"))?;
            let samples = channel_host.channel_data(buffer, index)?;
            let bytes = samples
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .collect();
            engine.typed_array(&TypedArray::new(TypedArrayKind::Float32, bytes)?)
        }),
    )?;

    engine.define_global_function(
        "__blitsenAudioDispose",
        Box::new(move |call| {
            host.dispose();
            Ok(call.this)
        }),
    )
}

/// Installs the transport the bootstrap's `fetch` classes call through.
fn install_fetch<E: JsEngine + 'static>(
    engine: &mut E,
    reader: Option<crate::app::AppReader>,
) -> Result<(), JsError> {
    let host = Rc::new(fetch::FetchHost::new(reader)?);

    let start_host = Rc::clone(&host);
    engine.define_global_function(
        "__blitsenFetchStart",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let spec = argument(&mut engine, &call, 0, "fetch request")?;
            let spec = serde_json::from_str(&spec)
                .map_err(|error| JsError::new(format!("invalid fetch request: {error}")))?;
            let body = match call.arguments.get(1) {
                Some(value) if engine.value_type(value)? == JsType::TypedArray => {
                    Some(engine.to_typed_array(value)?.bytes)
                }
                _ => None,
            };
            let id = start_host.start(&spec, body)?;
            Ok(engine.number(id as f64))
        }),
    )?;

    let poll_host = Rc::clone(&host);
    engine.define_global_function(
        "__blitsenFetchPoll",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            json_value(&mut engine, &poll_host.poll())
        }),
    )?;

    let body_host = Rc::clone(&host);
    engine.define_global_function(
        "__blitsenFetchBody",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let id = fetch_id(&mut engine, &call)?;
            let kind = argument(&mut engine, &call, 1, "body kind")?;
            let bytes = body_host.take_body(id)?;
            match kind.as_str() {
                "text" => engine.string(&String::from_utf8_lossy(&bytes)),
                "bytes" => engine.typed_array(&TypedArray::new(TypedArrayKind::Uint8, bytes)?),
                other => Err(JsError::new(format!("invalid body kind: {other}"))),
            }
        }),
    )?;

    let cancel_host = Rc::clone(&host);
    engine.define_global_function(
        "__blitsenFetchCancel",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            cancel_host.cancel(fetch_id(&mut engine, &call)?);
            Ok(call.this)
        }),
    )?;

    engine.define_global_function(
        "__blitsenFetchDispose",
        Box::new(move |call| {
            host.dispose();
            Ok(call.this)
        }),
    )
}

fn fetch_id<E: JsEngine>(engine: &mut E, call: &NativeCall<E::Value>) -> Result<u64, JsError> {
    argument(engine, call, 0, "request id")?
        .parse::<u64>()
        .map_err(|_| JsError::new("invalid fetch request id"))
}

/// Installs the transport the bootstrap's `WebSocket` class calls through.
fn install_web_socket<E: JsEngine + 'static>(engine: &mut E) -> Result<(), JsError> {
    let host = Rc::new(web_socket::WebSocketHost::new()?);

    let open_host = Rc::clone(&host);
    engine.define_global_function(
        "__blitsenSocketOpen",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let url = argument(&mut engine, &call, 0, "WebSocket address")?;
            let protocols = argument(&mut engine, &call, 1, "WebSocket subprotocols")?;
            let protocols: Vec<String> = serde_json::from_str(&protocols)
                .map_err(|error| JsError::new(format!("invalid subprotocol list: {error}")))?;
            let id = open_host.open(&url, &protocols)?;
            Ok(engine.number(id as f64))
        }),
    )?;

    let text_host = Rc::clone(&host);
    engine.define_global_function(
        "__blitsenSocketSendText",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let id = socket_id(&mut engine, &call)?;
            text_host.send_text(id, argument(&mut engine, &call, 1, "message text")?);
            Ok(call.this)
        }),
    )?;

    let binary_host = Rc::clone(&host);
    engine.define_global_function(
        "__blitsenSocketSendBinary",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let id = socket_id(&mut engine, &call)?;
            let payload = call
                .arguments
                .get(1)
                .ok_or_else(|| JsError::new("missing message payload"))?;
            binary_host.send_binary(id, engine.to_typed_array(payload)?.bytes);
            Ok(call.this)
        }),
    )?;

    let buffered_host = Rc::clone(&host);
    engine.define_global_function(
        "__blitsenSocketBuffered",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let bytes = buffered_host.buffered(socket_id(&mut engine, &call)?);
            Ok(engine.number(bytes as f64))
        }),
    )?;

    let close_host = Rc::clone(&host);
    engine.define_global_function(
        "__blitsenSocketClose",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let id = socket_id(&mut engine, &call)?;
            // An empty code is a close with no status, which is a different
            // frame from one carrying 1005.
            let code = argument(&mut engine, &call, 1, "close code")?;
            let code = match code.as_str() {
                "" => None,
                code => Some(
                    code.parse::<u16>()
                        .map_err(|_| JsError::new("invalid WebSocket close code"))?,
                ),
            };
            close_host.close(id, code, &argument(&mut engine, &call, 2, "close reason")?);
            Ok(call.this)
        }),
    )?;

    let poll_host = Rc::clone(&host);
    engine.define_global_function(
        "__blitsenSocketPoll",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            json_value(&mut engine, &poll_host.poll())
        }),
    )?;

    let payload_host = Rc::clone(&host);
    engine.define_global_function(
        "__blitsenSocketBinary",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let id = socket_id(&mut engine, &call)?;
            let sequence = argument(&mut engine, &call, 1, "message sequence")?
                .parse::<u64>()
                .map_err(|_| JsError::new("invalid WebSocket message sequence"))?;
            let bytes = payload_host.take_binary(id, sequence)?;
            engine.typed_array(&TypedArray::new(TypedArrayKind::Uint8, bytes)?)
        }),
    )?;

    engine.define_global_function(
        "__blitsenSocketDispose",
        Box::new(move |call| {
            host.dispose();
            Ok(call.this)
        }),
    )
}

fn socket_id<E: JsEngine>(engine: &mut E, call: &NativeCall<E::Value>) -> Result<u64, JsError> {
    argument(engine, call, 0, "socket id")?
        .parse::<u64>()
        .map_err(|_| JsError::new("invalid WebSocket id"))
}

/// Installs the transport the bootstrap's `EventSource` class calls through.
fn install_event_source<E: JsEngine + 'static>(engine: &mut E) -> Result<(), JsError> {
    let host = Rc::new(event_source::EventSourceHost::new()?);

    let open_host = Rc::clone(&host);
    engine.define_global_function(
        "__blitsenEventSourceOpen",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let url = argument(&mut engine, &call, 0, "EventSource address")?;
            let id = open_host.open(&url)?;
            Ok(engine.number(id as f64))
        }),
    )?;

    let close_host = Rc::clone(&host);
    engine.define_global_function(
        "__blitsenEventSourceClose",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            close_host.close(stream_id(&mut engine, &call)?);
            Ok(call.this)
        }),
    )?;

    let poll_host = Rc::clone(&host);
    engine.define_global_function(
        "__blitsenEventSourcePoll",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            json_value(&mut engine, &poll_host.poll())
        }),
    )?;

    engine.define_global_function(
        "__blitsenEventSourceDispose",
        Box::new(move |call| {
            host.dispose();
            Ok(call.this)
        }),
    )
}

fn stream_id<E: JsEngine>(engine: &mut E, call: &NativeCall<E::Value>) -> Result<u64, JsError> {
    argument(engine, call, 0, "stream id")?
        .parse::<u64>()
        .map_err(|_| JsError::new("invalid EventSource id"))
}

/// Installs the formatters the bootstrap's `Intl` object calls through.
///
/// Shared with the worker scope through [`install_worker_services`]: `Intl` is
/// a language global rather than a document one, and a worker that formats a
/// number is the ordinary case rather than an exotic one.
pub(crate) fn install_intl<E: JsEngine + 'static>(engine: &mut E) -> Result<(), JsError> {
    let host = Rc::new(intl::IntlHost::default());

    let resolve_host = Rc::clone(&host);
    engine.define_global_function(
        "__blitsenIntlResolve",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let kind = argument(&mut engine, &call, 0, "formatter kind")?;
            let options = argument(&mut engine, &call, 1, "formatter options")?;
            let options: Value = serde_json::from_str(&options)
                .map_err(|error| JsError::new(format!("invalid Intl options: {error}")))?;
            let resolved = resolve_host.resolve(&kind, &options)?;
            json_value(&mut engine, &resolved)
        }),
    )?;

    let format_host = Rc::clone(&host);
    engine.define_global_function(
        "__blitsenIntlFormat",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let handle = intl_handle(&mut engine, &call)?;
            let value = argument(&mut engine, &call, 1, "value")?;
            let formatted = format_host.format(handle, &value)?;
            engine.string(&formatted)
        }),
    )?;

    let select_host = Rc::clone(&host);
    engine.define_global_function(
        "__blitsenIntlSelect",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let handle = intl_handle(&mut engine, &call)?;
            let value = argument(&mut engine, &call, 1, "value")?;
            let category = select_host.select(handle, &value)?;
            engine.string(&category)
        }),
    )?;

    let compare_host = Rc::clone(&host);
    engine.define_global_function(
        "__blitsenIntlCompare",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let handle = intl_handle(&mut engine, &call)?;
            let left = argument(&mut engine, &call, 1, "left string")?;
            let right = argument(&mut engine, &call, 2, "right string")?;
            let ordering = compare_host.compare(handle, &left, &right)?;
            Ok(engine.number(f64::from(ordering)))
        }),
    )?;

    engine.define_global_function(
        "__blitsenIntlJoin",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let handle = intl_handle(&mut engine, &call)?;
            let items = argument(&mut engine, &call, 1, "list items")?;
            let items: Vec<String> = serde_json::from_str(&items)
                .map_err(|error| JsError::new(format!("invalid list: {error}")))?;
            let joined = host.join(handle, &items)?;
            engine.string(&joined)
        }),
    )
}

fn intl_handle<E: JsEngine>(engine: &mut E, call: &NativeCall<E::Value>) -> Result<usize, JsError> {
    argument(engine, call, 0, "formatter handle")?
        .parse::<usize>()
        .map_err(|_| JsError::new("invalid Intl formatter handle"))
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

    type Hooks = HostHooks<<QuickJs as JsEngine>::Value>;

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
        assert_eq!(lookups.values().copied().collect::<Vec<_>>(), vec![1; 11]);
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
