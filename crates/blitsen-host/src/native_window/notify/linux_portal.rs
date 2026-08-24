//! Launch-capable Linux notifications through the desktop portal (#252).
//!
//! The freedesktop notification interface reports actions only to the D-Bus
//! connection that submitted a notification. Once that connection exits there
//! is nobody to launch. The notification portal has the complementary contract:
//! an `app.` action is delivered through `org.freedesktop.Application`, and the
//! session bus starts the named application service when it is not running.
//!
//! This module owns that complete seam. The build writes a desktop entry and
//! session-service file named after the same application ID for an installer;
//! owns that well-known name and implements `ActivateAction`; each portal action
//! carries the ordinary serialized activation envelope as its target. The D-Bus
//! callback records it before returning and the frame thread drains the existing
//! replay store, so a callback racing document startup or Activity-like surface
//! recreation is still delivered once.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use winit::event_loop::EventLoopProxy;
use zbus::blocking::{Connection, Proxy, connection};
use zbus::zvariant::{OwnedValue, Value};

use super::ActivationStore;
use crate::dom_bridge::notify::{Activation, NotificationOptions};

const PORTAL_DESTINATION: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const PORTAL_INTERFACE: &str = "org.freedesktop.portal.Notification";
const HOST_REGISTRY_INTERFACE: &str = "org.freedesktop.host.portal.Registry";
const ACTION: &str = "app.blitsen-notification";
const ACTION_NAME: &str = "blitsen-notification";

static NEXT_NONCE: AtomicU64 = AtomicU64::new(1);
fn object_path(identity: &str) -> String {
    format!("/{}", identity.replace('.', "/").replace('-', "_"))
}

fn envelope(
    public_id: &str,
    action: Option<&str>,
    notification_session: &str,
) -> Result<Activation, String> {
    let entry = super::entry_point().ok_or_else(|| {
        "a launch-capable Linux notification needs a packaged application identity".to_owned()
    })?;
    let minted = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    let sequence = NEXT_NONCE.fetch_add(1, Ordering::Relaxed);
    Ok(Activation {
        nonce: format!("{minted:x}-{sequence:x}"),
        identity: entry.identity.clone(),
        id: public_id.to_owned(),
        session: Some(notification_session.to_owned()),
        action: action.map(str::to_owned),
        dismissed: None,
        platform: "linux".to_owned(),
        entry: entry.entry.clone(),
    })
}

fn owned(value: impl Into<Value<'static>>) -> OwnedValue {
    OwnedValue::try_from(value.into()).expect("a portal notification value becomes owned")
}

fn serialized_icon(icon: &str) -> Result<OwnedValue, String> {
    if Path::new(icon).is_absolute() {
        return Err(format!(
            "notification icon {icon:?} is an absolute path, but the launch-capable Linux \
             notification portal accepts image files only as sealed file descriptors; use an \
             installed icon-theme name for this packaged application"
        ));
    }
    Ok(owned(("themed".to_owned(), owned(vec![icon.to_owned()]))))
}

#[derive(Default)]
struct PortalRegistration {
    owner: Option<String>,
}

impl PortalRegistration {
    /// Registers once with each incarnation of xdg-desktop-portal.
    ///
    /// The registry permits one call per connection, but its memory disappears
    /// when the portal process restarts. The unique-name owner distinguishes
    /// those two cases. The new owner is remembered only after `Register`
    /// succeeds, so a transient failure is retried before the next portal call.
    fn ensure(
        &mut self,
        owner: &str,
        register: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        if self.owner.as_deref() == Some(owner) {
            return Ok(());
        }
        register()?;
        self.owner = Some(owner.to_owned());
        Ok(())
    }
}

fn payload(
    public_id: &str,
    options: &NotificationOptions,
    notification_session: &str,
) -> Result<HashMap<String, OwnedValue>, String> {
    let mut notification = HashMap::new();
    notification.insert("title".to_owned(), owned(options.title.clone()));
    if !options.body.is_empty() {
        notification.insert("body".to_owned(), owned(options.body.clone()));
    }
    notification.insert(
        "priority".to_owned(),
        owned(match options.urgency.as_str() {
            "low" => "low",
            "normal" => "normal",
            "critical" => "urgent",
            other => {
                return Err(format!(
                    "{other:?} is not a notification urgency: low, normal or critical"
                ));
            }
        }),
    );
    if let Some(icon) = &options.icon {
        notification.insert("icon".to_owned(), serialized_icon(icon)?);
    }

    notification.insert("default-action".to_owned(), owned(ACTION));
    notification.insert(
        "default-action-target".to_owned(),
        owned(
            serde_json::to_string(&envelope(public_id, None, notification_session)?)
                .expect("an envelope serializes"),
        ),
    );

    if !options.actions.is_empty() {
        let buttons = options
            .actions
            .iter()
            .map(|action| {
                Ok(HashMap::from([
                    ("label".to_owned(), owned(action.title.clone())),
                    ("action".to_owned(), owned(ACTION)),
                    (
                        "target".to_owned(),
                        owned(
                            serde_json::to_string(&envelope(
                                public_id,
                                Some(&action.id),
                                notification_session,
                            )?)
                            .expect("an envelope serializes"),
                        ),
                    ),
                ]))
            })
            .collect::<Result<Vec<_>, String>>()?;
        notification.insert("buttons".to_owned(), owned(buttons));
    }
    Ok(notification)
}

type Errors = Arc<Mutex<VecDeque<(String, String)>>>;
type Store = Arc<Mutex<ActivationStore>>;

struct Application {
    store: Store,
    errors: Errors,
    present: Arc<AtomicBool>,
    proxy: EventLoopProxy,
}

impl Application {
    fn present(&self) {
        self.present.store(true, Ordering::Release);
        self.proxy.wake_up();
    }

    fn record(&self, action_name: &str, parameter: &[OwnedValue]) {
        if action_name != ACTION_NAME {
            return;
        }
        let result = parameter
            .first()
            .ok_or_else(|| "Linux notification activation carried no target".to_owned())
            .and_then(|value| {
                value
                    .try_clone()
                    .map_err(|error| {
                        format!("could not retain Linux notification activation target: {error}")
                    })
                    .and_then(|value| {
                        String::try_from(value).map_err(|error| {
                            format!("Linux notification activation target was not text: {error}")
                        })
                    })
            })
            .and_then(|text| Activation::parse(&text))
            .and_then(|activation| {
                let id = activation.id.clone();
                self.store
                    .lock()
                    .record(activation)
                    .map_err(|error| format!("{id}\0{error}"))
            });
        if let Err(error) = result {
            let (id, message) = error
                .split_once('\0')
                .map_or(("", error.as_str()), |(id, message)| (id, message));
            self.errors
                .lock()
                .push_back((id.to_owned(), message.to_owned()));
        }
        self.proxy.wake_up();
    }
}

#[zbus::interface(name = "org.freedesktop.Application")]
impl Application {
    fn activate(&self, _platform_data: HashMap<String, OwnedValue>) {
        self.present();
    }

    fn open(&self, _uris: Vec<String>, _platform_data: HashMap<String, OwnedValue>) {
        self.present();
    }

    async fn activate_action(
        &self,
        action_name: &str,
        parameter: Vec<OwnedValue>,
        _platform_data: HashMap<String, OwnedValue>,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> zbus::fdo::Result<()> {
        let sender = header.sender().ok_or_else(|| {
            zbus::fdo::Error::AccessDenied(
                "Linux notification activation had no D-Bus sender".to_owned(),
            )
        })?;
        let bus = zbus::fdo::DBusProxy::new(connection).await?;
        let portal = bus
            .get_name_owner(
                PORTAL_DESTINATION
                    .try_into()
                    .expect("the portal destination is a D-Bus name"),
            )
            .await?;
        if sender.as_str() != portal.as_str() {
            return Err(zbus::fdo::Error::AccessDenied(
                "only the notification portal may activate a notification action".to_owned(),
            ));
        }
        self.record(action_name, &parameter);
        self.present();
        Ok(())
    }
}

pub(crate) struct LinuxPortal {
    connection: Connection,
    identity: String,
    registration: Mutex<PortalRegistration>,
    store: Store,
    errors: Errors,
    present: Arc<AtomicBool>,
    session: String,
}

impl LinuxPortal {
    pub(crate) fn new(proxy: EventLoopProxy) -> Result<Option<Self>, String> {
        let Some(entry) = super::entry_point() else {
            return Ok(None);
        };
        let directory = super::store_directory(&entry.identity)?;
        let store = Arc::new(Mutex::new(ActivationStore::new(
            &directory,
            &entry.identity,
        )));
        let errors = Arc::new(Mutex::new(VecDeque::new()));
        let present = Arc::new(AtomicBool::new(false));
        let connection = connection::Builder::session()
            .map_err(|error| {
                format!("could not connect Linux notification activation to D-Bus: {error}")
            })?
            .serve_at(
                object_path(&entry.entry),
                Application {
                    store: Arc::clone(&store),
                    errors: Arc::clone(&errors),
                    present: Arc::clone(&present),
                    proxy,
                },
            )
            .and_then(|builder| builder.name(entry.entry.as_str()))
            .and_then(connection::Builder::build)
            .map_err(|error| {
                format!(
                    "could not register Linux notification activation service {}: {error}",
                    entry.entry
                )
            })?;
        let portal = Self {
            connection,
            identity: entry.entry.clone(),
            registration: Mutex::new(PortalRegistration::default()),
            store,
            errors,
            present,
            session: super::session_token(),
        };
        // A host process has no Flatpak metadata for the portal to infer an
        // application ID from. The host registry binds this connection to the
        // packaged desktop-file ID before its first portal call; without it the
        // notification would be filed under an empty ID and could not activate
        // the service bearing `entry.entry` after this connection exits.
        portal.ensure_registered()?;
        Ok(Some(portal))
    }

    fn portal_owner(&self) -> Result<String, String> {
        let bus = zbus::blocking::fdo::DBusProxy::new(&self.connection).map_err(|error| {
            format!("could not find the Linux notification portal D-Bus owner: {error}")
        })?;
        bus.get_name_owner(
            PORTAL_DESTINATION
                .try_into()
                .expect("the portal destination is a D-Bus name"),
        )
        .map(|owner| owner.to_string())
        .map_err(|error| {
            format!("could not find the Linux notification portal D-Bus owner: {error}")
        })
    }

    fn ensure_registered(&self) -> Result<(), String> {
        let owner = self.portal_owner()?;
        self.registration.lock().ensure(&owner, || {
            Proxy::new(
                &self.connection,
                PORTAL_DESTINATION,
                PORTAL_PATH,
                HOST_REGISTRY_INTERFACE,
            )
            .and_then(|registry| {
                registry.call::<_, _, ()>(
                    "Register",
                    &(self.identity.as_str(), HashMap::<String, OwnedValue>::new()),
                )
            })
            .map_err(|error| {
                format!(
                    "could not register Linux portal application identity {}: {error}",
                    self.identity
                )
            })
        })
    }

    fn proxy(&self) -> Result<Proxy<'_>, String> {
        Proxy::new(
            &self.connection,
            PORTAL_DESTINATION,
            PORTAL_PATH,
            PORTAL_INTERFACE,
        )
        .map_err(|error| format!("could not connect to the Linux notification portal: {error}"))
    }

    pub(crate) fn show(
        &self,
        public_id: &str,
        options: &NotificationOptions,
    ) -> Result<(), String> {
        self.ensure_registered()?;
        let notification = payload(public_id, options, &self.session)?;
        self.proxy()?
            .call::<_, _, ()>("AddNotification", &(public_id, notification))
            .map_err(|error| format!("could not show notification through the portal: {error}"))
    }

    pub(crate) fn close(&self, public_id: &str) -> Result<(), String> {
        self.ensure_registered()?;
        self.proxy()?
            .call::<_, _, ()>("RemoveNotification", &(public_id,))
            .map_err(|error| format!("could not close notification through the portal: {error}"))
    }

    pub(crate) fn take(&self) -> (Result<Vec<Activation>, String>, Vec<(String, String)>) {
        let activations = self.store.lock().take();
        let errors = self.errors.lock().drain(..).collect();
        (activations, errors)
    }

    pub(crate) fn is_current(&self, activation: &Activation, active_record: bool) -> bool {
        super::addresses_session(activation, &self.session, active_record)
    }

    pub(crate) fn take_present_request(&self) -> bool {
        self.present.swap(false, Ordering::AcqRel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &OwnedValue) -> String {
        String::try_from(value.try_clone().expect("an owned portal value clones"))
            .expect("the portal value is text")
    }

    fn options() -> NotificationOptions {
        NotificationOptions {
            title: "Build complete".into(),
            body: "The artifact is ready".into(),
            app_name: None,
            timeout: None,
            urgency: "critical".into(),
            icon: Some("com.example.Pong".into()),
            actions: vec![crate::dom_bridge::notify::NotificationAction {
                id: "open".into(),
                title: "Open".into(),
            }],
        }
    }

    #[test]
    fn the_application_object_path_follows_the_desktop_entry_specification() {
        assert_eq!(object_path("com.example.Pong-App"), "/com/example/Pong_App");
    }

    #[test]
    fn portal_values_keep_body_and_action_identity() {
        let _ = super::super::ENTRY_POINT.set(crate::ActivationEntryPoint {
            identity: "com.example.Pong".into(),
            entry: "com.example.Pong".into(),
        });
        let payload = payload("n7", &options(), "session-1").expect("a portal payload");
        assert_eq!(text(&payload["priority"]), "urgent");
        assert_eq!(text(&payload["default-action"]), ACTION);
        let body = text(&payload["default-action-target"]);
        let body = Activation::parse(&body).expect("body envelope");
        assert_eq!(body.id, "n7");
        assert_eq!(body.action, None);
        assert_eq!(body.session.as_deref(), Some("session-1"));

        let buttons =
            Vec::<HashMap<String, OwnedValue>>::try_from(payload["buttons"].try_clone().unwrap())
                .expect("button dictionaries");
        let target = text(&buttons[0]["target"]);
        assert_eq!(
            Activation::parse(&target).unwrap().action.as_deref(),
            Some("open")
        );
    }

    #[test]
    fn the_portal_icon_is_a_supported_serialized_themed_icon() {
        let icon = serialized_icon("com.example.Pong").expect("a themed icon");
        let (kind, names) = <(String, OwnedValue)>::try_from(icon)
            .expect("the icon is the portal's (sv) serialization");
        assert_eq!(kind, "themed");
        assert_eq!(
            Value::from(names)
                .downcast::<Vec<String>>()
                .expect("the themed payload is an array of names"),
            ["com.example.Pong"]
        );
    }

    #[test]
    fn an_absolute_icon_is_rejected_instead_of_sending_an_invalid_file_variant() {
        let error = serialized_icon("/opt/Pong/icon.png")
            .expect_err("the portal supports image files only through a sealed descriptor");
        assert!(error.contains("sealed file descriptors"), "{error}");
        assert!(error.contains("icon-theme name"), "{error}");
    }

    #[test]
    fn portal_identity_is_registered_again_only_after_the_owner_changes() {
        let mut state = PortalRegistration::default();
        let mut registrations = Vec::new();
        state
            .ensure(":1.20", || {
                registrations.push(":1.20");
                Ok(())
            })
            .unwrap();
        state
            .ensure(":1.20", || {
                registrations.push("duplicate");
                Ok(())
            })
            .unwrap();
        state
            .ensure(":1.31", || {
                registrations.push(":1.31");
                Ok(())
            })
            .unwrap();
        assert_eq!(registrations, [":1.20", ":1.31"]);
    }

    #[test]
    fn a_failed_portal_registration_is_retried_for_the_same_owner() {
        let mut state = PortalRegistration::default();
        assert_eq!(
            state
                .ensure(":1.20", || Err("portal restarting".to_owned()))
                .expect_err("a failed registration remains a failure"),
            "portal restarting"
        );
        let mut retried = false;
        state
            .ensure(":1.20", || {
                retried = true;
                Ok(())
            })
            .unwrap();
        assert!(retried);
    }
}
