//! Desktop notification lifecycle built on `notify-rust`.
//!
//! The backend's callbacks run away from the JavaScript frame turn. They write
//! to `signals` and wake winit; [`NotifyController::poll`] is the only place
//! those callbacks become public events.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use notify_rust::{CloseReason, NotificationResponse, Urgency};
#[cfg(not(target_os = "windows"))]
use notify_rust::{Notification, Timeout};
use parking_lot::Mutex;
use serde_json::{Value, json};
use winit::event_loop::EventLoopProxy;

#[cfg(any(target_os = "windows", target_os = "macos"))]
use crate::dom_bridge::notify::Activation;
use crate::dom_bridge::notify::{NotificationOptions, NotificationPatch};

#[cfg(target_os = "linux")]
mod linux_backend;
#[cfg(target_os = "macos")]
mod macos_backend;
#[cfg(target_os = "windows")]
mod windows_backend;

#[cfg(target_os = "linux")]
use linux_backend::{LinuxHandle, Record};
#[cfg(target_os = "macos")]
use macos_backend::Record;
#[cfg(target_os = "windows")]
use windows_backend::Record;
#[cfg(target_os = "windows")]
pub(super) use windows_backend::register_entry_point;

#[derive(Debug)]
enum SignalKind {
    Response(NotificationResponse),
    Error(String),
}

/// What a macOS process without an application identity is told (#253).
///
/// Apple gates `UNUserNotificationCenter` on a bundle identifier and a
/// signature, and a development run is an interpreter executing a script, so it
/// has neither — the API aborts the process rather than answering a caller
/// without one. The alternative the library still carries, `mac-notification-sys`
/// with `get_bundle_identifier_or_default`, submits under an installed
/// application's identifier instead: the notification is then attributed to
/// Terminal, and the permission the user granted was granted to Terminal. That
/// is not a fallback, it is impersonation, so what this points at is an identity
/// the development host owns.
///
/// Compiled into the test build on every platform, because this sentence is the
/// whole of what a developer without a bundle receives and a message only macOS
/// could compile is a message nothing checks.
#[cfg(any(target_os = "macos", test))]
const NO_BUNDLE_IDENTITY: &str = concat!(
    "macOS notifications need an application bundle identity, and this process has no ",
    "CFBundleIdentifier for UNUserNotificationCenter to address or hold permission against. ",
    "Give the development host one of its own with `blitsen --dev-bundle`, which builds and ",
    "re-executes into a signed development .app, or run an application exported by ",
    "`blitsen build --bundle-id <id> --sign <command>`.",
);

#[derive(Debug)]
struct Signal {
    public_id: String,
    token: u64,
    kind: SignalKind,
}

/// What a Windows process with no registered application identity is told (#251).
///
/// Windows keeps a notifier — and the permission that notifier reports — per
/// AppUserModelID, and it keeps none for an identity nothing ever registered.
/// Calling that `"denied"` would report a decision no user, administrator or
/// policy made, and returning the raw `0x80070490` reports only that something
/// went wrong; neither tells the reader that what is missing is a prerequisite
/// of the platform rather than a permission. `blitsen/hid` already separates a
/// refusal from a broken backend for the same reason, and #253 gave the absent
/// macOS bundle this same shape: a missing identity, named as one.
///
/// Compiled into the test build on every platform, for the reason
/// `NO_BUNDLE_IDENTITY` is: this sentence is the whole of what a caller in
/// that environment receives, and a message only Windows could compile is a
/// message only Windows could check.
#[cfg(any(target_os = "windows", test))]
const NO_TOAST_IDENTITY: &str = concat!(
    "Windows notifications are delivered under an application identity the notification ",
    "platform knows, and no AppUserModelID is registered for this process, so Windows holds no ",
    "notifier whose permission could be read. An application exported with `blitsen build ",
    "--bundle-id <id>` registers an identity of its own at startup (#252); a development run has ",
    "none and borrows Windows PowerShell's, which an image stripped of its Start Menu entries — a ",
    "CI runner, a Server Core install — does not carry either.",
);

pub(crate) struct NotifyController {
    proxy: EventLoopProxy,
    signals: Arc<Mutex<VecDeque<Signal>>>,
    records: HashMap<String, Record>,
    next_token: u64,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    notification_session: String,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    activation_store: Option<super::ActivationStore>,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    activation_errors: Arc<Mutex<VecDeque<(String, String)>>>,
    #[cfg(target_os = "windows")]
    _com_server: Option<super::windows_activation::ComServer>,
    #[cfg(target_os = "linux")]
    portal: Result<Option<super::linux_portal::LinuxPortal>, String>,
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

#[cfg(not(target_os = "windows"))]
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
    // `DesktopEntry` identifies the installed application; `appname` above is
    // only a display string. Notification servers use this hint for attribution
    // and, where supported, to locate the application's entry point.
    #[cfg(target_os = "linux")]
    if let Some(entry_point) = super::entry_point() {
        notification.hint(notify_rust::Hint::DesktopEntry(entry_point.entry.clone()));
    }
    for action in &options.actions {
        notification.action(&action.id, &action.title);
    }
    Ok(notification)
}

/// The persisted envelope represented by one generation of a desktop
/// notification. `None` is the intentional development mode: no installed
/// identity means there is no stopped application for the platform to address.
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn activation(
    public_id: &str,
    action: Option<&str>,
    session: &str,
    generation: u64,
    platform: &str,
) -> Option<Activation> {
    let entry = super::entry_point()?;
    Some(Activation {
        nonce: super::generation_nonce(session, generation),
        identity: entry.identity.clone(),
        id: public_id.to_owned(),
        session: Some(session.to_owned()),
        action: action.map(str::to_owned),
        dismissed: None,
        platform: platform.to_owned(),
        entry: entry.entry.clone(),
    })
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
    signals.lock().push_back(Signal {
        public_id,
        token,
        kind,
    });
    proxy.wake_up();
}

impl NotifyController {
    pub(crate) fn new(proxy: EventLoopProxy) -> Self {
        #[cfg(target_os = "linux")]
        let portal = super::linux_portal::LinuxPortal::new(proxy.clone());
        let signals = Arc::new(Mutex::new(VecDeque::new()));
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        let notification_session = super::session_token();
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        let activation_errors = Arc::new(Mutex::new(VecDeque::new()));
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        let activation_location =
            super::entry_point().and_then(|entry| match super::store_directory(&entry.identity) {
                Ok(directory) => Some((entry.clone(), directory)),
                Err(error) => {
                    activation_errors.lock().push_back((String::new(), error));
                    None
                }
            });
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        let activation_store = activation_location
            .as_ref()
            .map(|(entry, directory)| super::ActivationStore::new(directory, &entry.identity));
        #[cfg(target_os = "macos")]
        if let Some((entry, directory)) = &activation_location {
            super::macos_activation::install(
                directory.clone(),
                entry.identity.clone(),
                entry.entry.clone(),
                Arc::clone(&activation_errors),
                proxy.clone(),
            );
        }
        #[cfg(target_os = "windows")]
        let com_server = activation_location.as_ref().and_then(|(entry, directory)| {
            match super::windows_activation::ComServer::start(
                directory.clone(),
                entry.identity.clone(),
                Arc::clone(&activation_errors),
                proxy.clone(),
            ) {
                Ok(server) => Some(server),
                Err(error) => {
                    activation_errors.lock().push_back((String::new(), error));
                    None
                }
            }
        });
        Self {
            proxy,
            signals,
            records: HashMap::new(),
            next_token: 1,
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            notification_session,
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            activation_store,
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            activation_errors,
            #[cfg(target_os = "windows")]
            _com_server: com_server,
            #[cfg(target_os = "linux")]
            portal,
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
            linux_backend::permission(request)
        }

        #[cfg(target_os = "macos")]
        {
            macos_backend::permission(request)
        }

        #[cfg(target_os = "windows")]
        {
            windows_backend::permission(request)
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
        // Refused before anything is built: without a bundle identity the
        // framework `show` would reach does not return an error, it aborts the
        // process.
        #[cfg(target_os = "macos")]
        macos_backend::bundle_identity()?;
        let token = self.token();
        #[cfg(target_os = "macos")]
        let mac_activation =
            activation(&public_id, None, &self.notification_session, token, "macos");
        #[cfg(target_os = "macos")]
        let native_id = mac_activation
            .as_ref()
            .map_or_else(|| public_id.clone(), super::encode_desktop_envelope);
        #[cfg(target_os = "macos")]
        let handle = {
            let mut spec = notification(&options)?;
            spec.id(native_id.clone());
            spec.show()
                .map_err(|error| format!("could not show notification: {error}"))?
        };
        // Windows registers the response handlers on the notifier before the
        // toast reaches the platform, so the token they report at has to exist
        // first. Its encoded launch argument is also the durable cold-start
        // envelope the COM callback receives.
        #[cfg(target_os = "windows")]
        let (toast, nonce) =
            windows_backend::toast(&public_id, &options, &self.notification_session, token)?;

        #[cfg(target_os = "linux")]
        {
            let handle = match &self.portal {
                Ok(Some(portal)) => {
                    portal.show(&public_id, &options)?;
                    LinuxHandle::Portal
                }
                Ok(None) => {
                    let handle = notification(&options)?
                        .show()
                        .map_err(|error| format!("could not show notification: {error}"))?;
                    linux_backend::watch(
                        handle.id(),
                        public_id.clone(),
                        token,
                        Arc::clone(&self.signals),
                        self.proxy.clone(),
                    );
                    LinuxHandle::Freedesktop(Box::new(handle))
                }
                Err(error) => return Err(error.clone()),
            };
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
            let nonce = mac_activation.map(|activation| activation.nonce);
            // An identity-less development bundle has no durable activation
            // envelope, so it retains the dependency's live-process watcher.
            if nonce.is_none() {
                macos_backend::watch(
                    handle,
                    public_id.clone(),
                    token,
                    Arc::clone(&self.signals),
                    self.proxy.clone(),
                );
            }
            self.records.insert(
                public_id.clone(),
                Record {
                    options,
                    token,
                    native_id,
                    nonce,
                },
            );
        }

        #[cfg(target_os = "windows")]
        {
            windows_backend::notifier(&public_id, token, &self.signals, &self.proxy)
                .show(&toast)
                .map_err(|error| format!("could not show notification: {error}"))?;
            self.records.insert(
                public_id.clone(),
                Record {
                    options,
                    token,
                    nonce,
                },
            );
        }

        Ok(json!(public_id))
    }

    pub(crate) fn update(
        &mut self,
        public_id: &str,
        patch: NotificationPatch,
    ) -> Result<Value, String> {
        let Some(record) = self.records.get_mut(public_id) else {
            return Ok(json!(false));
        };
        record.options.apply(patch);

        #[cfg(target_os = "linux")]
        {
            match &mut record.handle {
                LinuxHandle::Portal => self
                    .portal
                    .as_ref()
                    .map_err(Clone::clone)?
                    .as_ref()
                    .expect("a portal record has a portal")
                    .show(public_id, &record.options)?,
                LinuxHandle::Freedesktop(handle) => {
                    let native_id = handle.id();
                    let mut spec = notification(&record.options)?;
                    spec.id(native_id);
                    ***handle = spec;
                    handle.update().map_err(|error| {
                        format!("could not update notification {public_id}: {error}")
                    })?;
                }
            }
            Ok(json!(true))
        }

        #[cfg(target_os = "macos")]
        {
            let options = record.options.clone();
            let old_native_id = record.native_id.clone();
            let token = self.token();
            let activation =
                activation(public_id, None, &self.notification_session, token, "macos");
            let native_id = activation
                .as_ref()
                .map_or_else(|| public_id.to_owned(), super::encode_desktop_envelope);
            let mut spec = notification(&options)?;
            spec.id(native_id.clone());
            let handle = spec
                .show()
                .map_err(|error| format!("could not update notification {public_id}: {error}"))?;
            let nonce = activation.map(|activation| activation.nonce);
            if nonce.is_none() {
                macos_backend::watch(
                    handle,
                    public_id.to_owned(),
                    token,
                    Arc::clone(&self.signals),
                    self.proxy.clone(),
                );
            }
            if old_native_id != native_id {
                mac_usernotifications::blocking::close_delivered(&old_native_id);
            }
            let record = self
                .records
                .get_mut(public_id)
                .expect("record still exists");
            record.token = token;
            record.native_id = native_id;
            record.nonce = nonce;
            Ok(json!(true))
        }

        // Replacement on Windows is submission: a toast carrying the group and
        // tag of one already delivered supersedes it in place rather than
        // stacking beside it. The new token is what makes the superseded toast's
        // remaining callbacks stale.
        #[cfg(target_os = "windows")]
        {
            let options = record.options.clone();
            let token = self.token();
            let (toast, nonce) =
                windows_backend::toast(public_id, &options, &self.notification_session, token)?;
            windows_backend::notifier(public_id, token, &self.signals, &self.proxy)
                .show(&toast)
                .map_err(|error| format!("could not update notification {public_id}: {error}"))?;
            let record = self
                .records
                .get_mut(public_id)
                .expect("record still exists");
            record.token = token;
            record.nonce = nonce;
            Ok(json!(true))
        }
    }

    pub(crate) fn close(&mut self, public_id: &str) -> Result<Value, String> {
        let Some(record) = self.records.remove(public_id) else {
            return Ok(json!(false));
        };

        #[cfg(target_os = "linux")]
        match record.handle {
            LinuxHandle::Portal => self
                .portal
                .as_ref()
                .map_err(Clone::clone)?
                .as_ref()
                .expect("a portal record has a portal")
                .close(public_id)?,
            LinuxHandle::Freedesktop(handle) => handle.close(),
        }
        #[cfg(target_os = "macos")]
        mac_usernotifications::blocking::close_delivered(&record.native_id);
        // The toast is addressed by the group and tag `show` gave it, so nothing
        // the record holds is needed to withdraw it — only the fact that this
        // session still owned the ID. Removal covers both a toast still on
        // screen and one the user has left sitting in notification history.
        #[cfg(target_os = "windows")]
        {
            let _ = record;
            winrt_toast_reborn::ToastManager::new(windows_backend::app_id())
                .remove_grouped_tag(windows_backend::GROUP, public_id)
                .map_err(|error| format!("could not close notification {public_id}: {error}"))?;
        }

        crate::dom_bridge::notify::closed(public_id.to_owned(), "closed");
        Ok(json!(true))
    }

    pub(crate) fn poll(&mut self) {
        #[cfg(target_os = "linux")]
        if let Ok(Some(portal)) = &self.portal {
            let (activations, errors) = portal.take();
            for (id, error) in errors {
                crate::dom_bridge::notify::failed(id, error);
            }
            match activations {
                Ok(activations) => {
                    for activation in activations {
                        let active_record = self.records.contains_key(&activation.id);
                        if portal.is_current(&activation, active_record) {
                            self.records.remove(&activation.id);
                            if let Some(action) = activation.action {
                                crate::dom_bridge::notify::action(activation.id, action);
                            } else {
                                crate::dom_bridge::notify::clicked(activation.id);
                            }
                        } else {
                            crate::dom_bridge::notify::activated(activation);
                        }
                    }
                }
                Err(error) => crate::dom_bridge::notify::failed(String::new(), error),
            }
        }
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        {
            for (id, error) in self.activation_errors.lock().drain(..) {
                crate::dom_bridge::notify::failed(id, error);
            }
            let activations = self
                .activation_store
                .as_ref()
                .map(super::ActivationStore::take)
                .transpose();
            match activations {
                Ok(Some(activations)) => {
                    for activation in activations {
                        let current = super::addresses_generation(
                            &activation,
                            &self.notification_session,
                            self.records
                                .get(&activation.id)
                                .and_then(|record| record.nonce.as_deref()),
                        );
                        if current {
                            self.records.remove(&activation.id);
                            if activation.dismissed.is_some() {
                                crate::dom_bridge::notify::closed(activation.id, "dismissed");
                            } else if let Some(action) = activation.action {
                                crate::dom_bridge::notify::action(activation.id, action);
                            } else {
                                crate::dom_bridge::notify::clicked(activation.id);
                            }
                        } else {
                            crate::dom_bridge::notify::activated(activation);
                        }
                    }
                }
                Ok(None) => {}
                Err(error) => crate::dom_bridge::notify::failed(String::new(), error),
            }
        }
        let signals = self.signals.lock().drain(..).collect::<Vec<_>>();
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

    #[cfg(target_os = "linux")]
    pub(crate) fn take_present_request(&self) -> bool {
        self.portal
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .is_some_and(super::linux_portal::LinuxPortal::take_present_request)
    }

    pub(crate) fn clear(&mut self) {
        let ids = self.records.keys().cloned().collect::<Vec<_>>();
        for id in ids {
            let _ = self.close(&id);
        }
        self.records.clear();
        self.signals.lock().clear();
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        self.activation_errors.lock().clear();
    }

    /// Drops this process's callback state without withdrawing notifications.
    ///
    /// A graceful application exit is the ordinary way a notification gains a
    /// stopped application to activate. Reload uses [`Self::clear`] instead:
    /// that replaces a live session whose notifications must not keep speaking.
    pub(crate) fn detach(&mut self) {
        self.records.clear();
        self.signals.lock().clear();
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        self.activation_errors.lock().clear();
    }
}

#[cfg(test)]
mod tests;
