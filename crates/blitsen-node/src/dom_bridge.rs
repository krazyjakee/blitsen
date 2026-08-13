//! Native DOM object installation for the Bun host.

use std::cell::RefCell;
use std::rc::Rc;

use blitsen_core::{WindowState, WrapperTable};
use blitsen_dom::DomBackend;
use blitsen_js::{ExternalId, JsEngine, JsError, JsType, NativeClass, TypedArray, TypedArrayKind};
use blitz::dom::NodeId;
use napi::{Env, Unknown, sys};
use serde_json::{Value, json};

use super::{DomRuntime, NodeApiEngine, NodeWeakRef, callback_string, check, unknown};

mod audio;
mod fetch;
mod native;
mod ops;
mod web_socket;
mod web_url;
pub(crate) mod window;
mod worker;

// The DOM runtime the application sees, evaluated into the context before any
// document script runs. It is a single closure so the objects can share the
// bridge handle and their wrapper tables privately, which is why the source is
// spliced together here rather than loaded as modules: the fragments below are
// consecutive slices of one scope and are only valid in this order.
const BOOTSTRAP: &str = concat!(
    "\n(() => {\n",
    include_str!("dom_bridge/bootstrap/prelude.js"),
    include_str!("dom_bridge/bootstrap/events.js"),
    include_str!("dom_bridge/bootstrap/event_target.js"),
    include_str!("dom_bridge/bootstrap/node.js"),
    include_str!("dom_bridge/bootstrap/element.js"),
    include_str!("dom_bridge/bootstrap/cssom.js"),
    include_str!("dom_bridge/bootstrap/forms.js"),
    include_str!("dom_bridge/bootstrap/document.js"),
    include_str!("dom_bridge/bootstrap/fetch.js"),
    include_str!("dom_bridge/bootstrap/web_socket.js"),
    include_str!("dom_bridge/bootstrap/audio.js"),
    include_str!("dom_bridge/bootstrap/history.js"),
    include_str!("dom_bridge/bootstrap/storage.js"),
    include_str!("dom_bridge/bootstrap/native.js"),
    include_str!("dom_bridge/bootstrap/globals.js"),
    "})();\n",
);

/// Installs the real DOM object graph into a Node-API JavaScript environment.
pub(super) fn install(
    engine: &mut NodeApiEngine,
    runtime: DomRuntime,
    width: u32,
    height: u32,
    device_pixel_ratio: f64,
    test_harness: bool,
) -> Result<Rc<RefCell<WindowState>>, JsError> {
    let class = Rc::new(engine.register_class(NativeClass::new("BlitsenNode"))?);
    let table = Rc::new(WrapperTable::<NodeId, NodeWeakRef>::new());
    let raw_env = engine.raw_env();

    let wrapper_runtime = runtime.clone();
    let wrapper_table = Rc::clone(&table);
    let wrapper_class = Rc::clone(&class);
    let wrap_function = engine.define_function(
        "__blitsenWrap",
        Box::new(move |call| {
            let handle = argument(&call.arguments, 0, "node handle")?;
            let node = wrapper_runtime.resolve_handle(&handle)?;
            let mut callback_engine = NodeApiEngine::new(Env::from_raw(raw_env));
            wrapper_table.get_or_create(&mut callback_engine, node, |engine, table_finalizer| {
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
    engine.set_global("__blitsenWrap", &wrap_function)?;

    let dispatch_runtime = runtime.clone();
    let call_function = engine.define_function(
        "__blitsenDomCall",
        Box::new(move |call| {
            let operation = argument(&call.arguments, 0, "operation")?;
            let arguments = call
                .arguments
                .iter()
                .skip(1)
                .map(callback_string)
                .collect::<Result<Vec<_>, _>>()?;
            let result = ops::dispatch(&dispatch_runtime, &operation, &arguments)?;
            json_string(raw_env, &result)
        }),
    )?;
    engine.set_global("__blitsenDomCall", &call_function)?;
    let default_scroll_runtime = runtime.clone();
    let default_scroll_function = engine.define_function(
        "__blitsenScrollDefault",
        Box::new(move |call| {
            let handle = argument(&call.arguments, 0, "scroll target")?;
            let delta_x = argument(&call.arguments, 1, "horizontal scroll delta")?
                .parse::<f64>()
                .map_err(|_| JsError::new("invalid horizontal scroll delta"))?;
            let delta_y = argument(&call.arguments, 2, "vertical scroll delta")?
                .parse::<f64>()
                .map_err(|_| JsError::new("invalid vertical scroll delta"))?;
            let node = default_scroll_runtime.resolve_handle(&handle)?;
            let mut document = default_scroll_runtime.document.borrow_mut();
            document
                .flush_layout()
                .map_err(|error| JsError::new(error.to_string()))?;
            document
                .document_mut()
                .scroll_node_by(node, delta_x, delta_y, |_| {});
            Ok(call.this)
        }),
    )?;
    engine.set_global("__blitsenScrollDefault", &default_scroll_function)?;
    let viewport_runtime = runtime.clone();
    let viewport_write_function = engine.define_function(
        "__blitsenViewportWrite",
        Box::new(move |call| {
            let handle = argument(&call.arguments, 0, "viewport handle")?;
            let node = viewport_runtime.resolve_handle(&handle)?;
            let pixels = call
                .arguments
                .get(1)
                .ok_or_else(|| JsError::new("viewport surface contents are required"))?;
            let mut callback_engine = NodeApiEngine::new(Env::from_raw(raw_env));
            let pixels = callback_engine.to_typed_array(pixels)?;
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
                .map_err(|error| JsError::new(error.to_string()))?;
            Ok(call.this)
        }),
    )?;
    engine.set_global("__blitsenViewportWrite", &viewport_write_function)?;
    install_text_codec(engine, raw_env)?;
    install_fetch(engine, raw_env)?;
    install_audio(engine, raw_env)?;
    install_web_socket(engine, raw_env)?;
    native::install(engine, raw_env)?;
    let dev_layout_warnings = std::env::var("BLITSEN_DEV_LAYOUT_WARNINGS").is_ok_and(|value| {
        !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
    });
    let dev_layout_warnings = engine.boolean(dev_layout_warnings);
    engine.set_global("__blitsenDevLayoutWarnings", &dev_layout_warnings)?;
    let navigator = json_string(raw_env, &navigator_state())?;
    engine.set_global("__blitsenNavigatorState", &navigator)?;
    let test_harness = engine.boolean(test_harness);
    engine.set_global("__blitsenTestHarness", &test_harness)?;
    engine.evaluate_script(BOOTSTRAP, "blitsen:dom-bootstrap")?;

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
    let resize_function = engine.define_function(
        "__blitsenWindowResize",
        Box::new(move |call| {
            let width = argument(&call.arguments, 0, "viewport width")?
                .parse::<u32>()
                .map_err(|_| JsError::new("invalid viewport width"))?;
            let height = argument(&call.arguments, 1, "viewport height")?
                .parse::<u32>()
                .map_err(|_| JsError::new("invalid viewport height"))?;
            resize_state.borrow_mut().resize(width, height);
            let mut document = resize_runtime.document.borrow_mut();
            let mut viewport = document.document_ref().viewport().clone();
            viewport.window_size = (width, height);
            document.document_mut().set_viewport(viewport);
            drop(document);
            let mut callback_engine = NodeApiEngine::new(Env::from_raw(raw_env));
            let window =
                callback_engine.evaluate_script("globalThis", "blitsen:window-resize-target")?;
            resize_state.borrow().sync(&mut callback_engine, &window)?;
            callback_engine.evaluate_script(
                "globalThis.__blitsenDispatchLifecycleEvent('resize')",
                "blitsen:test-window-resize",
            )?;
            Ok(call.this)
        }),
    )?;
    engine.set_global("__blitsenWindowResize", &resize_function)?;
    Ok(window_state)
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
        "userAgent": format!("Blitsen/{} ({platform})", env!("CARGO_PKG_VERSION")),
        "platform": platform,
        "language": language,
    })
}

/// Installs the UTF-8 conversions the body classes need.
///
/// `TextEncoder` and `TextDecoder` are Web IDL, not ECMAScript: relying on the
/// host's would make the request and response bodies change shape under the
/// Phase 2 engine.
fn install_text_codec(engine: &mut NodeApiEngine, raw_env: sys::napi_env) -> Result<(), JsError> {
    let encode = engine.define_function(
        "__blitsenUtf8Encode",
        Box::new(move |call| {
            let text = argument(&call.arguments, 0, "text")?;
            let bytes = TypedArray::new(TypedArrayKind::Uint8, text.into_bytes())?;
            NodeApiEngine::new(Env::from_raw(raw_env)).typed_array(&bytes)
        }),
    )?;
    engine.set_global("__blitsenUtf8Encode", &encode)?;
    let decode = engine.define_function(
        "__blitsenUtf8Decode",
        Box::new(move |call| {
            let mut engine = NodeApiEngine::new(Env::from_raw(raw_env));
            let bytes = call
                .arguments
                .first()
                .ok_or_else(|| JsError::new("missing bytes"))
                .and_then(|value| engine.to_typed_array(value))?;
            engine.string(&String::from_utf8_lossy(&bytes.bytes))
        }),
    )?;
    engine.set_global("__blitsenUtf8Decode", &decode)
}

/// Installs the audio graph the bootstrap's Web Audio classes call through.
///
/// Nothing here opens a device: the host is created, and the context inside it
/// is not built until an application constructs an `AudioContext`.
/// `BLITSEN_AUDIO_OFFLINE` makes that context an offline one, which is how the
/// harness asserts on rendered samples rather than on the calls that were made.
fn install_audio(engine: &mut NodeApiEngine, raw_env: sys::napi_env) -> Result<(), JsError> {
    let offline = std::env::var("BLITSEN_AUDIO_OFFLINE").is_ok_and(|value| value == "1");
    let host = Rc::new(audio::AudioHost::new(offline));

    let call_host = Rc::clone(&host);
    let call = engine.define_function(
        "__blitsenAudioCall",
        Box::new(move |call| {
            let operation = argument(&call.arguments, 0, "audio operation")?;
            let arguments = call
                .arguments
                .iter()
                .skip(1)
                .map(callback_string)
                .collect::<Result<Vec<_>, _>>()?;
            let result = call_host.dispatch(&operation, &arguments)?;
            json_string(raw_env, &result)
        }),
    )?;
    engine.set_global("__blitsenAudioCall", &call)?;

    let decode_host = Rc::clone(&host);
    let decode = engine.define_function(
        "__blitsenAudioDecode",
        Box::new(move |call| {
            let mut engine = NodeApiEngine::new(Env::from_raw(raw_env));
            let bytes = call
                .arguments
                .first()
                .ok_or_else(|| JsError::new("missing encoded audio"))
                .and_then(|value| engine.to_typed_array(value))?;
            let id = decode_host.start_decode(bytes.bytes)?;
            Ok(engine.number(id as f64))
        }),
    )?;
    engine.set_global("__blitsenAudioDecode", &decode)?;

    let poll_host = Rc::clone(&host);
    let poll = engine.define_function(
        "__blitsenAudioPoll",
        Box::new(move |_| json_string(raw_env, &poll_host.poll())),
    )?;
    engine.set_global("__blitsenAudioPoll", &poll)?;

    let pending_host = Rc::clone(&host);
    let pending = engine.define_function(
        "__blitsenAudioPending",
        Box::new(move |_| {
            let mut engine = NodeApiEngine::new(Env::from_raw(raw_env));
            Ok(engine.boolean(pending_host.pending()))
        }),
    )?;
    engine.set_global("__blitsenAudioPending", &pending)?;

    let channel_host = Rc::clone(&host);
    let channel = engine.define_function(
        "__blitsenAudioChannel",
        Box::new(move |call| {
            let buffer = argument(&call.arguments, 0, "audio buffer id")?
                .parse::<u64>()
                .map_err(|_| JsError::new("invalid audio buffer id"))?;
            let index = argument(&call.arguments, 1, "channel index")?
                .parse::<usize>()
                .map_err(|_| JsError::new("invalid channel index"))?;
            let samples = channel_host.channel_data(buffer, index)?;
            let mut engine = NodeApiEngine::new(Env::from_raw(raw_env));
            let bytes = samples.iter().flat_map(|sample| sample.to_le_bytes()).collect();
            engine.typed_array(&TypedArray::new(TypedArrayKind::Float32, bytes)?)
        }),
    )?;
    engine.set_global("__blitsenAudioChannel", &channel)?;

    let dispose = engine.define_function(
        "__blitsenAudioDispose",
        Box::new(move |call| {
            host.dispose();
            Ok(call.this)
        }),
    )?;
    engine.set_global("__blitsenAudioDispose", &dispose)
}

/// Installs the transport the bootstrap's `fetch` classes call through.
fn install_fetch(engine: &mut NodeApiEngine, raw_env: sys::napi_env) -> Result<(), JsError> {
    let host = Rc::new(fetch::FetchHost::new()?);

    let start_host = Rc::clone(&host);
    let start = engine.define_function(
        "__blitsenFetchStart",
        Box::new(move |call| {
            let spec = argument(&call.arguments, 0, "fetch request")?;
            let spec = serde_json::from_str(&spec)
                .map_err(|error| JsError::new(format!("invalid fetch request: {error}")))?;
            let mut engine = NodeApiEngine::new(Env::from_raw(raw_env));
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
    engine.set_global("__blitsenFetchStart", &start)?;

    let poll_host = Rc::clone(&host);
    let poll = engine.define_function(
        "__blitsenFetchPoll",
        Box::new(move |_| json_string(raw_env, &poll_host.poll())),
    )?;
    engine.set_global("__blitsenFetchPoll", &poll)?;

    let body_host = Rc::clone(&host);
    let body = engine.define_function(
        "__blitsenFetchBody",
        Box::new(move |call| {
            let id = fetch_id(&call.arguments)?;
            let kind = argument(&call.arguments, 1, "body kind")?;
            let bytes = body_host.take_body(id)?;
            let mut engine = NodeApiEngine::new(Env::from_raw(raw_env));
            match kind.as_str() {
                "text" => engine.string(&String::from_utf8_lossy(&bytes)),
                "bytes" => engine.typed_array(&TypedArray::new(TypedArrayKind::Uint8, bytes)?),
                other => Err(JsError::new(format!("invalid body kind: {other}"))),
            }
        }),
    )?;
    engine.set_global("__blitsenFetchBody", &body)?;

    let cancel_host = Rc::clone(&host);
    let cancel = engine.define_function(
        "__blitsenFetchCancel",
        Box::new(move |call| {
            cancel_host.cancel(fetch_id(&call.arguments)?);
            Ok(call.this)
        }),
    )?;
    engine.set_global("__blitsenFetchCancel", &cancel)?;

    let dispose = engine.define_function(
        "__blitsenFetchDispose",
        Box::new(move |call| {
            host.dispose();
            Ok(call.this)
        }),
    )?;
    engine.set_global("__blitsenFetchDispose", &dispose)
}

fn fetch_id(arguments: &[Unknown<'static>]) -> Result<u64, JsError> {
    argument(arguments, 0, "request id")?
        .parse::<u64>()
        .map_err(|_| JsError::new("invalid fetch request id"))
}

/// Installs the transport the bootstrap's `WebSocket` class calls through.
fn install_web_socket(engine: &mut NodeApiEngine, raw_env: sys::napi_env) -> Result<(), JsError> {
    let host = Rc::new(web_socket::WebSocketHost::new()?);

    let open_host = Rc::clone(&host);
    let open = engine.define_function(
        "__blitsenSocketOpen",
        Box::new(move |call| {
            let url = argument(&call.arguments, 0, "WebSocket address")?;
            let protocols = argument(&call.arguments, 1, "WebSocket subprotocols")?;
            let protocols: Vec<String> = serde_json::from_str(&protocols)
                .map_err(|error| JsError::new(format!("invalid subprotocol list: {error}")))?;
            let id = open_host.open(&url, &protocols)?;
            Ok(NodeApiEngine::new(Env::from_raw(raw_env)).number(id as f64))
        }),
    )?;
    engine.set_global("__blitsenSocketOpen", &open)?;

    let text_host = Rc::clone(&host);
    let send_text = engine.define_function(
        "__blitsenSocketSendText",
        Box::new(move |call| {
            let id = socket_id(&call.arguments)?;
            text_host.send_text(id, argument(&call.arguments, 1, "message text")?);
            Ok(call.this)
        }),
    )?;
    engine.set_global("__blitsenSocketSendText", &send_text)?;

    let binary_host = Rc::clone(&host);
    let send_binary = engine.define_function(
        "__blitsenSocketSendBinary",
        Box::new(move |call| {
            let id = socket_id(&call.arguments)?;
            let payload = call
                .arguments
                .get(1)
                .ok_or_else(|| JsError::new("missing message payload"))?;
            let mut engine = NodeApiEngine::new(Env::from_raw(raw_env));
            binary_host.send_binary(id, engine.to_typed_array(payload)?.bytes);
            Ok(call.this)
        }),
    )?;
    engine.set_global("__blitsenSocketSendBinary", &send_binary)?;

    let buffered_host = Rc::clone(&host);
    let buffered = engine.define_function(
        "__blitsenSocketBuffered",
        Box::new(move |call| {
            let bytes = buffered_host.buffered(socket_id(&call.arguments)?);
            Ok(NodeApiEngine::new(Env::from_raw(raw_env)).number(bytes as f64))
        }),
    )?;
    engine.set_global("__blitsenSocketBuffered", &buffered)?;

    let close_host = Rc::clone(&host);
    let close = engine.define_function(
        "__blitsenSocketClose",
        Box::new(move |call| {
            let id = socket_id(&call.arguments)?;
            // An empty code is a close with no status, which is a different
            // frame from one carrying 1005.
            let code = argument(&call.arguments, 1, "close code")?;
            let code = match code.as_str() {
                "" => None,
                code => Some(
                    code.parse::<u16>()
                        .map_err(|_| JsError::new("invalid WebSocket close code"))?,
                ),
            };
            close_host.close(id, code, &argument(&call.arguments, 2, "close reason")?);
            Ok(call.this)
        }),
    )?;
    engine.set_global("__blitsenSocketClose", &close)?;

    let poll_host = Rc::clone(&host);
    let poll = engine.define_function(
        "__blitsenSocketPoll",
        Box::new(move |_| json_string(raw_env, &poll_host.poll())),
    )?;
    engine.set_global("__blitsenSocketPoll", &poll)?;

    let payload_host = Rc::clone(&host);
    let payload = engine.define_function(
        "__blitsenSocketBinary",
        Box::new(move |call| {
            let id = socket_id(&call.arguments)?;
            let sequence = argument(&call.arguments, 1, "message sequence")?
                .parse::<u64>()
                .map_err(|_| JsError::new("invalid WebSocket message sequence"))?;
            let bytes = payload_host.take_binary(id, sequence)?;
            let mut engine = NodeApiEngine::new(Env::from_raw(raw_env));
            engine.typed_array(&TypedArray::new(TypedArrayKind::Uint8, bytes)?)
        }),
    )?;
    engine.set_global("__blitsenSocketBinary", &payload)?;

    let dispose = engine.define_function(
        "__blitsenSocketDispose",
        Box::new(move |call| {
            host.dispose();
            Ok(call.this)
        }),
    )?;
    engine.set_global("__blitsenSocketDispose", &dispose)
}

fn socket_id(arguments: &[Unknown<'static>]) -> Result<u64, JsError> {
    argument(arguments, 0, "socket id")?
        .parse::<u64>()
        .map_err(|_| JsError::new("invalid WebSocket id"))
}

fn argument(arguments: &[Unknown<'static>], index: usize, name: &str) -> Result<String, JsError> {
    arguments
        .get(index)
        .ok_or_else(|| JsError::new(format!("missing {name}")))
        .and_then(callback_string)
}

fn json_string(env: sys::napi_env, value: &Value) -> Result<Unknown<'static>, JsError> {
    let value = serde_json::to_string(value).map_err(|error| JsError::new(error.to_string()))?;
    let length = isize::try_from(value.len())
        .map_err(|_| JsError::new("DOM bridge result exceeds Node-API string limits"))?;
    let mut result = std::ptr::null_mut();
    check(
        unsafe { sys::napi_create_string_utf8(env, value.as_ptr().cast(), length, &mut result) },
        "serialize DOM bridge result",
    )?;
    Ok(unknown(env, result))
}
