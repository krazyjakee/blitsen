use blitsen_js::{JsEngine, JsError};
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use serde_json::json;

#[cfg(not(any(target_os = "android", target_os = "ios")))]
use super::super::{argument, json_value};
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use super::failed;

/// `blitsen/shell` (#384): a URL to the browser, a path to its application,
/// or a path revealed in the file manager.
///
/// Every request returns the id its completion will carry, and the bootstrap
/// settles the promise on a frame turn, the way it does a dialog's answer: the
/// hand-off runs on a worker thread because `xdg-open` may block for as long
/// as the handler it found, and the calling thread is the one that paints.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub(super) fn install<E: JsEngine + 'static>(engine: &mut E) -> Result<(), JsError> {
    use blitsen_platform::shell;

    engine.define_global_function(
        "__blitsenNativeShellOpen",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let kind = argument(&mut engine, &call, 0, "shell operation")?;
            let target = argument(&mut engine, &call, 1, "shell target")?;
            let id = match kind.as_str() {
                "openExternal" => shell::open_external(&target),
                "openPath" => shell::open_path(&target),
                "showItemInFolder" => shell::show_item_in_folder(&target),
                other => return Err(JsError::new(format!("unknown shell operation: {other}"))),
            }
            .map_err(failed)?;
            engine.string(&id.to_string())
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeShellPending",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            Ok(engine.boolean(shell::pending()))
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeShellTake",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let finished = shell::take()
                .into_iter()
                .map(|completion| match completion.outcome {
                    Ok(()) => json!({
                        "id": completion.id.to_string(),
                        "error": null,
                        "errorName": null,
                    }),
                    Err(failure) => json!({
                        "id": completion.id.to_string(),
                        "error": failure.message(),
                        "errorName": failure.name(),
                    }),
                })
                .collect::<Vec<_>>();
            json_value(&mut engine, &json!(finished))
        }),
    )
}

// Opening a URL or a file on a mobile platform is an `Intent` the Activity
// sends, and there is no file manager to reveal an item in.
#[cfg(any(target_os = "android", target_os = "ios"))]
pub(super) fn install<E: JsEngine + 'static>(_engine: &mut E) -> Result<(), JsError> {
    Ok(())
}
