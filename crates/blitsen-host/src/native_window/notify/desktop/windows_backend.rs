//! Windows specifics of the desktop notification backend: the AppUserModelID a
//! toast is filed under, the toast built from the shared options, and the
//! notifier whose callbacks become queued signals.

use std::collections::VecDeque;
use std::sync::Arc;

use notify_rust::{CloseReason, NotificationResponse, Urgency};
use parking_lot::Mutex;
use serde_json::{Value, json};
use winit::event_loop::EventLoopProxy;

use super::super::{
    decode_desktop_envelope, encode_desktop_envelope, entry_point, windows_activation,
};
use super::{NO_TOAST_IDENTITY, Signal, SignalKind, activation, queue, urgency};
use crate::dom_bridge::notify::NotificationOptions;

pub(super) struct Record {
    pub(super) options: NotificationOptions,
    pub(super) token: u64,
    pub(super) nonce: Option<String>,
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
const BORROWED_APP_ID: &str = winrt_toast_reborn::ToastManager::POWERSHELL_AUM_ID;

/// The application identity Windows files every Blitsen toast under.
///
/// Permission, replacement and removal are all scoped to it, so all three have
/// to agree on which identity they mean — which is why this is read rather than
/// passed: `permission` is an associated function with no session to reach
/// through, and the notifier is built by a free function.
pub(super) fn app_id() -> &'static str {
    entry_point().map_or(BORROWED_APP_ID, |entry_point| entry_point.entry.as_str())
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
pub(crate) fn register_entry_point(display_name: &str) {
    let Some(entry_point) = entry_point() else {
        return;
    };
    // A registration that fails is not a reason to refuse to start: the identity
    // may already be registered by an installer that did it properly, and if it
    // is not, `permission` reports the missing identity in the sentence #251
    // wrote for exactly this.
    let _ = winrt_toast_reborn::register(&entry_point.entry, display_name, None);
    let _ = windows_activation::register(&entry_point.entry, display_name);
}

/// The toast group Blitsen's own notifications share.
///
/// The group narrows the tag: removing `(group, tag)` cannot reach a toast some
/// other application filed under the shared PowerShell identity with a colliding
/// tag of its own.
pub(super) const GROUP: &str = "blitsen";

/// `ERROR_ELEMENT_NOT_FOUND`, which is how Windows spells an identity it has
/// never seen when it is asked for that identity's notifier setting.
///
/// Written as the Win32 number the `windows` crate's `Win32_Foundation` feature
/// would name, because compiling that whole feature for one constant costs more
/// than the constant does. The message cannot be matched on instead: Windows
/// localises it, and the failing CI line only read "Element not found." because
/// the runner happened to be English.
const ELEMENT_NOT_FOUND: windows::core::HRESULT = windows::core::HRESULT::from_win32(1168);

pub(super) fn permission(request: bool) -> Result<Value, String> {
    use windows::UI::Notifications::{NotificationSetting, ToastNotificationManager};
    use windows::core::HSTRING;

    // Windows has no programmatic prompt: the user, the administrator or
    // group policy decides, and an application can only read the answer.
    // Requesting is therefore the same non-mutating reading, and there is
    // no third state to report — the notifier is either enabled for this
    // identity or it is switched off, whoever switched it off.
    let _ = request;
    let setting = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(app_id()))
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

/// The Windows toast for `options`, tagged with the session ID that addresses it.
///
/// The tag is what makes `update` and `close` possible at all: Windows replaces
/// a delivered toast whose group and tag match a newly shown one, and removes
/// the same pair from notification history. Both are derived from the public ID
/// alone, so a toast built here for an ID `show` already used is the same toast
/// as far as the platform is concerned.
pub(super) fn toast(
    public_id: &str,
    options: &NotificationOptions,
    session: &str,
    generation: u64,
) -> Result<(winrt_toast_reborn::Toast, Option<String>), String> {
    use winrt_toast_reborn::content::image::ImagePlacement;
    use winrt_toast_reborn::{Action, Image, Scenario, Toast, ToastDuration};

    let body_activation = activation(public_id, None, session, generation, "windows");
    let nonce = body_activation
        .as_ref()
        .map(|activation| activation.nonce.clone());
    let launch = body_activation
        .as_ref()
        .map_or_else(|| "default".to_owned(), encode_desktop_envelope);
    let mut toast = Toast::new();
    toast
        .text1(&options.title)
        .text2(&options.body)
        .tag(public_id)
        .group(GROUP)
        // Windows gives a body click no argument of its own, so the toast's
        // launch string is what distinguishes it from a button in the activation
        // handler. `"default"` is the identifier the declarations reserve for it.
        .launch(&launch)
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
        let argument = activation(public_id, Some(&action.id), session, generation, "windows")
            .as_ref()
            .map_or_else(|| action.id.clone(), encode_desktop_envelope);
        toast.action(Action::new(&action.title, &argument, ""));
    }
    Ok((toast, nonce))
}

/// A Windows toast notifier whose callbacks report as `public_id` at `token`.
///
/// The handlers belong to the notifier rather than to the toast, so a
/// replacement gets a fresh notifier carrying the replacement's token. The
/// toast Windows superseded may still report its own dismissal afterwards;
/// [`super::NotifyController::poll`] drops it because the record no longer
/// holds that token, which is how a replaced toast stops speaking for the ID
/// it had.
pub(super) fn notifier(
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
            let response = decode_desktop_envelope(&activated.arg).map_or_else(
                |_| {
                    if activated.arg == "default" {
                        NotificationResponse::Default
                    } else {
                        NotificationResponse::Action(activated.arg)
                    }
                },
                |activation| {
                    activation
                        .action
                        .map_or(NotificationResponse::Default, NotificationResponse::Action)
                },
            );
            queue(
                &activated_signals,
                &activated_proxy,
                activated_id.clone(),
                token,
                SignalKind::Response(response),
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
