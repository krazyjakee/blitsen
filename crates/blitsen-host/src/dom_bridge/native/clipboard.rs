use blitsen_js::{JsEngine, JsError};
#[cfg(not(target_os = "android"))]
use blitsen_js::{TypedArray, TypedArrayKind};
#[cfg(not(target_os = "android"))]
use blitsen_platform::clipboard::{self, Image};

#[cfg(not(target_os = "android"))]
use super::super::argument;
#[cfg(not(target_os = "android"))]
use super::failed;

#[cfg(not(target_os = "android"))]
pub(super) fn install<E: JsEngine + 'static>(engine: &mut E) -> Result<(), JsError> {
    engine.define_global_function(
        "__blitsenNativeClipboardRead",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let flavour = argument(&mut engine, &call, 0, "clipboard flavour")?;
            let text = match flavour.as_str() {
                "text" => clipboard::read_text(),
                "html" => clipboard::read_html(),
                other => return Err(JsError::new(format!("unknown clipboard flavour: {other}"))),
            }
            .map_err(failed)?;
            match text {
                Some(text) => engine.string(&text),
                None => Ok(engine.null()),
            }
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeClipboardWrite",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let flavour = argument(&mut engine, &call, 0, "clipboard flavour")?;
            let value = argument(&mut engine, &call, 1, "clipboard contents")?;
            match flavour.as_str() {
                "text" => clipboard::write_text(&value),
                "html" => {
                    let alternative = argument(&mut engine, &call, 2, "plain-text alternative")?;
                    clipboard::write_html(&value, Some(&alternative))
                }
                other => return Err(JsError::new(format!("unknown clipboard flavour: {other}"))),
            }
            .map_err(failed)?;
            Ok(call.this)
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeClipboardReadImage",
        Box::new(move |call| {
            let image = clipboard::read_image().map_err(failed)?;
            let mut engine = E::from_value(&call.this);
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

    engine.define_global_function(
        "__blitsenNativeClipboardWriteImage",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let width = argument(&mut engine, &call, 0, "image width")?;
            let height = argument(&mut engine, &call, 1, "image height")?;
            let pixels = call
                .arguments
                .get(2)
                .ok_or_else(|| JsError::new("missing image pixels"))?;
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

    engine.define_global_function(
        "__blitsenNativeClipboardClear",
        Box::new(move |call| {
            clipboard::clear().map_err(failed)?;
            Ok(call.this)
        }),
    )
}

// Nothing to install: `arboard` has no Android backend, and the service it would
// wrap answers a background read with a refusal these signatures cannot report
// apart from an empty clipboard. `blitsen_platform::clipboard` makes the case.
#[cfg(target_os = "android")]
pub(super) fn install<E: JsEngine + 'static>(_engine: &mut E) -> Result<(), JsError> {
    Ok(())
}
