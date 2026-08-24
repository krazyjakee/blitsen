use blitsen_js::{JsEngine, JsError, TypedArray, TypedArrayKind};

use super::super::{argument, hid, json_value};

/// Reads a report argument, refusing anything past the conservative ceiling.
///
/// The device's own declared bound is the real limit and is checked by the
/// controller before it allocates or transfers anything. This is the earlier,
/// cruder guard: it stops a caller from parking an arbitrarily large buffer in
/// the request queue for a frame before the controller ever sees it.
fn report_argument<E: JsEngine>(
    engine: &mut E,
    call: &blitsen_js::NativeCall<E::Value>,
    index: usize,
) -> Result<Vec<u8>, JsError> {
    let report = engine.to_typed_array(call.argument(index, "HID report")?)?;
    if !matches!(
        report.kind,
        TypedArrayKind::Uint8 | TypedArrayKind::Uint8Clamped
    ) {
        return Err(JsError::new(
            "a HID report must be a Uint8Array or Uint8ClampedArray",
        ));
    }
    let ceiling = crate::native_window::hid::MAX_REPORT_BYTES;
    if report.bytes.len() > ceiling {
        return Err(JsError::new(format!(
            "a HID report of {} bytes exceeds the {ceiling}-byte ceiling",
            report.bytes.len()
        )));
    }
    Ok(report.bytes)
}

pub(super) fn install<E: JsEngine + 'static>(engine: &mut E) -> Result<(), JsError> {
    fn command<E: JsEngine>(engine: &mut E, id: u64) -> Result<E::Value, JsError> {
        engine.string(&id.to_string())
    }

    engine.define_global_function(
        "__blitsenNativeHidDevices",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            command(&mut engine, hid::devices())
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeHidOpen",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let device_id = argument(&mut engine, &call, 0, "HID device id")?;
            command(&mut engine, hid::open(device_id))
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeHidClose",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let device_id = argument(&mut engine, &call, 0, "HID device id")?;
            command(&mut engine, hid::close(device_id))
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeHidWrite",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let device_id = argument(&mut engine, &call, 0, "HID device id")?;
            let data = report_argument(&mut engine, &call, 1)?;
            command(&mut engine, hid::write(device_id, data))
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeHidSendFeatureReport",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let device_id = argument(&mut engine, &call, 0, "HID device id")?;
            let data = report_argument(&mut engine, &call, 1)?;
            command(&mut engine, hid::send_feature_report(device_id, data))
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeHidReceiveFeatureReport",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let device_id = argument(&mut engine, &call, 0, "HID device id")?;
            let report_id = argument(&mut engine, &call, 1, "HID report id")?
                .parse::<u8>()
                .map_err(|_| JsError::new("a HID report id is a byte"))?;
            command(
                &mut engine,
                hid::receive_feature_report(device_id, report_id),
            )
        }),
    )?;

    // Hot-plug is polled, so the host has to be told when anything cares. An
    // application that never listens never makes the runtime walk the device
    // tree, which is the whole of "does not keep the runtime busy".
    engine.define_global_function(
        "__blitsenNativeHidWatch",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let watching = engine.to_boolean(call.argument(0, "HID watch flag")?)?;
            hid::watch(watching);
            Ok(engine.undefined())
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeHidPending",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            Ok(engine.boolean(hid::pending()))
        }),
    )?;

    // Structured fields as JSON beside the raw report, rather than a report
    // re-encoded into JSON: an input report is bytes, and every frame of a
    // 1 kHz device would otherwise be encoded and parsed for no one's benefit.
    engine.define_global_function(
        "__blitsenNativeHidTake",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let mut messages = Vec::new();
            for message in hid::take_messages() {
                let object = engine.object()?;
                let json = json_value(&mut engine, &message.value)?;
                engine.set_property(&object, "json", &json)?;
                let data = match message.data {
                    Some(bytes) => {
                        engine.typed_array(&TypedArray::new(TypedArrayKind::Uint8, bytes)?)?
                    }
                    None => engine.null(),
                };
                engine.set_property(&object, "data", &data)?;
                messages.push(object);
            }
            engine.array(&messages)
        }),
    )
}
