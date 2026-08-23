//! Host half of the `native:` modules, below the namespace the bootstrap builds.
//!
//! Every function here is installed under a `__blitsenNative…` name and only if
//! this platform can implement it properly. That is what makes the namespace
//! honest: the bootstrap drops any member whose host function is missing, so a
//! capability this build does not have reads as `undefined` and feature
//! detection selects a fallback (COMPATIBILITY.md, "Capability tiers").
//!
//! Android is where that sentence stops being a formality, because the platform
//! answers "no" to most of it. `os`, focused `input` snapshots, notifications
//! and — since #248 gave it a `UsbManager` backend — raw HID survive there; app,
//! clipboard, dialog, window and tray remain absent for the reasons their module
//! or platform documentation states.

use blitsen_js::{JsEngine, JsError, TypedArray, TypedArrayKind};
#[cfg(not(target_os = "android"))]
use blitsen_platform::PlatformError;
#[cfg(not(target_os = "android"))]
use blitsen_platform::app::{self, Directory};
#[cfg(not(target_os = "android"))]
use blitsen_platform::clipboard::{self, Image};
use blitsen_platform::os;
use serde_json::json;

#[cfg(not(target_os = "android"))]
use super::window;
use super::{argument, json_value};

#[cfg(not(target_os = "android"))]
fn failed(error: PlatformError) -> JsError {
    JsError::new(error.message().to_owned())
}

/// Installs the host functions the `native:` namespace is assembled from.
pub(super) fn install<E: JsEngine + 'static>(engine: &mut E) -> Result<(), JsError> {
    install_app(engine)?;
    install_clipboard(engine)?;
    install_window(engine)?;
    install_tray(engine)?;
    install_menu(engine)?;
    install_hid(engine)?;
    install_notify(engine)?;
    install_input(engine)?;
    install_os(engine)?;
    install_dialog(engine)
}

fn install_notify<E: JsEngine + 'static>(engine: &mut E) -> Result<(), JsError> {
    use std::collections::HashSet;

    use super::notify::{NotificationAction, NotificationOptions, NotificationPatch};

    fn validate_actions(actions: &[NotificationAction]) -> Result<(), JsError> {
        if actions.len() > 8 {
            return Err(JsError::new("notifications may contain at most 8 actions"));
        }
        let mut ids = HashSet::new();
        for action in actions {
            if action.id.is_empty() || action.title.is_empty() {
                return Err(JsError::new(
                    "notification action ids and titles must not be empty",
                ));
            }
            if action.id == "default" || !ids.insert(&action.id) {
                return Err(JsError::new(format!(
                    "notification action id {:?} is reserved or duplicated",
                    action.id
                )));
            }
        }
        Ok(())
    }

    fn validate_options(options: &NotificationOptions) -> Result<(), JsError> {
        if options.title.is_empty() {
            return Err(JsError::new("a notification needs a non-empty title"));
        }
        if !matches!(options.urgency.as_str(), "low" | "normal" | "critical") {
            return Err(JsError::new(format!(
                "{:?} is not a notification urgency: low, normal or critical",
                options.urgency
            )));
        }
        validate_actions(&options.actions)
    }

    fn command<E: JsEngine>(engine: &mut E, id: u64) -> Result<E::Value, JsError> {
        engine.string(&id.to_string())
    }

    engine.define_global_function(
        "__blitsenNativeNotifyShow",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let options: NotificationOptions =
                serde_json::from_str(&argument(&mut engine, &call, 0, "notification options")?)
                    .map_err(|error| {
                        JsError::new(format!("malformed notification options: {error}"))
                    })?;
            validate_options(&options)?;
            command(&mut engine, super::notify::show(options))
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeNotifyPermission",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let permission = crate::native_window::notify::NotifyController::permission(false)
                .map_err(JsError::new)?;
            engine.string(
                permission
                    .as_str()
                    .expect("notification permission is always a string"),
            )
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeNotifyRequestPermission",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            command(&mut engine, super::notify::request_permission())
        }),
    )?;

    // The standard facade needs a backend that can withdraw what it showed,
    // because `Notification.close()` is not optional in that contract. Windows
    // joins Linux here now that a toast is addressable by the tag `show` gave it
    // (#251).
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    engine.define_global_function(
        "__blitsenNativeNotifyStandard",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            Ok(engine.boolean(true))
        }),
    )?;
    #[cfg(target_os = "macos")]
    if notify_rust::check_bundle().is_ok() {
        engine.define_global_function(
            "__blitsenNativeNotifyStandard",
            Box::new(move |call| {
                let mut engine = E::from_value(&call.this);
                Ok(engine.boolean(true))
            }),
        )?;
    }

    engine.define_global_function(
        "__blitsenNativeNotifyUpdate",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let public_id = argument(&mut engine, &call, 0, "notification id")?;
            let patch: NotificationPatch =
                serde_json::from_str(&argument(&mut engine, &call, 1, "notification update")?)
                    .map_err(|error| {
                        JsError::new(format!("malformed notification update: {error}"))
                    })?;
            if patch.title.as_deref() == Some("") {
                return Err(JsError::new("a notification title must not be empty"));
            }
            if let Some(urgency) = patch.urgency.as_deref()
                && !matches!(urgency, "low" | "normal" | "critical")
            {
                return Err(JsError::new(format!(
                    "{urgency:?} is not a notification urgency: low, normal or critical"
                )));
            }
            if let Some(actions) = &patch.actions {
                validate_actions(actions)?;
            }
            command(&mut engine, super::notify::update(public_id, patch))
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeNotifyClose",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let public_id = argument(&mut engine, &call, 0, "notification id")?;
            command(&mut engine, super::notify::close(public_id))
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeNotifyPending",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            Ok(engine.boolean(super::notify::pending()))
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeNotifyTake",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            json_value(&mut engine, &json!(super::notify::take_messages()))
        }),
    )
}

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

fn install_hid<E: JsEngine + 'static>(engine: &mut E) -> Result<(), JsError> {
    fn command<E: JsEngine>(engine: &mut E, id: u64) -> Result<E::Value, JsError> {
        engine.string(&id.to_string())
    }

    engine.define_global_function(
        "__blitsenNativeHidDevices",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            command(&mut engine, super::hid::devices())
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeHidOpen",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let device_id = argument(&mut engine, &call, 0, "HID device id")?;
            command(&mut engine, super::hid::open(device_id))
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeHidClose",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let device_id = argument(&mut engine, &call, 0, "HID device id")?;
            command(&mut engine, super::hid::close(device_id))
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeHidWrite",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let device_id = argument(&mut engine, &call, 0, "HID device id")?;
            let data = report_argument(&mut engine, &call, 1)?;
            command(&mut engine, super::hid::write(device_id, data))
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeHidSendFeatureReport",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let device_id = argument(&mut engine, &call, 0, "HID device id")?;
            let data = report_argument(&mut engine, &call, 1)?;
            command(&mut engine, super::hid::send_feature_report(device_id, data))
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
                super::hid::receive_feature_report(device_id, report_id),
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
            super::hid::watch(watching);
            Ok(engine.undefined())
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeHidPending",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            Ok(engine.boolean(super::hid::pending()))
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
            for message in super::hid::take_messages() {
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

fn install_input<E: JsEngine + 'static>(engine: &mut E) -> Result<(), JsError> {
    engine.define_global_function(
        "__blitsenNativeInputSnapshot",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            json_value(&mut engine, &super::input::snapshot())
        }),
    )
}

#[cfg(not(target_os = "android"))]
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrayBridgeOptions {
    tooltip: Option<String>,
    open_on_click: bool,
    close_to_tray: bool,
    menu: Vec<TrayBridgeItem>,
}

#[cfg(not(target_os = "android"))]
type TrayBridgeItem = crate::MenuDefinition;

#[cfg(not(target_os = "android"))]
fn parse_tray_menu(
    raw: Vec<TrayBridgeItem>,
    icons: &[Vec<u8>],
) -> Result<(Vec<crate::native_window::menu::MenuEntry>, bool), JsError> {
    crate::native_window::menu::parse_menu(raw, icons, crate::native_window::menu::MenuSurface::Tray)
        .map_err(JsError::new)
}

#[cfg(not(target_os = "android"))]
fn install_tray<E: JsEngine + 'static>(engine: &mut E) -> Result<(), JsError> {
    use crate::native_window::tray::TraySpec;

    engine.define_global_function(
        "__blitsenNativeTrayConfigure",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let options: TrayBridgeOptions =
                serde_json::from_str(&argument(&mut engine, &call, 0, "tray options")?)
                    .map_err(|error| JsError::new(format!("malformed tray options: {error}")))?;
            let icon = call
                .arguments
                .get(1)
                .ok_or_else(|| JsError::new("missing tray icon bytes"))?;
            let icon = engine.to_typed_array(icon)?;
            if !matches!(
                icon.kind,
                TypedArrayKind::Uint8 | TypedArrayKind::Uint8Clamped
            ) {
                return Err(JsError::new(
                    "tray icon must be a Uint8Array or Uint8ClampedArray",
                ));
            }

            let menu_icons = call
                .arguments
                .iter()
                .skip(2)
                .map(|value| {
                    let icon = engine.to_typed_array(value)?;
                    if !matches!(
                        icon.kind,
                        TypedArrayKind::Uint8 | TypedArrayKind::Uint8Clamped
                    ) {
                        return Err(JsError::new(
                            "tray menu icons must be Uint8Array or Uint8ClampedArray values",
                        ));
                    }
                    Ok(icon.bytes)
                })
                .collect::<Result<Vec<_>, JsError>>()?;
            let (menu, has_quit) = parse_tray_menu(options.menu, &menu_icons)?;
            if options.close_to_tray && !has_quit {
                return Err(JsError::new(
                    "closeToTray requires a quit action in the tray menu",
                ));
            }
            let id = super::tray::configure(TraySpec {
                icon: icon.bytes,
                tooltip: options.tooltip,
                open_on_click: options.open_on_click,
                close_to_tray: options.close_to_tray,
                menu,
            });
            engine.string(&id.to_string())
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeTrayRemove",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            engine.string(&super::tray::remove().to_string())
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeTrayPending",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            Ok(engine.boolean(super::tray::pending()))
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeTrayTake",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            json_value(&mut engine, &json!(super::tray::take_messages()))
        }),
    )
}

#[cfg(target_os = "android")]
fn install_tray<E: JsEngine + 'static>(_engine: &mut E) -> Result<(), JsError> {
    Ok(())
}

// The application menu, installed only where a platform has one to install.
// macOS has NSApp's main menu, and Windows has a per-window menu bar; on Linux
// muda's only backend is a `gtk::MenuBar` added to a `gtk::Window`, and Blitsen
// windows are winit's. That is the whole argument, and `native-modules.mjs`
// carries it: a menu that appeared here would be a tray menu wearing the name.
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn install_menu<E: JsEngine + 'static>(engine: &mut E) -> Result<(), JsError> {
    engine.define_global_function(
        "__blitsenNativeMenuConfigure",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let entries: Vec<crate::MenuDefinition> =
                serde_json::from_str(&argument(&mut engine, &call, 0, "menu entries")?)
                    .map_err(|error| {
                        JsError::new(format!("malformed application menu: {error}"))
                    })?;
            // Parsed here rather than when the request is applied, so a tree
            // the platform cannot install is a rejected promise naming what is
            // wrong with it rather than a menu that half appeared.
            crate::native_window::menu::parse_menu(
                entries.clone(),
                &[],
                crate::native_window::menu::MenuSurface::Application,
            )
            .map_err(JsError::new)?;
            engine.string(&super::menu::configure(entries).to_string())
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeMenuRemove",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            engine.string(&super::menu::remove().to_string())
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeMenuPending",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            Ok(engine.boolean(super::menu::pending()))
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeMenuTake",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            json_value(&mut engine, &json!(super::menu::take_messages()))
        }),
    )
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn install_menu<E: JsEngine + 'static>(_engine: &mut E) -> Result<(), JsError> {
    Ok(())
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

#[cfg(all(test, not(target_os = "android")))]
mod tray_tests {
    use super::*;
    use crate::native_window::menu::{MenuEntry, MenuItemKind, MenuSignal};

    fn action(id: &str) -> TrayBridgeItem {
        TrayBridgeItem {
            id: Some(id.into()),
            label: Some(id.into()),
            ..Default::default()
        }
    }

    fn radio(id: &str, group: &str, checked: bool) -> TrayBridgeItem {
        TrayBridgeItem {
            kind: Some("radio".into()),
            id: Some(id.into()),
            label: Some(id.into()),
            group: Some(group.into()),
            checked: Some(checked),
            ..Default::default()
        }
    }

    #[test]
    fn nested_checkable_menu_keeps_public_identity_and_state() {
        let raw = vec![
            action("open"),
            TrayBridgeItem {
                kind: Some("submenu".into()),
                label: Some("Theme".into()),
                menu: Some(vec![
                    radio("light", "theme", true),
                    radio("dark", "theme", false),
                ]),
                ..Default::default()
            },
            TrayBridgeItem {
                kind: Some("checkbox".into()),
                id: Some("launch".into()),
                label: Some("Launch".into()),
                checked: Some(true),
                ..Default::default()
            },
        ];
        let (menu, has_quit) = parse_tray_menu(raw, &[]).expect("the tree is valid");
        assert!(!has_quit);
        let MenuEntry::Submenu { menu: theme, .. } = &menu[1] else {
            panic!("the second entry is the theme submenu")
        };
        let MenuEntry::Item(dark) = &theme[1] else {
            panic!("the second theme entry is an item")
        };
        assert_eq!(
            dark.signal,
            MenuSignal::Action {
                id: "dark".into(),
                checked: Some(false),
            }
        );
        assert_eq!(
            dark.kind,
            MenuItemKind::Radio {
                group: "theme".into(),
                checked: false,
            }
        );
    }

    #[test]
    fn ids_are_unique_across_submenus() {
        let raw = vec![
            action("open"),
            TrayBridgeItem {
                kind: Some("submenu".into()),
                label: Some("More".into()),
                menu: Some(vec![action("open")]),
                ..Default::default()
            },
        ];
        assert!(parse_tray_menu(raw, &[]).is_err());
    }

    #[test]
    fn radio_groups_are_consecutive_and_have_one_selection() {
        assert!(
            parse_tray_menu(
                vec![
                    radio("light", "theme", false),
                    radio("dark", "theme", false)
                ],
                &[],
            )
            .is_err()
        );
        assert!(
            parse_tray_menu(
                vec![
                    radio("light", "theme", true),
                    action("open"),
                    radio("dark", "theme", false),
                ],
                &[],
            )
            .is_err()
        );
    }

    #[test]
    fn accelerator_has_modifiers_before_one_key() {
        let mut valid = action("open");
        valid.accelerator = Some("CmdOrCtrl+Shift+KeyO".into());
        assert!(parse_tray_menu(vec![valid], &[]).is_ok());

        let mut invalid = action("open");
        invalid.accelerator = Some("KeyO+Control".into());
        assert!(parse_tray_menu(vec![invalid], &[]).is_err());
    }
}
