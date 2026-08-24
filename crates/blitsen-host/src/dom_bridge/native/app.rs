use blitsen_js::{JsEngine, JsError};
#[cfg(not(target_os = "android"))]
use blitsen_platform::app::{self, Directory};
#[cfg(not(target_os = "android"))]
use serde_json::json;

#[cfg(not(target_os = "android"))]
use super::super::{argument, json_value};
#[cfg(not(target_os = "android"))]
use super::failed;

#[cfg(not(target_os = "android"))]
pub(super) fn install<E: JsEngine + 'static>(engine: &mut E) -> Result<(), JsError> {
    engine.define_global_function(
        "__blitsenNativeAppDirectory",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let kind = match argument(&mut engine, &call, 0, "directory kind")?.as_str() {
                "data" => Directory::Data,
                "cache" => Directory::Cache,
                "config" => Directory::Config,
                other => return Err(JsError::new(format!("unknown directory kind: {other}"))),
            };
            let name = argument(&mut engine, &call, 1, "application name")?;
            let path = app::directory(kind, &name).map_err(failed)?;
            engine.string(&path.to_string_lossy())
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeAppRelaunch",
        Box::new(move |call| {
            app::relaunch().map_err(failed)?;
            Ok(call.this)
        }),
    )?;

    install_single_instance(engine)
}

// Android is the one platform where the OS delivers a second launch to the
// existing Activity. Desktop Unix uses domain sockets and Windows named pipes
// behind the same platform API.
#[cfg(not(target_os = "android"))]
fn install_single_instance<E: JsEngine + 'static>(engine: &mut E) -> Result<(), JsError> {
    use blitsen_platform::app::{Instance, Invocation, single_instance};

    // The invocation to hand over is read from the OS rather than passed in:
    // this is the same command line `process.argv` reports, and asking the
    // bootstrap for it would make the bridge depend on the Phase 1 host's
    // `process` object.
    engine.define_global_function(
        "__blitsenNativeAppSingleInstance",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let name = argument(&mut engine, &call, 0, "application name")?;
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
            Ok(engine.boolean(instance == Instance::Primary))
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeAppPending",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            Ok(engine.boolean(single_instance::pending()))
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeAppSecondInstances",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            json_value(&mut engine, &json!(single_instance::take()))
        }),
    )
}

// Nothing to install. Android is a unix, so the socket above would bind and the
// lock would be taken — and it would be answering a question nobody asked: an
// Android application is one process by construction, and a second launch is an
// `Intent` delivered to the instance already running rather than a command line
// to hand over. The directories are the Activity's and relaunch has no
// executable to spawn; `blitsen_platform::app` states the case for all three.
#[cfg(target_os = "android")]
pub(super) fn install<E: JsEngine + 'static>(_engine: &mut E) -> Result<(), JsError> {
    Ok(())
}
