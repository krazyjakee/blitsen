use blitsen_js::{JsEngine, JsError};

#[cfg(not(target_os = "android"))]
use super::super::{argument, json_value, window};

/// The window this session already owns, never a second one: creating windows
/// waits on the shared-versus-isolated JavaScript context decision, and the
/// members that would need it are declared absent instead.
#[cfg(not(target_os = "android"))]
pub(super) fn install<E: JsEngine + 'static>(engine: &mut E) -> Result<(), JsError> {
    engine.define_global_function(
        "__blitsenNativeWindowSet",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let property = argument(&mut engine, &call, 0, "window property")?;
            let value = argument(&mut engine, &call, 1, "window property value")?;
            window::set(&property, &value)?;
            Ok(call.this)
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeWindowGet",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let property = argument(&mut engine, &call, 0, "window property")?;
            let value = window::get(&property)?;
            Ok(engine.boolean(value))
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeWindowResize",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let mut dimension = |index, name: &str| {
                argument(&mut engine, &call, index, name)?
                    .parse::<f64>()
                    .map_err(|_| JsError::new(format!("invalid window {name}")))
            };
            let width = dimension(0, "width")?;
            let height = dimension(1, "height")?;
            window::resize(width, height)?;
            Ok(call.this)
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeWindowCommand",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let command = argument(&mut engine, &call, 0, "window command")?;
            window::command(&command)?;
            Ok(call.this)
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeWindowMonitors",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            json_value(&mut engine, &window::monitors()?)
        }),
    )
}

// Nothing to install. Not because winit refuses any of this on Android — it
// accepts every setter and answers every getter — but because none of the
// answers are true, and a wrong answer is worse than a missing one. The monitor
// list is the one that had to be checked rather than assumed, because it looks
// like the survivor: winit's Android `available_monitors()` is `iter::empty()`,
// so `monitors()` would report a device with no display. `dom_bridge/window.rs`
// reads off the rest of the backend, line by line. Immersive mode and
// orientation are the real capabilities here and are not these under another
// name (#146, #147).
#[cfg(target_os = "android")]
pub(super) fn install<E: JsEngine + 'static>(_engine: &mut E) -> Result<(), JsError> {
    Ok(())
}
