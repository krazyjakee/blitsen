use blitsen_js::{JsEngine, JsError};
use blitsen_platform::os;
use serde_json::json;

use super::super::{intl, json_value};
#[cfg(not(target_os = "android"))]
use super::failed;

// Every one of these answers a whole record at once rather than a field at a
// time. A monitor reads the processor once per tick and shows a dozen numbers
// off it; a getter per field would sample the machine a dozen times for one
// frame and hand back readings taken at different instants, which is a
// per-core usage list that does not add up to the total beside it.
pub(super) fn install<E: JsEngine + 'static>(engine: &mut E) -> Result<(), JsError> {
    engine.define_global_function(
        "__blitsenNativeOsCpu",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            json_value(&mut engine, &json!(os::cpu()))
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeOsMemory",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            json_value(&mut engine, &json!(os::memory()))
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeOsStorage",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            json_value(&mut engine, &json!(os::storage()))
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeOsHost",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            json_value(&mut engine, &json!(os::host()))
        }),
    )?;

    // The locale and zone this session is configured for. Absent until `Intl`
    // was implemented (#237), because a tag with no formatter behind it implies
    // a capability that is not there; these are the two values an application
    // now passes straight into `Intl.NumberFormat` and `Intl.DateTimeFormat`,
    // and they are read from the same sources those default to.
    engine.define_global_function(
        "__blitsenNativeOsLocale",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let locale = json!({
                "language": intl::default_locale().to_string(),
                "timeZone": intl::default_time_zone(),
            });
            json_value(&mut engine, &locale)
        }),
    )?;

    install_battery(engine)
}

/// The batteries, which are the one reading in this module Android does not get.
///
/// A machine that cannot be asked about power throws rather than answering an
/// empty list, because the empty list already means something else: it is a
/// desktop with no battery, and that is a fact rather than a failure (#98).
#[cfg(not(target_os = "android"))]
fn install_battery<E: JsEngine + 'static>(engine: &mut E) -> Result<(), JsError> {
    engine.define_global_function(
        "__blitsenNativeOsBatteries",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let batteries = os::batteries().map_err(failed)?;
            json_value(&mut engine, &json!(batteries))
        }),
    )
}

// Android has no `starship-battery` backend, so there is no reading to install
// and `os.batteries` is `undefined` there. Its own power service is a
// `BatteryManager` over JNI, which is a module-shaped decision rather than this
// one with the source swapped out.
#[cfg(target_os = "android")]
fn install_battery<E: JsEngine + 'static>(_engine: &mut E) -> Result<(), JsError> {
    Ok(())
}
