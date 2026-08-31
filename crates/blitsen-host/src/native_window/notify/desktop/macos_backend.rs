//! macOS specifics of the desktop notification backend: the bundle-identity
//! gate in front of `UNUserNotificationCenter`, the record a delivered
//! notification is addressed through, and the live-process response watcher.

use std::collections::VecDeque;
use std::sync::Arc;

use notify_rust::NotificationResponse;
use parking_lot::Mutex;
use serde_json::{Value, json};
use winit::event_loop::EventLoopProxy;

use super::{NO_BUNDLE_IDENTITY, Signal, SignalKind, queue};
use crate::dom_bridge::notify::NotificationOptions;

/// The library's own bundle check, ahead of anything that reaches the framework.
///
/// Only `permission` and `show` need it: every other entry point addresses a
/// notification that a `show` already got through, and a process cannot acquire
/// or lose a bundle identifier while it runs.
pub(super) fn bundle_identity() -> Result<(), String> {
    notify_rust::check_bundle().map_err(|_| NO_BUNDLE_IDENTITY.to_owned())
}

pub(super) struct Record {
    pub(super) options: NotificationOptions,
    pub(super) token: u64,
    pub(super) native_id: String,
    pub(super) nonce: Option<String>,
}

pub(super) fn permission(request: bool) -> Result<Value, String> {
    // From the crate root, not from `notify_rust::macos`: that module is
    // private (`notify-rust-4.18.0/src/lib.rs`, `mod macos;`) and only
    // its items are re-exported, so naming it never compiled — this
    // whole arm has been dead since #97 landed it, which is also why
    // #253's macOS gate is the first thing to run it.
    use mac_usernotifications::AuthorizationStatus;
    use notify_rust::{get_notification_settings_blocking, request_auth_blocking};

    bundle_identity()?;
    if request {
        return request_auth_blocking()
            .map(|granted| json!(if granted { "granted" } else { "denied" }))
            .map_err(|error| format!("could not request notification permission: {error}"));
    }
    let status = get_notification_settings_blocking()
        .map_err(|error| format!("could not read notification permission: {error}"))?
        .authorization_status;
    Ok(json!(match status {
        AuthorizationStatus::NotDetermined | AuthorizationStatus::Unknown => "default",
        AuthorizationStatus::Denied => "denied",
        AuthorizationStatus::Authorized
        | AuthorizationStatus::Provisional
        | AuthorizationStatus::Ephemeral => "granted",
    }))
}

pub(super) fn watch(
    handle: notify_rust::NotificationHandle,
    public_id: String,
    token: u64,
    signals: Arc<Mutex<VecDeque<Signal>>>,
    proxy: EventLoopProxy,
) {
    std::thread::spawn(move || {
        let result = handle.wait_for_response(|response: &NotificationResponse| {
            queue(
                &signals,
                &proxy,
                public_id.clone(),
                token,
                SignalKind::Response(response.clone()),
            );
        });
        if let Err(error) = result {
            queue(
                &signals,
                &proxy,
                public_id,
                token,
                SignalKind::Error(format!("could not observe notification response: {error}")),
            );
        }
    });
}
