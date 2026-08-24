//! Android notifications through the platform SDK and `jni`.
//!
//! `android-activity` owns the JVM/activity references and supplies the
//! Java-main-thread hop required by `requestPermissions`. NotificationManager
//! itself is safe to call from the session thread, so delivery, replacement and
//! cancellation stay synchronous with the native-module command queue.
//!
//! # The activation trampoline (#252)
//!
//! A tapped Android notification is an `Intent` the system sends on the
//! application's behalf, and something has to be waiting to receive it. The
//! body, action and delete trampolines are immutable `getBroadcast` intents
//! aimed at one private receiver. It persists the envelope before body/actions
//! launch the platform `NativeActivity` with a clean Intent; dismissal does not
//! open a window. The exported launcher never reads activation extras, so an
//! explicit Intent from another package or adb cannot forge a trusted event.
//! Rust drains the atomic inbox through the nonce/replay store.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use android_activity::AndroidApp;
use jni::objects::{JObject, JString, JValue};
use jni::{Env, JavaVM, jni_sig, jni_str};
use parking_lot::Mutex;
use serde_json::{Value, json};
use winit::event_loop::EventLoopProxy;

use super::ActivationStore;
use crate::dom_bridge::notify::{Activation, NotificationOptions, NotificationPatch};

const CHANNEL_ID: &str = "blitsen.default";
const PERMISSION: &str = "android.permission.POST_NOTIFICATIONS";
const PREFERENCES: &str = "blitsen.notifications";
const REQUESTED_KEY: &str = "permissionRequested";
const REQUEST_CODE: i32 = 0x424e;

/// The extra the activation envelope travels in.
const ACTIVATION_EXTRA: &str = "blitsen.notification.activation";
const NONCE_EXTRA: &str = "blitsen.notification.nonce";
const LAUNCH_EXTRA: &str = "blitsen.notification.launch";
const ACTIVATION_INBOX: &str = "notification-activation-inbox";

/// The scheme that makes one trampoline `Intent` different from another.
///
/// `PendingIntent` deduplicates by `Intent.filterEquals`, which compares the
/// action, the data, the type, the component and the categories — and ignores
/// extras entirely. Two notifications, or a body tap and a button on the same
/// notification, would therefore collapse into one `PendingIntent` and deliver
/// whichever envelope was registered first. Giving each one a data URI of its
/// own is what keeps them distinct, and the nonce is already the thing that is
/// unique per activation.
const ACTIVATION_SCHEME: &str = "blitsen-notification";

/// `PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT`.
///
/// Immutable is required from API 31 and is right regardless: the envelope is
/// this application's statement about which notification was tapped, and a
/// mutable `PendingIntent` is one another application could fill in. Updating
/// the current one is what makes a replacement (`update`) re-aim an existing
/// trampoline at its new envelope instead of leaving the old one live.
const PENDING_INTENT_FLAGS: i32 = 0x0400_0000 | 0x0800_0000;

static NEXT_ACTIVATION_NONCE: AtomicU64 = AtomicU64::new(1);

static ANDROID_APP: OnceLock<RwLock<Option<AndroidApp>>> = OnceLock::new();

pub(crate) fn set_android_app(app: AndroidApp) {
    *ANDROID_APP
        .get_or_init(|| RwLock::new(None))
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(app);
}

/// The Activity handle, which is `android-activity`'s and is shared rather than
/// duplicated: `blitsen/hid` reaches `UsbManager` through the same one (#248).
pub(crate) fn android_app() -> Result<AndroidApp, String> {
    ANDROID_APP
        .get()
        .and_then(|slot| {
            slot.read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        })
        .ok_or_else(|| "the Android activity is not available".to_owned())
}

/// Runs `operation` with an attached `Env` and this process's Activity.
///
/// Shared with `blitsen/hid` for the same reason the handle above is: there is
/// one Activity, one JVM and one way to reach them, and a second copy of this
/// would be a second place for the unsafety argument below to be wrong.
pub(crate) fn with_activity<T>(
    app: &AndroidApp,
    operation: impl FnOnce(&mut Env<'_>, &JObject<'_>) -> jni::errors::Result<T>,
) -> Result<T, String> {
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) };
    let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
    vm.attach_current_thread(|env| {
        // SAFETY: android-activity documents this as an unowned global
        // reference valid for as long as this AndroidApp handle is retained.
        let activity = unsafe { env.as_cast_raw::<JObject>(&raw_activity)? };
        operation(env, &activity)
    })
    .map_err(|error| format!("the Android platform call failed: {error}"))
}

fn sdk_int(app: &AndroidApp) -> Result<i32, String> {
    with_activity(app, |env, _| {
        env.get_static_field(
            jni_str!("android/os/Build$VERSION"),
            jni_str!("SDK_INT"),
            jni_sig!("I"),
        )?
        .i()
    })
}

fn permission_granted(app: &AndroidApp) -> Result<bool, String> {
    if sdk_int(app)? < 33 {
        return Ok(true);
    }
    with_activity(app, |env, activity| {
        let permission = env.new_string(PERMISSION)?;
        Ok(env
            .call_method(
                activity,
                jni_str!("checkSelfPermission"),
                jni_sig!("(Ljava/lang/String;)I"),
                &[JValue::Object(&permission)],
            )?
            .i()?
            == 0)
    })
}

fn permission_requested(app: &AndroidApp) -> Result<bool, String> {
    with_activity(app, |env, activity| {
        let name = env.new_string(PREFERENCES)?;
        let preferences = env
            .call_method(
                activity,
                jni_str!("getSharedPreferences"),
                jni_sig!("(Ljava/lang/String;I)Landroid/content/SharedPreferences;"),
                &[JValue::Object(&name), JValue::Int(0)],
            )?
            .l()?;
        let key = env.new_string(REQUESTED_KEY)?;
        env.call_method(
            &preferences,
            jni_str!("getBoolean"),
            jni_sig!("(Ljava/lang/String;Z)Z"),
            &[JValue::Object(&key), JValue::Bool(false)],
        )?
        .z()
    })
}

fn remember_permission_request(app: &AndroidApp) -> Result<(), String> {
    with_activity(app, |env, activity| {
        let name = env.new_string(PREFERENCES)?;
        let preferences = env
            .call_method(
                activity,
                jni_str!("getSharedPreferences"),
                jni_sig!("(Ljava/lang/String;I)Landroid/content/SharedPreferences;"),
                &[JValue::Object(&name), JValue::Int(0)],
            )?
            .l()?;
        let editor = env
            .call_method(
                &preferences,
                jni_str!("edit"),
                jni_sig!("()Landroid/content/SharedPreferences$Editor;"),
                &[],
            )?
            .l()?;
        let key = env.new_string(REQUESTED_KEY)?;
        let editor = env
            .call_method(
                &editor,
                jni_str!("putBoolean"),
                jni_sig!("(Ljava/lang/String;Z)Landroid/content/SharedPreferences$Editor;"),
                &[JValue::Object(&key), JValue::Bool(true)],
            )?
            .l()?;
        env.call_method(&editor, jni_str!("apply"), jni_sig!("()V"), &[])?;
        Ok(())
    })
}

fn has_window_focus(app: &AndroidApp) -> Result<bool, String> {
    with_activity(app, |env, activity| {
        env.call_method(activity, jni_str!("hasWindowFocus"), jni_sig!("()Z"), &[])?
            .z()
    })
}

fn request_android_permission(
    app: AndroidApp,
    signals: Arc<Mutex<VecDeque<String>>>,
    proxy: EventLoopProxy,
) {
    let outer = app.clone();
    app.run_on_java_main_thread(Box::new(move || {
        let result = with_activity(&outer, |env, activity| {
            let permission = env.new_string(PERMISSION)?;
            let string_class = env.find_class(jni_str!("java/lang/String"))?;
            let permissions = env.new_object_array(1, &string_class, JObject::null())?;
            permissions.set_element(env, 0, &permission)?;
            env.call_method(
                activity,
                jni_str!("requestPermissions"),
                jni_sig!("([Ljava/lang/String;I)V"),
                &[JValue::Object(&permissions), JValue::Int(REQUEST_CODE)],
            )?;
            Ok(())
        });
        if let Err(error) = result {
            signals.lock().push_back(error);
            proxy.wake_up();
        }
    }));
}

/// The application ID this package was installed under.
///
/// Read rather than recorded by the export: an Android install is keyed by this
/// string, the manifest declared it, and the system will not let the two differ
/// — so asking the Activity is both simpler than carrying it in the artifact and
/// impossible to get out of step with it.
pub(crate) fn installed_entry_point() -> Option<crate::ActivationEntryPoint> {
    let app = android_app().ok()?;
    let package = with_activity(&app, |env, activity| {
        let name = env
            .call_method(
                activity,
                jni_str!("getPackageName"),
                jni_sig!("()Ljava/lang/String;"),
                &[],
            )?
            .l()?;
        env.cast_local::<JString>(name)?.try_to_string(env)
    })
    .ok()?;
    Some(crate::ActivationEntryPoint {
        entry: package.clone(),
        identity: package,
    })
}

/// The directory the Activity owns, which is where the activation queue lives.
///
/// `filesDir` rather than an XDG path: Android sets none of the variables the
/// desktop answer reads, which is why `blitsen_platform::app` is absent on this
/// platform rather than answering a path nothing can write to.
pub(crate) fn files_directory() -> Result<PathBuf, String> {
    let app = android_app()?;
    with_activity(&app, |env, activity| {
        let directory = env
            .call_method(
                activity,
                jni_str!("getFilesDir"),
                jni_sig!("()Ljava/io/File;"),
                &[],
            )?
            .l()?;
        let path = env
            .call_method(
                &directory,
                jni_str!("getAbsolutePath"),
                jni_sig!("()Ljava/lang/String;"),
                &[],
            )?
            .l()?;
        let path = env.cast_local::<JString>(path)?;
        Ok(PathBuf::from(path.try_to_string(env)?))
    })
}

/// Moves Java activation callbacks into the process-independent replay store.
///
/// The Java bridge writes one file per nonce. A successful record is removed;
/// a write failure leaves it for the next frame or launch, while malformed data
/// is removed after it is reported so one bad callback cannot emit forever.
pub(crate) fn record_inbox_activations(directory: &std::path::Path, store: &ActivationStore) {
    let inbox = directory.join(ACTIVATION_INBOX);
    for (id, error) in store.record_inbox(&inbox) {
        crate::dom_bridge::notify::failed(id, error);
    }
}

/// The envelope a tap on `public_id`, or on one of its buttons, will deliver.
///
/// `None` when nothing registered an identity for this process: there is then no
/// application for an activation to be addressed to, and a trampoline built
/// anyway would hand the next launch an envelope it must refuse.
fn envelope(
    public_id: &str,
    action: Option<&str>,
    dismissed: Option<&str>,
    native_id: i32,
    notification_session: &str,
) -> Option<Activation> {
    let entry_point = super::entry_point()?;
    // Unique across launches as well as within one: the notification survives
    // the process, so a nonce built only from the notification's own identity
    // would collide with one the store had already consumed after a restart.
    let minted = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    let sequence = NEXT_ACTIVATION_NONCE.fetch_add(1, Ordering::Relaxed);
    Some(Activation {
        nonce: format!("{minted:x}-{native_id:x}-{sequence:x}"),
        identity: entry_point.identity.clone(),
        id: public_id.to_owned(),
        session: Some(notification_session.to_owned()),
        action: action.map(str::to_owned),
        dismissed: dismissed.map(str::to_owned),
        platform: "android".to_owned(),
        entry: entry_point.entry.clone(),
    })
}

/// A private receiver PendingIntent that persists `activation` before launch.
fn trampoline<'env>(
    env: &mut Env<'env>,
    activity: &JObject<'_>,
    activation: &Activation,
    launch: bool,
) -> jni::errors::Result<JObject<'env>> {
    let receiver = JObject::from(env.find_class(jni_str!(
        "com/blitsen/runtime/NotificationBridge$ActivationReceiver"
    ))?);
    let intent = env.new_object(
        jni_str!("android/content/Intent"),
        jni_sig!("(Landroid/content/Context;Ljava/lang/Class;)V"),
        &[JValue::Object(activity), JValue::Object(&receiver)],
    )?;
    let kind = if launch { "activate" } else { "dismiss" };
    let uri = env.new_string(format!("{ACTIVATION_SCHEME}:{kind}-{}", activation.nonce))?;
    let data = env
        .call_static_method(
            jni_str!("android/net/Uri"),
            jni_str!("parse"),
            jni_sig!("(Ljava/lang/String;)Landroid/net/Uri;"),
            &[JValue::Object(&uri)],
        )?
        .l()?;
    env.call_method(
        &intent,
        jni_str!("setData"),
        jni_sig!("(Landroid/net/Uri;)Landroid/content/Intent;"),
        &[JValue::Object(&data)],
    )?;
    let key = env.new_string(ACTIVATION_EXTRA)?;
    let value = env.new_string(
        serde_json::to_string(activation).expect("an activation envelope serializes"),
    )?;
    env.call_method(
        &intent,
        jni_str!("putExtra"),
        jni_sig!("(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;"),
        &[JValue::Object(&key), JValue::Object(&value)],
    )?;
    let nonce_key = env.new_string(NONCE_EXTRA)?;
    let nonce = env.new_string(&activation.nonce)?;
    env.call_method(
        &intent,
        jni_str!("putExtra"),
        jni_sig!("(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;"),
        &[JValue::Object(&nonce_key), JValue::Object(&nonce)],
    )?;
    let launch_key = env.new_string(LAUNCH_EXTRA)?;
    env.call_method(
        &intent,
        jni_str!("putExtra"),
        jni_sig!("(Ljava/lang/String;Z)Landroid/content/Intent;"),
        &[JValue::Object(&launch_key), JValue::Bool(launch)],
    )?;
    env.call_static_method(
        jni_str!("android/app/PendingIntent"),
        jni_str!("getBroadcast"),
        jni_sig!(
            "(Landroid/content/Context;ILandroid/content/Intent;I)Landroid/app/PendingIntent;"
        ),
        &[
            JValue::Object(activity),
            JValue::Int(0),
            JValue::Object(&intent),
            JValue::Int(PENDING_INTENT_FLAGS),
        ],
    )?
    .l()
}

fn notification_manager<'env, 'object>(
    env: &mut Env<'env>,
    activity: &JObject<'object>,
) -> jni::errors::Result<JObject<'env>> {
    let service = env.new_string("notification")?;
    env.call_method(
        activity,
        jni_str!("getSystemService"),
        jni_sig!("(Ljava/lang/String;)Ljava/lang/Object;"),
        &[JValue::Object(&service)],
    )?
    .l()
}

fn create_channel(env: &mut Env<'_>, manager: &JObject<'_>) -> jni::errors::Result<()> {
    let id = env.new_string(CHANNEL_ID)?;
    let name = env.new_string("General")?;
    let channel = env.new_object(
        jni_str!("android/app/NotificationChannel"),
        jni_sig!("(Ljava/lang/String;Ljava/lang/CharSequence;I)V"),
        &[
            JValue::Object(&id),
            JValue::Object(&name),
            JValue::Int(3), // NotificationManager.IMPORTANCE_DEFAULT
        ],
    )?;
    env.call_method(
        manager,
        jni_str!("createNotificationChannel"),
        jni_sig!("(Landroid/app/NotificationChannel;)V"),
        &[JValue::Object(&channel)],
    )?;
    Ok(())
}

fn small_icon(
    env: &mut Env<'_>,
    activity: &JObject<'_>,
    requested: Option<&str>,
) -> jni::errors::Result<i32> {
    let resources = env
        .call_method(
            activity,
            jni_str!("getResources"),
            jni_sig!("()Landroid/content/res/Resources;"),
            &[],
        )?
        .l()?;
    let (name, package) = if let Some(name) = requested {
        let package = env
            .call_method(
                activity,
                jni_str!("getPackageName"),
                jni_sig!("()Ljava/lang/String;"),
                &[],
            )?
            .l()?;
        (env.new_string(name)?, package)
    } else {
        (
            env.new_string("ic_dialog_info")?,
            JObject::from(env.new_string("android")?),
        )
    };
    let drawable = env.new_string("drawable")?;
    env.call_method(
        &resources,
        jni_str!("getIdentifier"),
        jni_sig!("(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)I"),
        &[
            JValue::Object(&name),
            JValue::Object(&drawable),
            JValue::Object(&package),
        ],
    )?
    .i()
}

fn post(
    app: &AndroidApp,
    public_id: &str,
    native_id: i32,
    options: &NotificationOptions,
    notification_session: &str,
) -> Result<(), String> {
    // A button whose tap cannot be addressed back to this application would be
    // a control that does nothing, so it is refused rather than drawn.
    if !options.actions.is_empty() && super::entry_point().is_none() {
        return Err(super::NO_ACTIVATION_IDENTITY.to_owned());
    }
    with_activity(app, |env, activity| {
        let manager = notification_manager(env, activity)?;
        create_channel(env, &manager)?;
        let channel = env.new_string(CHANNEL_ID)?;
        let builder = env.new_object(
            jni_str!("android/app/Notification$Builder"),
            jni_sig!("(Landroid/content/Context;Ljava/lang/String;)V"),
            &[JValue::Object(activity), JValue::Object(&channel)],
        )?;
        let title = env.new_string(&options.title)?;
        env.call_method(
            &builder,
            jni_str!("setContentTitle"),
            jni_sig!("(Ljava/lang/CharSequence;)Landroid/app/Notification$Builder;"),
            &[JValue::Object(&title)],
        )?;
        let body = env.new_string(&options.body)?;
        env.call_method(
            &builder,
            jni_str!("setContentText"),
            jni_sig!("(Ljava/lang/CharSequence;)Landroid/app/Notification$Builder;"),
            &[JValue::Object(&body)],
        )?;
        let icon = small_icon(env, activity, options.icon.as_deref())?;
        if icon == 0 {
            return Err(jni::errors::Error::NullPtr("notification icon resource"));
        }
        env.call_method(
            &builder,
            jni_str!("setSmallIcon"),
            jni_sig!("(I)Landroid/app/Notification$Builder;"),
            &[JValue::Int(icon)],
        )?;
        env.call_method(
            &builder,
            jni_str!("setAutoCancel"),
            jni_sig!("(Z)Landroid/app/Notification$Builder;"),
            &[JValue::Bool(true)],
        )?;
        let priority = match options.urgency.as_str() {
            "low" => -1,
            "normal" => 0,
            "critical" => 2,
            _ => 0,
        };
        env.call_method(
            &builder,
            jni_str!("setPriority"),
            jni_sig!("(I)Landroid/app/Notification$Builder;"),
            &[JValue::Int(priority)],
        )?;
        if let Some(timeout) = options.timeout {
            if timeout == 0 {
                env.call_method(
                    &builder,
                    jni_str!("setOngoing"),
                    jni_sig!("(Z)Landroid/app/Notification$Builder;"),
                    &[JValue::Bool(true)],
                )?;
            } else {
                env.call_method(
                    &builder,
                    jni_str!("setTimeoutAfter"),
                    jni_sig!("(J)Landroid/app/Notification$Builder;"),
                    &[JValue::Long(i64::from(timeout))],
                )?;
            }
        }
        // What a tap does (#252). Without a registered identity there is no
        // envelope to carry, and a body tap then only dismisses the
        // notification — which is what it did before this existed.
        if let Some(activation) = envelope(public_id, None, None, native_id, notification_session) {
            let intent = trampoline(env, activity, &activation, true)?;
            env.call_method(
                &builder,
                jni_str!("setContentIntent"),
                jni_sig!("(Landroid/app/PendingIntent;)Landroid/app/Notification$Builder;"),
                &[JValue::Object(&intent)],
            )?;
        }
        if let Some(activation) = envelope(
            public_id,
            None,
            Some("dismissed"),
            native_id,
            notification_session,
        ) {
            let intent = trampoline(env, activity, &activation, false)?;
            env.call_method(
                &builder,
                jni_str!("setDeleteIntent"),
                jni_sig!("(Landroid/app/PendingIntent;)Landroid/app/Notification$Builder;"),
                &[JValue::Object(&intent)],
            )?;
        }
        for action in &options.actions {
            let Some(activation) = envelope(
                public_id,
                Some(&action.id),
                None,
                native_id,
                notification_session,
            ) else {
                continue;
            };
            let intent = trampoline(env, activity, &activation, true)?;
            let title = env.new_string(&action.title)?;
            // Icon 0, because `blitsen/notify` actions carry a title and an ID
            // and no icon — the same surface every other platform's actions have
            // here. This constructor is the one that takes the three of them.
            let built = env.new_object(
                jni_str!("android/app/Notification$Action"),
                jni_sig!("(ILjava/lang/CharSequence;Landroid/app/PendingIntent;)V"),
                &[
                    JValue::Int(0),
                    JValue::Object(&title),
                    JValue::Object(&intent),
                ],
            )?;
            env.call_method(
                &builder,
                jni_str!("addAction"),
                jni_sig!("(Landroid/app/Notification$Action;)Landroid/app/Notification$Builder;"),
                &[JValue::Object(&built)],
            )?;
        }
        let notification = env
            .call_method(
                &builder,
                jni_str!("build"),
                jni_sig!("()Landroid/app/Notification;"),
                &[],
            )?
            .l()?;
        env.call_method(
            &manager,
            jni_str!("notify"),
            jni_sig!("(ILandroid/app/Notification;)V"),
            &[JValue::Int(native_id), JValue::Object(&notification)],
        )?;
        Ok(())
    })
}

fn cancel(app: &AndroidApp, native_id: i32) -> Result<(), String> {
    with_activity(app, |env, activity| {
        let manager = notification_manager(env, activity)?;
        env.call_method(
            &manager,
            jni_str!("cancel"),
            jni_sig!("(I)V"),
            &[JValue::Int(native_id)],
        )?;
        Ok(())
    })
}

struct Record {
    options: NotificationOptions,
    native_id: i32,
}

struct PermissionPrompt {
    command_ids: Vec<u64>,
    started: Instant,
    saw_focus_loss: bool,
}

pub(crate) struct NotifyController {
    app: AndroidApp,
    proxy: EventLoopProxy,
    activation_directory: PathBuf,
    activation_store: ActivationStore,
    notification_session: String,
    records: HashMap<String, Record>,
    next_native_id: i32,
    permission_prompt: Option<PermissionPrompt>,
    permission_errors: Arc<Mutex<VecDeque<String>>>,
}

impl NotifyController {
    pub(crate) fn new(proxy: EventLoopProxy) -> Self {
        let entry_point = super::entry_point()
            .expect("the Android package identity is installed before its notification controller");
        let activation_directory = files_directory()
            .expect("the Android files directory is available while its Activity is alive");
        Self {
            app: android_app().expect("Android activity is installed before the window session"),
            proxy,
            activation_store: ActivationStore::new(&activation_directory, &entry_point.identity),
            activation_directory,
            notification_session: super::session_token(),
            records: HashMap::new(),
            next_native_id: 1,
            permission_prompt: None,
            permission_errors: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    pub(crate) fn permission(_request: bool) -> Result<Value, String> {
        let app = android_app()?;
        if permission_granted(&app)? {
            Ok(json!("granted"))
        } else if permission_requested(&app)? {
            Ok(json!("denied"))
        } else {
            Ok(json!("default"))
        }
    }

    pub(crate) fn request_permission(&mut self, command_id: u64) {
        match Self::permission(false) {
            Ok(value) if value != json!("default") => {
                crate::dom_bridge::notify::complete(command_id, Ok(value));
            }
            Err(error) => crate::dom_bridge::notify::complete(command_id, Err(error)),
            Ok(_) => {
                if let Some(prompt) = &mut self.permission_prompt {
                    prompt.command_ids.push(command_id);
                    return;
                }
                if let Err(error) = remember_permission_request(&self.app) {
                    crate::dom_bridge::notify::complete(command_id, Err(error));
                    return;
                }
                self.permission_prompt = Some(PermissionPrompt {
                    command_ids: vec![command_id],
                    started: Instant::now(),
                    saw_focus_loss: false,
                });
                request_android_permission(
                    self.app.clone(),
                    Arc::clone(&self.permission_errors),
                    self.proxy.clone(),
                );
            }
        }
    }

    pub(crate) fn show(
        &mut self,
        public_id: String,
        options: NotificationOptions,
    ) -> Result<Value, String> {
        if !permission_granted(&self.app)? {
            return Err("notification permission has not been granted".to_owned());
        }
        let native_id = self.next_native_id;
        self.next_native_id = self.next_native_id.saturating_add(1);
        post(
            &self.app,
            &public_id,
            native_id,
            &options,
            &self.notification_session,
        )?;
        self.records
            .insert(public_id.clone(), Record { options, native_id });
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
        post(
            &self.app,
            public_id,
            record.native_id,
            &record.options,
            &self.notification_session,
        )?;
        Ok(json!(true))
    }

    pub(crate) fn close(&mut self, public_id: &str) -> Result<Value, String> {
        let Some(record) = self.records.remove(public_id) else {
            return Ok(json!(false));
        };
        cancel(&self.app, record.native_id)?;
        crate::dom_bridge::notify::closed(public_id.to_owned(), "closed");
        Ok(json!(true))
    }

    pub(crate) fn poll(&mut self) {
        record_inbox_activations(&self.activation_directory, &self.activation_store);
        let activations = match self.activation_store.take() {
            Ok(activations) => activations,
            Err(error) => {
                crate::dom_bridge::notify::failed(String::new(), error);
                Vec::new()
            }
        };
        for activation in activations {
            let current = super::addresses_session(
                &activation,
                &self.notification_session,
                self.records.contains_key(&activation.id),
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
        if let Some(error) = self.permission_errors.lock().pop_front() {
            if let Some(prompt) = self.permission_prompt.take() {
                for command_id in prompt.command_ids {
                    crate::dom_bridge::notify::complete(command_id, Err(error.clone()));
                }
            }
            return;
        }
        let Some(prompt) = &mut self.permission_prompt else {
            return;
        };
        match permission_granted(&self.app) {
            Ok(true) => self.finish_permission(json!("granted")),
            Ok(false) => match has_window_focus(&self.app) {
                Ok(false) => prompt.saw_focus_loss = true,
                Ok(true)
                    if prompt.saw_focus_loss
                        || prompt.started.elapsed() >= Duration::from_secs(2) =>
                {
                    self.finish_permission(json!("denied"));
                }
                Ok(true) => {}
                Err(error) => self.fail_permission(error),
            },
            Err(error) => self.fail_permission(error),
        }
    }

    fn finish_permission(&mut self, value: Value) {
        if let Some(prompt) = self.permission_prompt.take() {
            for command_id in prompt.command_ids {
                crate::dom_bridge::notify::complete(command_id, Ok(value.clone()));
            }
        }
    }

    fn fail_permission(&mut self, error: String) {
        if let Some(prompt) = self.permission_prompt.take() {
            for command_id in prompt.command_ids {
                crate::dom_bridge::notify::complete(command_id, Err(error.clone()));
            }
        }
    }

    pub(crate) fn clear(&mut self) {
        let records = self
            .records
            .drain()
            .map(|(_, record)| record)
            .collect::<Vec<_>>();
        for record in records {
            let _ = cancel(&self.app, record.native_id);
        }
        self.permission_prompt = None;
        self.permission_errors.lock().clear();
    }

    /// Releases callbacks at process shutdown without cancelling the platform
    /// notifications whose PendingIntents can start the next process (#252).
    pub(crate) fn detach(&mut self) {
        self.records.clear();
        self.permission_prompt = None;
        self.permission_errors.lock().clear();
    }
}
