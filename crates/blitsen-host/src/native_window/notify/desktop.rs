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
    handle: LinuxHandle,
    token: u64,
}

#[cfg(target_os = "linux")]
enum LinuxHandle {
    /// Development runs have no installed identity and retain the original
    /// freedesktop notification backend and its live-process response stream.
    Freedesktop(Box<notify_rust::NotificationHandle>),
    /// Packaged identities submit through the portal, which can D-Bus-activate
    /// the application after this process and its connection have exited.
    Portal,
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

/// The identity Windows files every Blitsen toast under when nothing registered
/// one for this executable.
///
/// Windows will not display a toast from an identity it does not know.
/// PowerShell's is present on every installation, which is why it is the
/// identity the notification libraries offer as their unpackaged default and the
/// one `notify-rust` already used here. It stays as the development answer: an
/// interpreter running a script is not an installed application and has nothing
/// of its own to be known by.
#[cfg(target_os = "windows")]
const BORROWED_APP_ID: &str = winrt_toast_reborn::ToastManager::POWERSHELL_AUM_ID;

/// The application identity Windows files every Blitsen toast under.
///
/// Permission, replacement and removal are all scoped to it, so all three have
/// to agree on which identity they mean — which is why this is read rather than
/// passed: `permission` is an associated function with no session to reach
/// through, and the notifier is built by a free function.
#[cfg(target_os = "windows")]
fn app_id() -> &'static str {
    super::entry_point().map_or(BORROWED_APP_ID, |entry_point| entry_point.entry.as_str())
}

/// Tells the notification platform that this AppUserModelID exists (#252).
///
/// An AppUserModelID Windows has never seen holds no notifier, which is the
/// state #251's refusal describes: `permission` cannot be read, and a toast has
/// no identity to be delivered under. Registering one is a key under the running
/// user's own `SOFTWARE\Classes\AppUserModelId`, and `winrt-toast-reborn` — the
/// crate already delivering the toasts — writes it, so no registry code is
/// forked here to do it.
///
/// It happens at startup rather than at packaging time because `blitsen build`
/// cross-compiles: the machine that writes a Windows artifact is routinely a
/// Linux one, and the hive that has to carry this key belongs to the user who
/// eventually runs it.
#[cfg(target_os = "windows")]
pub(super) fn register_entry_point(display_name: &str) {
    let Some(entry_point) = super::entry_point() else {
        return;
    };
    // A registration that fails is not a reason to refuse to start: the identity
    // may already be registered by an installer that did it properly, and if it
    // is not, `permission` reports the missing identity in the sentence #251
    // wrote for exactly this.
    let _ = winrt_toast_reborn::register(&entry_point.entry, display_name, None);
}

/// The toast group Blitsen's own notifications share.
///
/// The group narrows the tag: removing `(group, tag)` cannot reach a toast some
/// other application filed under the shared PowerShell identity with a colliding
/// tag of its own.
#[cfg(target_os = "windows")]
const GROUP: &str = "blitsen";

/// `ERROR_ELEMENT_NOT_FOUND`, which is how Windows spells an identity it has
/// never seen when it is asked for that identity's notifier setting.
///
/// Written as the Win32 number the `windows` crate's `Win32_Foundation` feature
/// would name, because compiling that whole feature for one constant costs more
/// than the constant does. The message cannot be matched on instead: Windows
/// localises it, and the failing CI line only read "Element not found." because
/// the runner happened to be English.
#[cfg(target_os = "windows")]
const ELEMENT_NOT_FOUND: windows::core::HRESULT = windows::core::HRESULT::from_win32(1168);

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
    // Which installed application this notification belongs to, in the one term
    // the freedesktop notification specification has for it (#252). A server
    // resolves the name against the installed desktop entries to attribute the
    // notification, to file it under the right application in a notification
    // centre, and — where the server implements it at all — to find the entry
    // point to start when the application is no longer running. `appname` above
    // is a display string and answers none of those: it is what the sender calls
    // itself, not what the system has installed.
    #[cfg(target_os = "linux")]
    if let Some(entry_point) = super::entry_point() {
        notification.hint(notify_rust::Hint::DesktopEntry(entry_point.entry.clone()));
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
    signals.lock().push_back(Signal {
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

    ToastManager::new(app_id())
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
        #[cfg(target_os = "linux")]
        let portal = super::linux_portal::LinuxPortal::new(proxy.clone());
        Self {
            proxy,
            signals: Arc::new(Mutex::new(VecDeque::new())),
            records: HashMap::new(),
            next_token: 1,
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
                ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(app_id()))
                    .and_then(|notifier| notifier.Setting())
                    .map_err(|error| {
                        // An identity the platform never registered is not a
                        // notifier reporting a state, so it is refused with the
                        // prerequisite rather than reported as one more failure.
                        if error.code() == ELEMENT_NOT_FOUND {
                            NO_TOAST_IDENTITY.to_owned()
                        } else {
                            format!("could not read notification permission: {error}")
                        }
                    })?;
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
        #[cfg(all(not(target_os = "windows"), not(target_os = "linux")))]
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
            let handle = match &self.portal {
                Ok(Some(portal)) => {
                    portal.show(&public_id, &options)?;
                    LinuxHandle::Portal
                }
                Ok(None) => {
                    let handle = notification(&options)?
                        .show()
                        .map_err(|error| format!("could not show notification: {error}"))?;
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
        mac_usernotifications::blocking::close_delivered(public_id);
        // The toast is addressed by the group and tag `show` gave it, so nothing
        // the record holds is needed to withdraw it — only the fact that this
        // session still owned the ID. Removal covers both a toast still on
        // screen and one the user has left sitting in notification history.
        #[cfg(target_os = "windows")]
        {
            let _ = record;
            winrt_toast_reborn::ToastManager::new(app_id())
                .remove_grouped_tag(GROUP, public_id)
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
    }

    /// Drops this process's callback state without withdrawing notifications.
    ///
    /// A graceful application exit is the ordinary way a notification gains a
    /// stopped application to activate. Reload uses [`Self::clear`] instead:
    /// that replaces a live session whose notifications must not keep speaking.
    pub(crate) fn detach(&mut self) {
        self.records.clear();
        self.signals.lock().clear();
    }
}

#[cfg(test)]
mod tests;
