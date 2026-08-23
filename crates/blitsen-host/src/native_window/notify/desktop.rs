//! Desktop notification lifecycle built on `notify-rust`.
//!
//! The backend's callbacks run away from the JavaScript frame turn. They write
//! to `signals` and wake winit; [`NotifyController::poll`] is the only place
//! those callbacks become public events.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use notify_rust::{CloseReason, NotificationResponse, Urgency};
#[cfg(not(target_os = "windows"))]
use notify_rust::{Notification, Timeout};
use serde_json::{Value, json};
use winit::event_loop::EventLoopProxy;

use crate::dom_bridge::notify::{NotificationOptions, NotificationPatch};

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
    "Give the development host one of its own with `blitsen run --dev-bundle`, which builds and ",
    "re-executes into a signed development .app, or run an application exported by ",
    "`blitsen build --bundle-id <id> --sign <command>`.",
);

/// The library's own bundle check, ahead of anything that reaches the framework.
///
/// Only `permission` and `show` need it: every other entry point addresses a
/// notification that a `show` already got through, and a process cannot acquire
/// or lose a bundle identifier while it runs.
#[cfg(target_os = "macos")]
fn bundle_identity() -> Result<(), String> {
    notify_rust::check_bundle().map_err(|_| NO_BUNDLE_IDENTITY.to_owned())
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
    options: NotificationOptions,
    token: u64,
}

/// The application identity Windows files every Blitsen toast under.
///
/// Windows will not display a toast from an identity it does not know, and
/// registering one is the packaging work #252 tracks. PowerShell's is present on
/// every installation, which is why it is the identity the notification
/// libraries offer as their unpackaged default and the one `notify-rust` already
/// used here. Permission, replacement and removal are all scoped to it, so the
/// three have to agree on which identity they mean.
#[cfg(target_os = "windows")]
const APP_ID: &str = winrt_toast_reborn::ToastManager::POWERSHELL_AUM_ID;

/// The toast group Blitsen's own notifications share.
///
/// The group narrows the tag: removing `(group, tag)` cannot reach a toast some
/// other application filed under the shared PowerShell identity with a colliding
/// tag of its own.
#[cfg(target_os = "windows")]
const GROUP: &str = "blitsen";

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
    for action in &options.actions {
        notification.action(&action.id, &action.title);
    }
    Ok(notification)
}

/// The Windows toast for `options`, tagged with the session ID that addresses it.
///
/// The tag is what makes `update` and `close` possible at all: Windows replaces
/// a delivered toast whose group and tag match a newly shown one, and removes
/// the same pair from notification history. Both are derived from the public ID
/// alone, so a toast built here for an ID `show` already used is the same toast
/// as far as the platform is concerned.
#[cfg(target_os = "windows")]
fn toast(
    public_id: &str,
    options: &NotificationOptions,
) -> Result<winrt_toast_reborn::Toast, String> {
    use winrt_toast_reborn::content::image::ImagePlacement;
    use winrt_toast_reborn::{Action, Image, Scenario, Toast, ToastDuration};

    let mut toast = Toast::new();
    toast
        .text1(&options.title)
        .text2(&options.body)
        .tag(public_id)
        .group(GROUP)
        // Windows gives a body click no argument of its own, so the toast's
        // launch string is what distinguishes it from a button in the activation
        // handler. `"default"` is the identifier the declarations reserve for it.
        .launch("default")
        // Windows has two toast durations rather than a timeout, and picks the
        // exact seconds itself. This is the mapping `notify-rust` applied.
        .duration(match options.timeout {
            Some(0) => ToastDuration::Long,
            Some(timeout) if timeout >= 25_000 => ToastDuration::Long,
            _ => ToastDuration::Short,
        });
    // A critical notification is one the user must not miss, and the reminder
    // scenario is Windows' name for a toast that stays until it is dismissed.
    // Low and normal are the ordinary toast, which has no scenario of its own.
    if matches!(urgency(&options.urgency)?, Urgency::Critical) {
        toast.scenario(Scenario::Reminder);
    }
    if let Some(icon) = &options.icon {
        // Windows resolves a toast image through a URI, so a relative path has
        // no meaning to the notification platform and is rejected rather than
        // silently resolved against whatever directory this process is in.
        let image = Image::new_local(icon)
            .map_err(|error| format!("could not use notification icon {icon:?}: {error}"))?;
        toast.image(1, image.with_placement(ImagePlacement::AppLogoOverride));
    }
    for action in &options.actions {
        toast.action(Action::new(&action.title, &action.id, ""));
    }
    Ok(toast)
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

/// A Windows toast notifier whose callbacks report as `public_id` at `token`.
///
/// The handlers belong to the notifier rather than to the toast, so a
/// replacement gets a fresh notifier carrying the replacement's token. The
/// toast Windows superseded may still report its own dismissal afterwards;
/// [`NotifyController::poll`] drops it because the record no longer holds that
/// token, which is how a replaced toast stops speaking for the ID it had.
#[cfg(target_os = "windows")]
fn notifier(
    public_id: &str,
    token: u64,
    signals: &Arc<Mutex<VecDeque<Signal>>>,
    proxy: &EventLoopProxy,
) -> winrt_toast_reborn::ToastManager {
    use winrt_toast_reborn::{DismissalReason, ToastManager};

    let (activated_signals, activated_proxy, activated_id) =
        (Arc::clone(signals), proxy.clone(), public_id.to_owned());
    let (dismissed_signals, dismissed_proxy, dismissed_id) =
        (Arc::clone(signals), proxy.clone(), public_id.to_owned());
    let (failed_signals, failed_proxy, failed_id) =
        (Arc::clone(signals), proxy.clone(), public_id.to_owned());

    ToastManager::new(APP_ID)
        .on_activated(None, move |activated| {
            // Windows can report an activation it attributes to neither the body
            // nor a button. Calling that a click would end the notification for
            // the user on a guess, so it is dropped instead.
            let Some(activated) = activated else { return };
            queue(
                &activated_signals,
                &activated_proxy,
                activated_id.clone(),
                token,
                SignalKind::Response(if activated.arg == "default" {
                    NotificationResponse::Default
                } else {
                    NotificationResponse::Action(activated.arg)
                }),
            );
        })
        .on_dismissed(move |dismissed| {
            queue(
                &dismissed_signals,
                &dismissed_proxy,
                dismissed_id.clone(),
                token,
                match dismissed {
                    Ok(dismissed) => {
                        SignalKind::Response(NotificationResponse::Closed(match dismissed.reason {
                            DismissalReason::UserCanceled => CloseReason::Dismissed,
                            DismissalReason::TimedOut => CloseReason::Expired,
                            DismissalReason::ApplicationHidden => CloseReason::CloseAction,
                        }))
                    }
                    Err(error) => SignalKind::Error(format!(
                        "could not observe notification response: {error}"
                    )),
                },
            );
        })
        .on_failed(move |failed| {
            queue(
                &failed_signals,
                &failed_proxy,
                failed_id.clone(),
                token,
                SignalKind::Error(format!("could not show notification: {}", failed.error)),
            );
        })
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
            use windows::UI::Notifications::{NotificationSetting, ToastNotificationManager};
            use windows::core::HSTRING;

            // Windows has no programmatic prompt: the user, the administrator or
            // group policy decides, and an application can only read the answer.
            // Requesting is therefore the same non-mutating reading, and there is
            // no third state to report — the notifier is either enabled for this
            // identity or it is switched off, whoever switched it off.
            let _ = request;
            let setting =
                ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID))
                    .and_then(|notifier| notifier.Setting())
                    .map_err(|error| format!("could not read notification permission: {error}"))?;
            Ok(json!(if setting == NotificationSetting::Enabled {
                "granted"
            } else {
                "denied"
            }))
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
        bundle_identity()?;
        #[cfg(not(target_os = "windows"))]
        let handle = {
            #[allow(unused_mut)]
            let mut spec = notification(&options)?;
            #[cfg(target_os = "macos")]
            spec.id(public_id.clone());
            spec.show()
                .map_err(|error| format!("could not show notification: {error}"))?
        };
        // Windows registers the response handlers on the notifier before the
        // toast reaches the platform, so the token they report at has to exist
        // first. Building the toast before taking one keeps a rejected option —
        // an unusable icon, an unknown urgency — from consuming a token.
        #[cfg(target_os = "windows")]
        let toast = toast(&public_id, &options)?;
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
            notifier(&public_id, token, &self.signals, &self.proxy)
                .show(&toast)
                .map_err(|error| format!("could not show notification: {error}"))?;
            self.records
                .insert(public_id.clone(), Record { options, token });
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

        // Replacement on Windows is submission: a toast carrying the group and
        // tag of one already delivered supersedes it in place rather than
        // stacking beside it. The new token is what makes the superseded toast's
        // remaining callbacks stale.
        #[cfg(target_os = "windows")]
        {
            let toast = toast(public_id, &record.options)?;
            let token = self.token();
            self.records
                .get_mut(public_id)
                .expect("record still exists")
                .token = token;
            notifier(public_id, token, &self.signals, &self.proxy)
                .show(&toast)
                .map_err(|error| format!("could not update notification {public_id}: {error}"))?;
            Ok(json!(true))
        }
    }

    pub(crate) fn close(&mut self, public_id: &str) -> Result<Value, String> {
        let Some(record) = self.records.remove(public_id) else {
            return Ok(json!(false));
        };

        #[cfg(target_os = "linux")]
        record.handle.close();
        #[cfg(target_os = "macos")]
        mac_usernotifications::blocking::close_delivered(public_id);
        // The toast is addressed by the group and tag `show` gave it, so nothing
        // the record holds is needed to withdraw it — only the fact that this
        // session still owned the ID. Removal covers both a toast still on
        // screen and one the user has left sitting in notification history.
        #[cfg(target_os = "windows")]
        {
            let _ = record;
            winrt_toast_reborn::ToastManager::new(APP_ID)
                .remove_grouped_tag(GROUP, public_id)
                .map_err(|error| format!("could not close notification {public_id}: {error}"))?;
        }

        crate::dom_bridge::notify::closed(public_id.to_owned(), "closed");
        Ok(json!(true))
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

    #[test]
    fn the_unbundled_macos_refusal_names_a_command_and_borrows_no_identity() {
        // Both halves of #253's acceptance: the limitation is actionable, and
        // the action is one the reader can type.
        assert!(NO_BUNDLE_IDENTITY.contains("blitsen run --dev-bundle"));
        assert!(NO_BUNDLE_IDENTITY.contains("blitsen build --bundle-id <id> --sign <command>"));
        // And the shortcut it refuses stays refused: submitting under an
        // installed application's identifier is what the legacy backend's
        // `get_bundle_identifier_or_default` does, and no message that named one
        // could be read as anything but an invitation to do it.
        for borrowed in ["com.apple.", "Terminal", "Script Editor", "iTerm"] {
            assert!(
                !NO_BUNDLE_IDENTITY.contains(borrowed),
                "the macOS notification refusal must not name {borrowed}"
            );
        }
    }
}

/// What only a Windows host can answer.
///
/// Replacement by tag and removal from notification history are behaviours of
/// the Windows notification platform rather than of anything this file could
/// stand in for, so these talk to the real platform under the same identity,
/// group and tags [`NotifyController`] uses. They need no event loop, which is
/// what lets them run under the ordinary `cargo test` the Windows job runs.
#[cfg(all(test, target_os = "windows"))]
mod windows_tests {
    use super::*;
    use windows::UI::Notifications::ToastNotificationManager;
    use windows::core::HSTRING;
    use winrt_toast_reborn::ToastManager;

    /// The tags Blitsen's group currently holds in notification history.
    fn tags() -> Vec<String> {
        ToastNotificationManager::History()
            .and_then(|history| history.GetHistoryWithId(&HSTRING::from(APP_ID)))
            .expect("notification history is readable")
            .into_iter()
            .filter(|toast| toast.Group().is_ok_and(|group| group == GROUP))
            .map(|toast| {
                toast
                    .Tag()
                    .expect("a delivered toast keeps the tag it was shown with")
                    .to_string()
            })
            .collect()
    }

    /// Asserts that notification history settles on `expected`.
    ///
    /// `Show` hands a toast to the notification platform rather than to the
    /// Action Center, and a removal is acknowledged the same way, so reading
    /// back at once races the platform instead of testing it. The wait is
    /// bounded and the assertion it ends in is the whole one.
    fn settles_on(expected: &[&str]) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut observed = tags();
        while observed != expected && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
            observed = tags();
        }
        assert_eq!(observed, expected);
    }

    fn options(body: &str) -> NotificationOptions {
        NotificationOptions {
            title: "Export complete".into(),
            body: body.into(),
            app_name: None,
            timeout: Some(1000),
            urgency: "normal".into(),
            icon: None,
            actions: vec![crate::dom_bridge::notify::NotificationAction {
                id: "open".into(),
                title: "Open archive".into(),
            }],
        }
    }

    #[test]
    fn permission_reads_the_native_notifier_setting() {
        let read = NotifyController::permission(false).expect("the notifier setting is readable");
        assert!(
            read == json!("granted") || read == json!("denied"),
            "Windows has no undetermined notification state, but reported {read}"
        );
        assert_eq!(
            NotifyController::permission(true).expect("requesting reads the same setting"),
            read,
            "requesting must not prompt or change what the notifier reports"
        );
    }

    #[test]
    fn a_shown_toast_is_replaced_and_removed_through_its_session_id() {
        // A notifier the user or policy has switched off is Windows declining
        // to deliver, so there is nothing for history to hold and delivery is
        // not the platform's promise to keep. Removal is asserted either way: an
        // ID Blitsen closed must leave nothing behind whether or not the toast
        // was ever displayed.
        let delivers = NotifyController::permission(false)
            .expect("the notifier setting is readable")
            == json!("granted");
        let public_id = "n-windows-lifecycle";
        let manager = ToastManager::new(APP_ID);
        manager
            .remove_grouped_tag(GROUP, public_id)
            .expect("notification history is writable");

        manager
            .show(&toast(public_id, &options("The archive is ready.")).expect("the toast builds"))
            .expect("the toast is accepted");
        if delivers {
            settles_on(&[public_id]);
        }

        manager
            .show(&toast(public_id, &options("Copied to Downloads.")).expect("the toast builds"))
            .expect("the replacement is accepted");
        if delivers {
            settles_on(&[public_id]);
        }

        manager
            .remove_grouped_tag(GROUP, public_id)
            .expect("the toast is removable");
        settles_on(&[]);
    }

    #[test]
    fn an_unusable_option_is_rejected_before_a_toast_reaches_windows() {
        // Windows resolves a toast image through a URI, so a relative path is
        // not a path it can be asked about later.
        let mut relative_icon = options("The archive is ready.");
        relative_icon.icon = Some("archive.png".into());
        assert!(toast("n1", &relative_icon).is_err());

        let mut unknown_urgency = options("The archive is ready.");
        unknown_urgency.urgency = "urgent".into();
        assert!(toast("n1", &unknown_urgency).is_err());
    }
}
