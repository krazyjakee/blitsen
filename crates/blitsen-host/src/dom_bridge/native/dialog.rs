use blitsen_js::{JsEngine, JsError};
#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "android", target_os = "ios"))
))]
use serde_json::json;

#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "android", target_os = "ios"))
))]
use super::super::{argument, json_value, window};
#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "android", target_os = "ios"))
))]
use super::failed;

#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "android", target_os = "ios"))
))]
pub(super) fn install<E: JsEngine + 'static>(engine: &mut E) -> Result<(), JsError> {
    use blitsen_platform::dialog::{
        self, Buttons, FileKind, FileRequest, Filter, Level, MessageRequest, Outcome,
    };
    use serde::Deserialize;

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FileSpec {
        title: Option<String>,
        directory: Option<String>,
        file_name: Option<String>,
        filters: Vec<FilterSpec>,
    }

    #[derive(Deserialize)]
    struct FilterSpec {
        name: String,
        extensions: Vec<String>,
    }

    #[derive(Deserialize)]
    struct MessageSpec {
        title: String,
        message: String,
        level: String,
        buttons: String,
    }

    fn spec<T: serde::de::DeserializeOwned>(json: &str) -> Result<T, JsError> {
        serde_json::from_str(json)
            .map_err(|error| JsError::new(format!("malformed dialog options: {error}")))
    }

    engine.define_global_function(
        "__blitsenNativeDialogFile",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let kind = match argument(&mut engine, &call, 0, "dialog kind")?.as_str() {
                "openFile" => FileKind::OpenFile,
                "openFiles" => FileKind::OpenFiles,
                "saveFile" => FileKind::SaveFile,
                "openFolder" => FileKind::OpenFolder,
                "openFolders" => FileKind::OpenFolders,
                other => return Err(JsError::new(format!("unknown file dialog: {other}"))),
            };
            let options: FileSpec = spec(&argument(&mut engine, &call, 1, "dialog options")?)?;
            let request = FileRequest {
                title: options.title,
                directory: options.directory.map(Into::into),
                file_name: options.file_name,
                filters: options
                    .filters
                    .into_iter()
                    .map(|filter| Filter {
                        name: filter.name,
                        extensions: filter.extensions,
                    })
                    .collect(),
            };
            // Inside `with`, because a dialog here is always modal to the
            // application window: there is no parentless one to open.
            let id = window::with(|parent| {
                dialog::open_file(kind, &request, Some(parent)).map_err(failed)
            })?;
            engine.string(&id.to_string())
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeDialogMessage",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let options: MessageSpec = spec(&argument(&mut engine, &call, 0, "dialog options")?)?;
            let request = MessageRequest {
                title: options.title,
                message: options.message,
                level: match options.level.as_str() {
                    "info" => Level::Info,
                    "warning" => Level::Warning,
                    "error" => Level::Error,
                    other => {
                        return Err(JsError::new(format!(
                            "{other:?} is not a message level: info, warning or error"
                        )));
                    }
                },
                buttons: match options.buttons.as_str() {
                    "ok" => Buttons::Ok,
                    "okCancel" => Buttons::OkCancel,
                    "yesNo" => Buttons::YesNo,
                    "yesNoCancel" => Buttons::YesNoCancel,
                    other => {
                        return Err(JsError::new(format!(
                            "{other:?} is not a button set: ok, okCancel, yesNo or yesNoCancel"
                        )));
                    }
                },
            };
            let id = window::with(|parent| {
                dialog::open_message(&request, Some(parent)).map_err(failed)
            })?;
            engine.string(&id.to_string())
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeDialogPending",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            Ok(engine.boolean(dialog::pending()))
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeDialogTake",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let closed = dialog::take()
                .into_iter()
                .map(|completion| {
                    let value = match completion.outcome {
                        Outcome::Paths(paths) => json!(
                            paths
                                .iter()
                                .map(|path| path.to_string_lossy().into_owned())
                                .collect::<Vec<_>>()
                        ),
                        Outcome::Button(button) => json!(match button {
                            dialog::Button::Ok => "ok",
                            dialog::Button::Cancel => "cancel",
                            dialog::Button::Yes => "yes",
                            dialog::Button::No => "no",
                        }),
                    };
                    json!({ "id": completion.id.to_string(), "value": value })
                })
                .collect::<Vec<_>>();
            json_value(&mut engine, &json!(closed))
        }),
    )
}

// Nothing to install: `rfd` opens a macOS file dialog on the main thread, which
// is the thread this design deliberately leaves free to keep painting, and a
// Windows dialog was never verified here. Approximating either would be a
// different design wearing this one's name.
#[cfg(not(all(
    unix,
    not(any(target_os = "macos", target_os = "android", target_os = "ios"))
)))]
pub(super) fn install<E: JsEngine + 'static>(_engine: &mut E) -> Result<(), JsError> {
    Ok(())
}
