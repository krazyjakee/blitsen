//! Desktop notification lifecycle built on `notify-rust`.
//!
//! The backend's callbacks run away from the JavaScript frame turn. They write
//! to `signals` and wake winit; [`NotifyController::poll`] is the only place
//! those callbacks become public events.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use notify_rust::{CloseReason, Notification, NotificationResponse, Timeout, Urgency};
use serde_json::{Value, json};
use winit::event_loop::EventLoopProxy;

use crate::dom_bridge::notify::{NotificationOptions, NotificationPatch};

#[derive(Debug)]
enum SignalKind {
    Response(NotificationResponse),
    Error(String),
}

#[derive(Debug)]
struct Signal {
    public_id: String,
    token: u64,
    kind: SignalKind,
}

#[cfg(target_os = "linux")]
struct Record {
    options: NotificationOptions,
    handle: notify_rust::NotificationHandle,
    token: u64,
}

#[cfg(target_os = "macos")]
struct Record {
    options: NotificationOptions,
    token: u64,
}

#[cfg(target_os = "windows")]
struct Record {
    token: u64,
}

pub(crate) struct NotifyController {
    proxy: EventLoopProxy,
    signals: Arc<Mutex<VecDeque<Signal>>>,
    records: HashMap<String, Record>,
    next_token: u64,
}

fn urgency(value: &str) -> Result<Urgency, String> {
    match value {
        "low" => Ok(Urgency::Low),
        "normal" => Ok(Urgency::Normal),
        "critical" => Ok(Urgency::Critical),
        other => Err(format!(
            "{other:?} is not a notification urgency: low, normal or critical"
        )),
    }
}

fn notification(options: &NotificationOptions) -> Result<Notification, String> {
    #[cfg(target_os = "macos")]
    if options.icon.is_some() {
        return Err("notification icons are not supported by macOS".into());
    }

    let mut notification = Notification::new();
    notification
        .summary(&options.title)
        .body(&options.body)
        .urgency(urgency(&options.urgency)?);
    if let Some(app_name) = &options.app_name {
        notification.appname(app_name);
    }
    if let Some(timeout) = options.timeout {
        notification.timeout(if timeout == 0 {
            Timeout::Never
        } else {
            Timeout::Milliseconds(timeout)
        });
    }
    if let Some(icon) = &options.icon {
        notification.icon(icon);
    }
    for action in &options.actions {
        notification.action(&action.id, &action.title);
    }
    Ok(notification)
}

fn close_reason(reason: CloseReason) -> &'static str {
    match reason {
        CloseReason::Expired => "expired",
        CloseReason::Dismissed => "dismissed",
        CloseReason::CloseAction => "closed",
        CloseReason::Other(_) => "unknown",
    }
}

fn queue(
    signals: &Mutex<VecDeque<Signal>>,
    proxy: &EventLoopProxy,
    public_id: String,
    token: u64,
    kind: SignalKind,
) {
    crate::dom_bridge::net_lock(signals).push_back(Signal {
        public_id,
        token,
        kind,
    });
    proxy.wake_up();
}

#[cfg(target_os = "macos")]
fn watch(
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

impl NotifyController {
    pub(crate) fn new(proxy: EventLoopProxy) -> Self {
        Self {
            proxy,
            signals: Arc::new(Mutex::new(VecDeque::new())),
            records: HashMap::new(),
            next_token: 1,
        }
    }

    fn token(&mut self) -> u64 {
        let token = self.next_token;
        self.next_token = token.saturating_add(1);
        token
    }

    pub(crate) fn permission(request: bool) -> Result<Value, String> {
        #[cfg(target_os = "linux")]
        {
            let _ = request;
            Ok(json!("granted"))
        }

        #[cfg(target_os = "macos")]
        {
            use mac_usernotifications::AuthorizationStatus;
            use notify_rust::macos::{get_notification_settings_blocking, request_auth_blocking};

            if request {
                return request_auth_blocking()
                    .map(|granted| json!(if granted { "granted" } else { "denied" }))
                    .map_err(|error| {
                        format!("could not request notification permission: {error}")
                    });
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

        #[cfg(target_os = "windows")]
        {
            // Windows has no programmatic prompt for unpackaged desktop apps.
            // `notify-rust` reports submission failures, so requesting is the
            // same non-mutating reading until its backend exposes Setting().
            let _ = request;
            Ok(json!("default"))
        }
    }

    pub(crate) fn request_permission(&mut self, command_id: u64) {
        crate::dom_bridge::notify::complete(command_id, Self::permission(true));
    }

    pub(crate) fn show(
        &mut self,
        public_id: String,
        options: NotificationOptions,
    ) -> Result<Value, String> {
        #[allow(unused_mut)]
        let mut spec = notification(&options)?;
        #[cfg(target_os = "macos")]
        spec.id(public_id.clone());
        let handle = spec
            .show()
            .map_err(|error| format!("could not show notification: {error}"))?;
        let token = self.token();

        #[cfg(target_os = "linux")]
        {
            let native_id = handle.id();
            let signals = Arc::clone(&self.signals);
            let proxy = self.proxy.clone();
            let watched_id = public_id.clone();
            std::thread::spawn(move || {
                #[allow(deprecated)]
                let result = notify_rust::handle_action(native_id, |response| {
                    #[allow(deprecated)]
                    let response = match response {
                        notify_rust::ActionResponse::Custom("default") => {
                            NotificationResponse::Default
                        }
                        notify_rust::ActionResponse::Custom(action) => {
                            NotificationResponse::Action((*action).to_owned())
                        }
                        notify_rust::ActionResponse::Closed(reason) => {
                            NotificationResponse::Closed(*reason)
                        }
                    };
                    queue(
                        &signals,
                        &proxy,
                        watched_id.clone(),
                        token,
                        SignalKind::Response(response),
                    );
                });
                if let Err(error) = result {
                    queue(
                        &signals,
                        &proxy,
                        watched_id,
                        token,
                        SignalKind::Error(format!(
                            "could not observe notification response: {error}"
                        )),
                    );
                }
            });
            self.records.insert(
                public_id.clone(),
                Record {
                    options,
                    handle,
                    token,
                },
            );
        }

        #[cfg(target_os = "macos")]
        {
            watch(
                handle,
                public_id.clone(),
                token,
                Arc::clone(&self.signals),
                self.proxy.clone(),
            );
            self.records
                .insert(public_id.clone(), Record { options, token });
        }

        #[cfg(target_os = "windows")]
        {
            let signals = Arc::clone(&self.signals);
            let proxy = self.proxy.clone();
            let watched_id = public_id.clone();
            std::thread::spawn(move || {
                let result = handle.wait_for_response(|response: &NotificationResponse| {
                    queue(
                        &signals,
                        &proxy,
                        watched_id.clone(),
                        token,
                        SignalKind::Response(response.clone()),
                    );
                });
                if let Err(error) = result {
                    queue(
                        &signals,
                        &proxy,
                        watched_id,
                        token,
                        SignalKind::Error(format!(
                            "could not observe notification response: {error}"
                        )),
                    );
                }
            });
            let _ = options;
            self.records.insert(public_id.clone(), Record { token });
        }

        Ok(json!(public_id))
    }

    pub(crate) fn update(
        &mut self,
        public_id: &str,
        patch: NotificationPatch,
    ) -> Result<Value, String> {
        #[cfg(target_os = "windows")]
        {
            let _ = patch;
            return if self.records.contains_key(public_id) {
                Err(
                    "notification update is not supported by the notify-rust Windows backend"
                        .into(),
                )
            } else {
                Ok(json!(false))
            };
        }

        #[cfg(not(target_os = "windows"))]
        let Some(record) = self.records.get_mut(public_id) else {
            return Ok(json!(false));
        };
        #[cfg(not(target_os = "windows"))]
        record.options.apply(patch);

        #[cfg(target_os = "linux")]
        {
            let native_id = record.handle.id();
            let mut spec = notification(&record.options)?;
            spec.id(native_id);
            *record.handle = spec;
            record
                .handle
                .update()
                .map_err(|error| format!("could not update notification {public_id}: {error}"))?;
            Ok(json!(true))
        }

        #[cfg(target_os = "macos")]
        {
            let mut spec = notification(&record.options)?;
            spec.id(public_id);
            let handle = spec
                .show()
                .map_err(|error| format!("could not update notification {public_id}: {error}"))?;
            let token = self.token();
            self.records
                .get_mut(public_id)
                .expect("record still exists")
                .token = token;
            watch(
                handle,
                public_id.to_owned(),
                token,
                Arc::clone(&self.signals),
                self.proxy.clone(),
            );
            Ok(json!(true))
        }
    }

    pub(crate) fn close(&mut self, public_id: &str) -> Result<Value, String> {
        #[cfg(target_os = "windows")]
        {
            if self.records.contains_key(public_id) {
                return Err(
                    "notification close is not supported by the notify-rust Windows backend".into(),
                );
            }
            return Ok(json!(false));
        }

        #[cfg(not(target_os = "windows"))]
        let Some(record) = self.records.remove(public_id) else {
            return Ok(json!(false));
        };

        #[cfg(target_os = "linux")]
        record.handle.close();
        #[cfg(target_os = "macos")]
        mac_usernotifications::blocking::close_delivered(public_id);

        #[cfg(not(target_os = "windows"))]
        {
            crate::dom_bridge::notify::closed(public_id.to_owned(), "closed");
            Ok(json!(true))
        }
    }

    pub(crate) fn poll(&mut self) {
        let signals = crate::dom_bridge::net_lock(&self.signals)
            .drain(..)
            .collect::<Vec<_>>();
        for signal in signals {
            if self
                .records
                .get(&signal.public_id)
                .is_none_or(|record| record.token != signal.token)
            {
                continue;
            }
            match signal.kind {
                SignalKind::Response(NotificationResponse::Default) => {
                    self.records.remove(&signal.public_id);
                    crate::dom_bridge::notify::clicked(signal.public_id);
                }
                SignalKind::Response(NotificationResponse::Action(action)) => {
                    self.records.remove(&signal.public_id);
                    crate::dom_bridge::notify::action(signal.public_id, action);
                }
                SignalKind::Response(NotificationResponse::Reply(reply)) => {
                    self.records.remove(&signal.public_id);
                    crate::dom_bridge::notify::action(signal.public_id, reply);
                }
                SignalKind::Response(NotificationResponse::Closed(reason)) => {
                    self.records.remove(&signal.public_id);
                    crate::dom_bridge::notify::closed(signal.public_id, close_reason(reason));
                }
                SignalKind::Error(message) => {
                    self.records.remove(&signal.public_id);
                    crate::dom_bridge::notify::failed(signal.public_id, message);
                }
            }
        }
    }

    pub(crate) fn clear(&mut self) {
        let ids = self.records.keys().cloned().collect::<Vec<_>>();
        for id in ids {
            let _ = self.close(&id);
        }
        self.records.clear();
        crate::dom_bridge::net_lock(&self.signals).clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_patch_preserves_unspecified_values() {
        let mut options = NotificationOptions {
            title: "Before".into(),
            body: "Body".into(),
            app_name: Some("Demo".into()),
            timeout: Some(1000),
            urgency: "normal".into(),
            icon: None,
            actions: vec![],
        };
        options.apply(NotificationPatch {
            title: Some("After".into()),
            body: None,
            app_name: None,
            timeout: None,
            urgency: Some("critical".into()),
            icon: None,
            actions: None,
        });
        assert_eq!(options.title, "After");
        assert_eq!(options.body, "Body");
        assert_eq!(options.app_name.as_deref(), Some("Demo"));
        assert_eq!(options.timeout, Some(1000));
        assert_eq!(options.urgency, "critical");
    }
}
