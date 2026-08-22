//! The 2D context's seam, which is one function rather than one per call.
//!
//! Everything a `CanvasRenderingContext2D` does crosses here: a frame's worth
//! of drawing as a `Float64Array`, and the six reads that answer with something
//! — pixels, a data URL, an encoded image, text metrics, a hit test, the
//! backing store size.
//!
//! One function because the drawing half is hot. A canvas frame is hundreds to
//! thousands of operations, and the DOM bridge's own channel costs a string
//! conversion per argument and a JSON parse per answer; a canvas that paid that
//! per `fillRect` would spend more time at the boundary than drawing. So the
//! commands arrive as one typed array the bootstrap fills in place, and the
//! answers are built as values rather than serialized and re-parsed.
//!
//! What the numbers in that array mean is not decided here. This function
//! carries them; `blitsen-blitz`'s canvas module reads them, and the bootstrap
//! writes them. Naming the operations here as well would be a third place for
//! the encoding to drift.

use blitsen_dom::{CanvasCommands, CanvasTextStyle, DomBackend as _};
use blitsen_js::{JsEngine, JsError, JsType, NativeCall, TypedArray, TypedArrayKind};

use super::{DomRuntime, argument};
use crate::dom_error;

/// Installs `__blitsenCanvasCall`, the only entry point a 2D context has.
pub(super) fn install<E: JsEngine + 'static>(
    engine: &mut E,
    runtime: DomRuntime,
) -> Result<(), JsError> {
    engine.define_global_function(
        "__blitsenCanvasCall",
        Box::new(move |call| dispatch::<E>(&runtime, call)),
    )
}

fn dispatch<E: JsEngine + 'static>(
    runtime: &DomRuntime,
    call: NativeCall<E::Value>,
) -> Result<E::Value, JsError> {
    let mut engine = E::from_value(&call.this);
    let handle = argument(&mut engine, &call, 0, "canvas handle")?;
    let operation = argument(&mut engine, &call, 1, "canvas operation")?;
    let node = runtime.resolve_handle(&handle)?;
    let numbers = numbers(&mut engine, &call, 2)?;
    let strings = strings(&mut engine, &call, 3)?;
    let document = runtime.document();

    match operation.as_str() {
        "submit" => {
            let pixels = bytes(&mut engine, &call, 4)?;
            document
                .borrow_mut()
                .submit_canvas(
                    node,
                    CanvasCommands {
                        numbers: &numbers,
                        strings: &strings,
                        pixels: &pixels,
                    },
                )
                .map_err(dom_error)?;
            Ok(call.this)
        }
        "size" => {
            let surface = document
                .borrow_mut()
                .canvas_surface(node)
                .map_err(dom_error)?;
            floats(
                &mut engine,
                &[f64::from(surface.width), f64::from(surface.height)],
            )
        }
        // `getImageData`, whose rectangle the bootstrap has already turned into
        // a non-negative origin and extent.
        "pixels" => {
            let [x, y, width, height] = fixed(&numbers, "image data rectangle")?;
            let pixels = document
                .borrow_mut()
                .canvas_pixels(node, x, y, width as u32, height as u32)
                .map_err(dom_error)?;
            engine.typed_array(&TypedArray::new(TypedArrayKind::Uint8Clamped, pixels)?)
        }
        "dataUrl" => {
            let url = document
                .borrow_mut()
                .canvas_data_url(node, string_at(&strings, 0)?, number_at(&numbers, 0))
                .map_err(dom_error)?;
            engine.string(&url)
        }
        // `toBlob`, which needs the type it actually encoded as well as the
        // bytes: a canvas asked for a format this runtime cannot write answers
        // in PNG, and the `Blob` has to say so.
        "encode" => {
            let encoding = document
                .borrow_mut()
                .encode_canvas(node, string_at(&strings, 0)?, number_at(&numbers, 0))
                .map_err(dom_error)?;
            let result = engine.object()?;
            let mime = engine.string(encoding.mime_type)?;
            engine.set_property(&result, "type", &mime)?;
            let bytes =
                engine.typed_array(&TypedArray::new(TypedArrayKind::Uint8, encoding.bytes)?)?;
            engine.set_property(&result, "bytes", &bytes)?;
            Ok(result)
        }
        "measure" => {
            let [size, weight, style, stretch, align, baseline, rtl] =
                fixed(&numbers, "text style")?;
            let metrics = document
                .borrow_mut()
                .measure_canvas_text(
                    CanvasTextStyle {
                        families: string_at(&strings, 0)?,
                        size,
                        weight,
                        style: style as u8,
                        stretch,
                        align: align as u8,
                        baseline: baseline as u8,
                        rtl: rtl != 0.0,
                    },
                    string_at(&strings, 1)?,
                )
                .map_err(dom_error)?;
            floats(
                &mut engine,
                &[
                    metrics.width,
                    metrics.actual_left,
                    metrics.actual_right,
                    metrics.actual_ascent,
                    metrics.actual_descent,
                    metrics.font_ascent,
                    metrics.font_descent,
                ],
            )
        }
        // `isPointInPath` and `isPointInStroke`. The leading number says which,
        // because the two differ only in whether the path is expanded by the
        // pen before the point is tested against it.
        "contains" => {
            let (stroked, geometry) = numbers.split_first().ok_or_else(|| {
                JsError::new("canvas hit test needs a path and a point to test it against")
            })?;
            let contains = document
                .borrow_mut()
                .canvas_contains(*stroked != 0.0, geometry)
                .map_err(dom_error)?;
            Ok(engine.boolean(contains))
        }
        _ => Err(JsError::new(format!(
            "unknown canvas bridge operation: {operation}"
        ))),
    }
}

/// Reads the command stream, which is always a `Float64Array`.
fn numbers<E: JsEngine>(
    engine: &mut E,
    call: &NativeCall<E::Value>,
    index: usize,
) -> Result<Vec<f64>, JsError> {
    let array = engine.to_typed_array(call.argument(index, "canvas command numbers")?)?;
    if array.kind != TypedArrayKind::Float64 {
        return Err(JsError::new("canvas commands must be a Float64Array"));
    }
    Ok(array
        .bytes
        .as_chunks::<8>()
        .0
        .iter()
        .map(|chunk| f64::from_ne_bytes(*chunk))
        .collect())
}

/// Reads the stream's string table, which may be absent when it needs none.
fn strings<E: JsEngine>(
    engine: &mut E,
    call: &NativeCall<E::Value>,
    index: usize,
) -> Result<Vec<String>, JsError> {
    let Some(value) = call.arguments.get(index) else {
        return Ok(Vec::new());
    };
    if engine.value_type(value)? != JsType::Array {
        return Ok(Vec::new());
    }
    let values = engine.to_array(value)?;
    let mut strings = Vec::with_capacity(values.len());
    for value in &values {
        strings.push(engine.to_string(value)?);
    }
    Ok(strings)
}

/// Reads the stream's pixel buffer, which only `putImageData` fills.
fn bytes<E: JsEngine>(
    engine: &mut E,
    call: &NativeCall<E::Value>,
    index: usize,
) -> Result<Vec<u8>, JsError> {
    let Some(value) = call.arguments.get(index) else {
        return Ok(Vec::new());
    };
    if engine.value_type(value)? != JsType::TypedArray {
        return Ok(Vec::new());
    }
    let array = engine.to_typed_array(value)?;
    if !matches!(
        array.kind,
        TypedArrayKind::Uint8 | TypedArrayKind::Uint8Clamped
    ) {
        return Err(JsError::new(
            "canvas pixel data must be a Uint8Array or Uint8ClampedArray",
        ));
    }
    Ok(array.bytes)
}

/// Answers with numbers, which is every answer that is not bytes or a string.
///
/// A typed array rather than an object because the bootstrap knows what each
/// position means and an object would cost a property write and a lookup per
/// field to say the same thing.
fn floats<E: JsEngine>(engine: &mut E, values: &[f64]) -> Result<E::Value, JsError> {
    let bytes = values
        .iter()
        .flat_map(|value| value.to_ne_bytes())
        .collect();
    engine.typed_array(&TypedArray::new(TypedArrayKind::Float64, bytes)?)
}

/// Reads exactly the arguments an operation takes, naming what was short.
fn fixed<const N: usize>(numbers: &[f64], what: &str) -> Result<[f64; N], JsError> {
    numbers
        .get(..N)
        .and_then(|values| <[f64; N]>::try_from(values).ok())
        .ok_or_else(|| JsError::new(format!("canvas {what} needs {N} numbers")))
}

fn number_at(numbers: &[f64], index: usize) -> f64 {
    numbers.get(index).copied().unwrap_or(f64::NAN)
}

fn string_at(strings: &[String], index: usize) -> Result<&str, JsError> {
    strings
        .get(index)
        .map(String::as_str)
        .ok_or_else(|| JsError::new("canvas operation is missing a string argument"))
}
