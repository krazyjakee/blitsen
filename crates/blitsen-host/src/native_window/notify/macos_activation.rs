//! Cold `UNUserNotificationCenter` response capture (#252).
//!
//! The submission library's delegate intentionally knows only requests the
//! current process submitted. A notification that launches a stopped `.app`
//! belongs to the previous process, so its response has no waiting sender and
//! is otherwise discarded. Blitsen encodes the durable activation envelope in
//! the request identifier and installs this delegate after the library worker
//! has installed its own. Every response is persisted before the framework's
//! completion handler is called; the ordinary frame poll then decides whether
//! it belongs to a live record or is a cold activation.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use block2::DynBlock;
use objc2::{AnyThread, define_class, rc::Retained};
use objc2_foundation::{NSObject, NSObjectProtocol};
use objc2_user_notifications::{
    UNNotificationDefaultActionIdentifier, UNNotificationDismissActionIdentifier,
    UNNotificationResponse, UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};
use parking_lot::Mutex;
use winit::event_loop::EventLoopProxy;

use super::{ActivationStore, decode_desktop_envelope};

#[derive(Clone)]
struct Capture {
    directory: PathBuf,
    identity: String,
    entry: String,
    errors: Arc<Mutex<VecDeque<(String, String)>>>,
    proxy: EventLoopProxy,
}

static CAPTURE: OnceLock<Mutex<Option<Capture>>> = OnceLock::new();

fn capture() -> &'static Mutex<Option<Capture>> {
    CAPTURE.get_or_init(|| Mutex::new(None))
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "BlitsenNotificationActivationDelegate"]
    struct ActivationDelegate;

    unsafe impl NSObjectProtocol for ActivationDelegate {}

    unsafe impl UNUserNotificationCenterDelegate for ActivationDelegate {
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present_notification(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &objc2_user_notifications::UNNotification,
            completion: &DynBlock<
                dyn Fn(objc2_user_notifications::UNNotificationPresentationOptions),
            >,
        ) {
            completion.call((
                objc2_user_notifications::UNNotificationPresentationOptions::Banner
                    | objc2_user_notifications::UNNotificationPresentationOptions::Sound,
            ));
        }

        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive_response(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            completion: &DynBlock<dyn Fn()>,
        ) {
            record(response);
            completion.call(());
        }
    }
);

impl ActivationDelegate {
    fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(());
        // SAFETY: this is the designated NSObject initialiser for the subclass
        // produced by `define_class!`.
        unsafe { objc2::msg_send![super(this), init] }
    }
}

fn record(response: &UNNotificationResponse) {
    let identifier = response.notification().request().identifier().to_string();
    let Ok(mut activation) = decode_desktop_envelope(&identifier) else {
        return;
    };
    let Some(capture) = capture().lock().clone() else {
        return;
    };
    if activation.identity != capture.identity || activation.entry != capture.entry {
        return;
    }

    let action = response.actionIdentifier().to_string();
    // SAFETY: both symbols are non-null framework constants with process
    // lifetime, as required by objc2's extern-static access contract.
    let default_action = unsafe { UNNotificationDefaultActionIdentifier.to_string() };
    let dismiss_action = unsafe { UNNotificationDismissActionIdentifier.to_string() };
    if action == dismiss_action {
        activation.dismissed = Some("dismissed".to_owned());
    } else if action != default_action {
        activation.action = Some(action);
    }

    let id = activation.id.clone();
    if let Err(error) =
        ActivationStore::new(&capture.directory, &capture.identity).record(activation)
    {
        capture.errors.lock().push_back((id, error));
    }
    capture.proxy.wake_up();
}

/// Installs the capturing delegate for this application session.
pub(super) fn install(
    directory: PathBuf,
    identity: String,
    entry: String,
    errors: Arc<Mutex<VecDeque<(String, String)>>>,
    proxy: EventLoopProxy,
) {
    // The first settings query starts the dependency's worker and lets it
    // install its private delegate. Installing ours afterwards makes ownership
    // deterministic; subsequent sends reuse that worker and do not replace it.
    let _ = mac_usernotifications::blocking::get_notification_settings();
    *capture().lock() = Some(Capture {
        directory,
        identity,
        entry,
        errors,
        proxy,
    });
    static DELEGATE: OnceLock<Retained<ActivationDelegate>> = OnceLock::new();
    let delegate = DELEGATE.get_or_init(ActivationDelegate::new);
    UNUserNotificationCenter::currentNotificationCenter()
        .setDelegate(Some(objc2::runtime::ProtocolObject::from_ref(&**delegate)));
}
