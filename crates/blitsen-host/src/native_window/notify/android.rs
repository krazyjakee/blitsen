//! Android notifications through the platform SDK and `jni`.
//!
//! `android-activity` owns the JVM/activity references and supplies the
//! Java-main-thread hop required by `requestPermissions`. NotificationManager
//! itself is safe to call from the session thread, so delivery, replacement and
//! cancellation stay synchronous with the native-module command queue.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use android_activity::AndroidApp;
use jni::objects::{JObject, JValue};
use jni::{Env, JavaVM, jni_sig, jni_str};
use serde_json::{Value, json};
use winit::event_loop::EventLoopProxy;

use crate::dom_bridge::notify::{NotificationOptions, NotificationPatch};

const CHANNEL_ID: &str = "blitsen.default";
const PERMISSION: &str = "android.permission.POST_NOTIFICATIONS";
const PREFERENCES: &str = "blitsen.notifications";
const REQUESTED_KEY: &str = "permissionRequested";
const REQUEST_CODE: i32 = 0x424e;

static ANDROID_APP: OnceLock<RwLock<Option<AndroidApp>>> = OnceLock::new();

pub(crate) fn set_android_app(app: AndroidApp) {
    *ANDROID_APP
        .get_or_init(|| RwLock::new(None))
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(app);
}

fn android_app() -> Result<AndroidApp, String> {
    ANDROID_APP
        .get()
        .and_then(|slot| {
            slot.read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        })
        .ok_or_else(|| "the Android activity is not available".to_owned())
}

fn with_activity<T>(
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
    .map_err(|error| format!("Android notification API failed: {error}"))
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
            crate::dom_bridge::net_lock(&signals).push_back(error);
            proxy.wake_up();
        }
    }));
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

fn post(app: &AndroidApp, native_id: i32, options: &NotificationOptions) -> Result<(), String> {
    if !options.actions.is_empty() {
        return Err(
            "notification actions require Android activation routing, tracked by issue #252"
                .to_owned(),
        );
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
    records: HashMap<String, Record>,
    next_native_id: i32,
    permission_prompt: Option<PermissionPrompt>,
    permission_errors: Arc<Mutex<VecDeque<String>>>,
}

impl NotifyController {
    pub(crate) fn new(proxy: EventLoopProxy) -> Self {
        Self {
            app: android_app().expect("Android activity is installed before the window session"),
            proxy,
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
        post(&self.app, native_id, &options)?;
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
        post(&self.app, record.native_id, &record.options)?;
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
        if let Some(error) = crate::dom_bridge::net_lock(&self.permission_errors).pop_front() {
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
        crate::dom_bridge::net_lock(&self.permission_errors).clear();
    }
}
