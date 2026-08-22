//! Host half of the `native:` modules, below the namespace the bootstrap builds.
//!
//! Every function here is installed under a `__blitsenNative…` name and only if
//! this platform can implement it properly. That is what makes the namespace
//! honest: the bootstrap drops any member whose host function is missing, so a
//! capability this build does not have reads as `undefined` and feature
//! detection selects a fallback (COMPATIBILITY.md, "Capability tiers").
//!
//! Android is where that sentence stops being a formality, because the platform
//! answers "no" to most of it. What survives there is `os`, which reads the same
//! `/proc` a Linux desktop does. What does not is `app`, `clipboard`, `dialog`
//! and `window` — each absent for a reason its own module or `cfg` states, and
//! none of them absent merely because the port has not been written (#147).

use blitsen_js::{JsEngine, JsError};
#[cfg(not(target_os = "android"))]
use blitsen_js::{TypedArray, TypedArrayKind};
#[cfg(not(target_os = "android"))]
use blitsen_platform::PlatformError;
#[cfg(not(target_os = "android"))]
use blitsen_platform::app::{self, Directory};
#[cfg(not(target_os = "android"))]
use blitsen_platform::clipboard::{self, Image};
use blitsen_platform::os;
use serde_json::json;

use super::json_value;
#[cfg(not(target_os = "android"))]
use super::{argument, window};

#[cfg(not(target_os = "android"))]
fn failed(error: PlatformError) -> JsError {
    JsError::new(error.message().to_owned())
}

/// Installs the host functions the `native:` namespace is assembled from.
pub(super) fn install<E: JsEngine + 'static>(engine: &mut E) -> Result<(), JsError> {
    install_app(engine)?;
    install_clipboard(engine)?;
    install_window(engine)?;
    install_os(engine)?;
    install_dialog(engine)
}

// Every one of these answers a whole record at once rather than a field at a
// time. A monitor reads the processor once per tick and shows a dozen numbers
// off it; a getter per field would sample the machine a dozen times for one
// frame and hand back readings taken at different instants, which is a
// per-core usage list that does not add up to the total beside it.
fn install_os<E: JsEngine + 'static>(engine: &mut E) -> Result<(), JsError> {
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
                "language": super::intl::default_locale().to_string(),
                "timeZone": super::intl::default_time_zone(),
            });
            json_value(&mut engine, &locale)
        }),
    )
}

#[cfg(not(target_os = "android"))]
fn install_app<E: JsEngine + 'static>(engine: &mut E) -> Result<(), JsError> {
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

// Android is a unix and this would compile there, which is exactly why the
// predicate names it: the module it reaches into is absent on Android, and a
// lock nobody is racing for is not a capability. See the no-op `install_app`.
#[cfg(all(unix, not(target_os = "android")))]
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

// Nothing to install: a named mutex and a pipe are a different design, not this
// one with the socket swapped out.
#[cfg(not(unix))]
fn install_single_instance<E: JsEngine + 'static>(_engine: &mut E) -> Result<(), JsError> {
    Ok(())
}

// Nothing to install. Android is a unix, so the socket above would bind and the
// lock would be taken — and it would be answering a question nobody asked: an
// Android application is one process by construction, and a second launch is an
// `Intent` delivered to the instance already running rather than a command line
// to hand over. The directories are the Activity's and relaunch has no
// executable to spawn; `blitsen_platform::app` states the case for all three.
#[cfg(target_os = "android")]
fn install_app<E: JsEngine + 'static>(_engine: &mut E) -> Result<(), JsError> {
    Ok(())
}

#[cfg(not(target_os = "android"))]
fn install_clipboard<E: JsEngine + 'static>(engine: &mut E) -> Result<(), JsError> {
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
fn install_clipboard<E: JsEngine + 'static>(_engine: &mut E) -> Result<(), JsError> {
    Ok(())
}

/// The window this session already owns, never a second one: creating windows
/// waits on the shared-versus-isolated JavaScript context decision, and the
/// members that would need it are declared absent instead.
#[cfg(not(target_os = "android"))]
fn install_window<E: JsEngine + 'static>(engine: &mut E) -> Result<(), JsError> {
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
fn install_window<E: JsEngine + 'static>(_engine: &mut E) -> Result<(), JsError> {
    Ok(())
}

#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "android", target_os = "ios"))
))]
fn install_dialog<E: JsEngine + 'static>(engine: &mut E) -> Result<(), JsError> {
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
fn install_dialog<E: JsEngine + 'static>(_engine: &mut E) -> Result<(), JsError> {
    Ok(())
}
