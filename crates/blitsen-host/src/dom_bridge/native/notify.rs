use std::collections::HashSet;

use blitsen_js::{JsEngine, JsError};
use serde_json::json;

use super::super::notify::{self, NotificationAction, NotificationOptions, NotificationPatch};
use super::super::{argument, json_value};

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

pub(super) fn install<E: JsEngine + 'static>(engine: &mut E) -> Result<(), JsError> {
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
            command(&mut engine, notify::show(options))
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
            command(&mut engine, notify::request_permission())
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
    // Android joins them when the activation contract is present (#252). The
    // facade's `click` event is a body tap, and a body tap on Android is a
    // `PendingIntent` addressed to an installed application identity — without
    // one there is nothing for the tap to come back to, and a `Notification`
    // whose `onclick` could never fire is a promise the constructor should not
    // make. The identity is installed by the window session before the document
    // loads, so this reads a decision already taken rather than one it makes.
    #[cfg(target_os = "android")]
    if crate::native_window::notify::entry_point().is_some() {
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
            command(&mut engine, notify::update(public_id, patch))
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeNotifyClose",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let public_id = argument(&mut engine, &call, 0, "notification id")?;
            command(&mut engine, notify::close(public_id))
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeNotifyPending",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            Ok(engine.boolean(notify::pending()))
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeNotifyTake",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            json_value(&mut engine, &json!(notify::take_messages()))
        }),
    )
}
