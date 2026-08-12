//! Host half of the `native:` modules, below the namespace the bootstrap builds.
//!
//! Every function here is installed under a `__blitsenNative…` name and only if
//! this platform can implement it properly. That is what makes the namespace
//! honest: the bootstrap drops any member whose host function is missing, so a
//! capability this build does not have reads as `undefined` and feature
//! detection selects a fallback (COMPATIBILITY.md, "Capability tiers").

use blitsen_js::{JsEngine, JsError, TypedArray, TypedArrayKind};
use blitsen_platform::PlatformError;
use blitsen_platform::app::{self, Directory};
use blitsen_platform::clipboard::{self, Image};
use napi::{Env, sys};
use serde_json::json;

use super::{NodeApiEngine, argument, json_string};

fn failed(error: PlatformError) -> JsError {
    JsError::new(error.message().to_owned())
}

/// Installs the host functions the `native:` namespace is assembled from.
pub(super) fn install(engine: &mut NodeApiEngine, raw_env: sys::napi_env) -> Result<(), JsError> {
    install_app(engine, raw_env)?;
    install_clipboard(engine, raw_env)
}

fn install_app(engine: &mut NodeApiEngine, raw_env: sys::napi_env) -> Result<(), JsError> {
    let directory = engine.define_function(
        "__blitsenNativeAppDirectory",
        Box::new(move |call| {
            let kind = match argument(&call.arguments, 0, "directory kind")?.as_str() {
                "data" => Directory::Data,
                "cache" => Directory::Cache,
                "config" => Directory::Config,
                other => return Err(JsError::new(format!("unknown directory kind: {other}"))),
            };
            let name = argument(&call.arguments, 1, "application name")?;
            let path = app::directory(kind, &name).map_err(failed)?;
            NodeApiEngine::new(Env::from_raw(raw_env)).string(&path.to_string_lossy())
        }),
    )?;
    engine.set_global("__blitsenNativeAppDirectory", &directory)?;

    let relaunch = engine.define_function(
        "__blitsenNativeAppRelaunch",
        Box::new(move |call| {
            app::relaunch().map_err(failed)?;
            Ok(call.this)
        }),
    )?;
    engine.set_global("__blitsenNativeAppRelaunch", &relaunch)?;

    install_single_instance(engine, raw_env)
}

#[cfg(unix)]
fn install_single_instance(
    engine: &mut NodeApiEngine,
    raw_env: sys::napi_env,
) -> Result<(), JsError> {
    use blitsen_platform::app::{Instance, Invocation, single_instance};

    // The invocation to hand over is read from the OS rather than passed in:
    // this is the same command line `process.argv` reports, and asking the
    // bootstrap for it would make the bridge depend on the Phase 1 host's
    // `process` object.
    let request = engine.define_function(
        "__blitsenNativeAppSingleInstance",
        Box::new(move |call| {
            let name = argument(&call.arguments, 0, "application name")?;
            let invocation = Invocation {
                argv: std::env::args().collect(),
                cwd: std::env::current_dir()
                    .map_err(|error| {
                        JsError::new(format!("could not read the working directory: {error}"))
                    })?
                    .to_string_lossy()
                    .into_owned(),
            };
            let instance = single_instance::request(&name, &invocation).map_err(failed)?;
            let mut engine = NodeApiEngine::new(Env::from_raw(raw_env));
            Ok(engine.boolean(instance == Instance::Primary))
        }),
    )?;
    engine.set_global("__blitsenNativeAppSingleInstance", &request)?;

    let pending = engine.define_function(
        "__blitsenNativeAppPending",
        Box::new(move |_| {
            let mut engine = NodeApiEngine::new(Env::from_raw(raw_env));
            Ok(engine.boolean(single_instance::pending()))
        }),
    )?;
    engine.set_global("__blitsenNativeAppPending", &pending)?;

    let take = engine.define_function(
        "__blitsenNativeAppSecondInstances",
        Box::new(move |_| json_string(raw_env, &json!(single_instance::take()))),
    )?;
    engine.set_global("__blitsenNativeAppSecondInstances", &take)
}

// Nothing to install: a named mutex and a pipe are a different design, not this
// one with the socket swapped out.
#[cfg(not(unix))]
fn install_single_instance(
    _engine: &mut NodeApiEngine,
    _raw_env: sys::napi_env,
) -> Result<(), JsError> {
    Ok(())
}

fn install_clipboard(engine: &mut NodeApiEngine, raw_env: sys::napi_env) -> Result<(), JsError> {
    let read = engine.define_function(
        "__blitsenNativeClipboardRead",
        Box::new(move |call| {
            let flavour = argument(&call.arguments, 0, "clipboard flavour")?;
            let text = match flavour.as_str() {
                "text" => clipboard::read_text(),
                "html" => clipboard::read_html(),
                other => return Err(JsError::new(format!("unknown clipboard flavour: {other}"))),
            }
            .map_err(failed)?;
            let mut engine = NodeApiEngine::new(Env::from_raw(raw_env));
            match text {
                Some(text) => engine.string(&text),
                None => Ok(engine.null()),
            }
        }),
    )?;
    engine.set_global("__blitsenNativeClipboardRead", &read)?;

    let write = engine.define_function(
        "__blitsenNativeClipboardWrite",
        Box::new(move |call| {
            let flavour = argument(&call.arguments, 0, "clipboard flavour")?;
            let value = argument(&call.arguments, 1, "clipboard contents")?;
            match flavour.as_str() {
                "text" => clipboard::write_text(&value),
                "html" => {
                    let alternative = argument(&call.arguments, 2, "plain-text alternative")?;
                    clipboard::write_html(&value, Some(&alternative))
                }
                other => return Err(JsError::new(format!("unknown clipboard flavour: {other}"))),
            }
            .map_err(failed)?;
            Ok(call.this)
        }),
    )?;
    engine.set_global("__blitsenNativeClipboardWrite", &write)?;

    let read_image = engine.define_function(
        "__blitsenNativeClipboardReadImage",
        Box::new(move |_| {
            let image = clipboard::read_image().map_err(failed)?;
            let mut engine = NodeApiEngine::new(Env::from_raw(raw_env));
            let Some(image) = image else {
                return Ok(engine.null());
            };
            let pixels =
                engine.typed_array(&TypedArray::new(TypedArrayKind::Uint8, image.rgba)?)?;
            let object = engine.object()?;
            let width = engine.number(image.width as f64);
            let height = engine.number(image.height as f64);
            engine.set_property(&object, "width", &width)?;
            engine.set_property(&object, "height", &height)?;
            engine.set_property(&object, "data", &pixels)?;
            Ok(object)
        }),
    )?;
    engine.set_global("__blitsenNativeClipboardReadImage", &read_image)?;

    let write_image = engine.define_function(
        "__blitsenNativeClipboardWriteImage",
        Box::new(move |call| {
            let width = argument(&call.arguments, 0, "image width")?;
            let height = argument(&call.arguments, 1, "image height")?;
            let pixels = call
                .arguments
                .get(2)
                .ok_or_else(|| JsError::new("missing image pixels"))?;
            let mut engine = NodeApiEngine::new(Env::from_raw(raw_env));
            let pixels = engine.to_typed_array(pixels)?;
            if !matches!(
                pixels.kind,
                TypedArrayKind::Uint8 | TypedArrayKind::Uint8Clamped
            ) {
                return Err(JsError::new(
                    "clipboard image pixels must be a Uint8Array or Uint8ClampedArray",
                ));
            }
            let dimension = |value: String, name: &str| {
                value
                    .parse::<usize>()
                    .map_err(|_| JsError::new(format!("invalid image {name}")))
            };
            clipboard::write_image(&Image {
                width: dimension(width, "width")?,
                height: dimension(height, "height")?,
                rgba: pixels.bytes,
            })
            .map_err(failed)?;
            Ok(call.this)
        }),
    )?;
    engine.set_global("__blitsenNativeClipboardWriteImage", &write_image)?;

    let clear = engine.define_function(
        "__blitsenNativeClipboardClear",
        Box::new(move |call| {
            clipboard::clear().map_err(failed)?;
            Ok(call.this)
        }),
    )?;
    engine.set_global("__blitsenNativeClipboardClear", &clear)
}
