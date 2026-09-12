//! The system's appearance and motion preferences, read once and then
//! followed (#385).
//!
//! Two questions with different sources on each platform. The colour scheme
//! is the window's own business on macOS and Windows — winit reports it at
//! creation and on every change — and this module answers `None` there so the
//! window's answer stands. On Linux winit has nothing to say, and the desktop
//! settings portal (`org.freedesktop.portal.Settings`) is the one source every
//! desktop agrees on; it also carries GNOME's `enable-animations`, which is
//! the closest thing Linux has to a reduced-motion preference. macOS answers
//! motion through `NSWorkspace`, Windows through `SPI_GETCLIENTAREAANIMATION`.
//!
//! Reading happens on a thread of this module's own and never on the frame
//! thread: a D-Bus round trip can wait on a portal that is still starting, and
//! the first paint must not wait behind it. The caller may wait a bounded
//! moment for the first reading so that paint is right rather than corrected
//! a frame later, and afterwards is woken whenever a reading changes — by the
//! portal's own signal on Linux, and by polling every couple of seconds where
//! the platform offers no notification a thread can subscribe to. A platform
//! with no trustworthy answer reports `None`, and the caller's documented
//! fallback stands.

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// The appearance a desktop prefers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColorScheme {
    /// A light appearance.
    Light,
    /// A dark appearance.
    Dark,
}

/// What the platform said, with `None` for each question it did not answer.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Preferences {
    /// The preferred colour scheme, where the platform states one.
    pub color_scheme: Option<ColorScheme>,
    /// Whether motion should be reduced, where the platform states it.
    pub reduced_motion: Option<bool>,
}

/// How often a platform without a change notification is asked again.
#[cfg(any(target_os = "macos", windows))]
const POLL_INTERVAL: Duration = Duration::from_secs(2);

struct Shared {
    current: Mutex<Preferences>,
    first_reading: (Mutex<bool>, Condvar),
    on_change: Box<dyn Fn() + Send + Sync>,
}

impl Shared {
    /// Stores a reading, telling the caller only when it differs.
    fn publish(&self, preferences: Preferences) {
        let changed = {
            let mut current = self
                .current
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let changed = *current != preferences;
            *current = preferences;
            changed
        };
        let (ready, woken) = &self.first_reading;
        let first = {
            let mut ready = ready
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let first = !*ready;
            *ready = true;
            first
        };
        if first {
            woken.notify_all();
        }
        if changed || first {
            (self.on_change)();
        }
    }
}

/// The platform's preferences, kept current on a thread.
pub struct Watcher {
    shared: Arc<Shared>,
}

impl Watcher {
    /// Starts following the preferences, calling `on_change` from the watcher
    /// thread whenever a reading changes — and once after the first reading.
    pub fn start(on_change: impl Fn() + Send + Sync + 'static) -> Self {
        let shared = Arc::new(Shared {
            current: Mutex::new(Preferences::default()),
            first_reading: (Mutex::new(false), Condvar::new()),
            on_change: Box::new(on_change),
        });
        let worker = Arc::clone(&shared);
        let started = std::thread::Builder::new()
            .name("blitsen-appearance".to_owned())
            .spawn(move || platform::watch(&worker));
        if started.is_err() {
            shared.publish(Preferences::default());
        }
        Self { shared }
    }

    /// The most recent reading.
    pub fn current(&self) -> Preferences {
        *self
            .shared
            .current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Waits at most `timeout` for the first reading, so a first paint can be
    /// right rather than corrected. Returns whether one arrived.
    pub fn wait_for_first_reading(&self, timeout: Duration) -> bool {
        let (ready, woken) = &self.shared.first_reading;
        let guard = ready
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (guard, _) = woken
            .wait_timeout_while(guard, timeout, |ready| !*ready)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use zbus::blocking::{Connection, Proxy};
    use zbus::zvariant::{OwnedValue, Value};

    use super::{ColorScheme, Preferences, Shared};

    const PORTAL: &str = "org.freedesktop.portal.Desktop";
    const PATH: &str = "/org/freedesktop/portal/desktop";
    const SETTINGS: &str = "org.freedesktop.portal.Settings";
    const APPEARANCE: &str = "org.freedesktop.appearance";
    const COLOR_SCHEME: &str = "color-scheme";
    const INTERFACE: &str = "org.gnome.desktop.interface";
    const ANIMATIONS: &str = "enable-animations";

    /// The portal's `color-scheme`: 0 for no preference, 1 for dark, 2 for
    /// light; anything else is the specification's "treat as no preference".
    pub(super) fn color_scheme(value: &Value<'_>) -> Option<ColorScheme> {
        match unwrapped(value) {
            Value::U32(1) => Some(ColorScheme::Dark),
            Value::U32(2) => Some(ColorScheme::Light),
            _ => None,
        }
    }

    /// GNOME's `enable-animations`, which is the inverse of reduced motion.
    pub(super) fn reduced_motion(value: &Value<'_>) -> Option<bool> {
        match unwrapped(value) {
            Value::Bool(enabled) => Some(!enabled),
            _ => None,
        }
    }

    /// The portal wraps a setting in a variant inside the reply's variant.
    fn unwrapped<'a>(value: &'a Value<'a>) -> &'a Value<'a> {
        let mut value = value;
        while let Value::Value(inner) = value {
            value = inner;
        }
        value
    }

    fn read(proxy: &Proxy<'_>, namespace: &str, key: &str) -> Option<OwnedValue> {
        proxy.call("Read", &(namespace, key)).ok()
    }

    pub(super) fn watch(shared: &Shared) {
        let Some((connection, proxy)) = Connection::session().ok().and_then(|connection| {
            let proxy = Proxy::new(&connection, PORTAL, PATH, SETTINGS).ok()?;
            Some((connection, proxy))
        }) else {
            shared.publish(Preferences::default());
            return;
        };
        let mut preferences = Preferences {
            color_scheme: read(&proxy, APPEARANCE, COLOR_SCHEME)
                .as_deref()
                .and_then(color_scheme),
            reduced_motion: read(&proxy, INTERFACE, ANIMATIONS)
                .as_deref()
                .and_then(reduced_motion),
        };
        shared.publish(preferences);
        let Ok(signals) = proxy.receive_signal("SettingChanged") else {
            return;
        };
        for message in signals {
            let Ok((namespace, key, value)) =
                message.body().deserialize::<(String, String, OwnedValue)>()
            else {
                continue;
            };
            match (namespace.as_str(), key.as_str()) {
                (APPEARANCE, COLOR_SCHEME) => preferences.color_scheme = color_scheme(&value),
                (INTERFACE, ANIMATIONS) => preferences.reduced_motion = reduced_motion(&value),
                _ => continue,
            }
            shared.publish(preferences);
        }
        drop(connection);
    }
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW,
    };

    use super::{POLL_INTERVAL, Preferences, Shared};

    /// Whether Windows has client-area animations switched off, which is the
    /// setting "Show animations in Windows" and the one a reduced-motion
    /// preference means there.
    fn reduced_motion() -> Option<bool> {
        let mut enabled: windows_sys::core::BOOL = 1;
        // SAFETY: `SPI_GETCLIENTAREAANIMATION` writes one `BOOL` through the
        // pointer, which is what is passed.
        let read = unsafe {
            SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&raw mut enabled).cast(), 0)
        };
        (read != 0).then_some(enabled == 0)
    }

    pub(super) fn watch(shared: &Shared) {
        loop {
            // The colour scheme is the window's: winit reports it and its
            // changes, so a second reading here could only disagree.
            shared.publish(Preferences {
                color_scheme: None,
                reduced_motion: reduced_motion(),
            });
            std::thread::sleep(POLL_INTERVAL);
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use objc2_app_kit::NSWorkspace;

    use super::{POLL_INTERVAL, Preferences, Shared};

    pub(super) fn watch(shared: &Shared) {
        loop {
            // The colour scheme is the window's: winit reports it and its
            // changes. Reduce Motion is an accessibility setting the workspace
            // answers from any thread.
            let reduced = NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion();
            shared.publish(Preferences {
                color_scheme: None,
                reduced_motion: Some(reduced),
            });
            std::thread::sleep(POLL_INTERVAL);
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod platform {
    use super::{Preferences, Shared};

    pub(super) fn watch(shared: &Shared) {
        shared.publish(Preferences::default());
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Condvar, Mutex};

    use super::*;

    #[test]
    fn a_reading_is_reported_once_and_then_only_when_it_changes() {
        let changes = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&changes);
        let shared = Shared {
            current: Mutex::new(Preferences::default()),
            first_reading: (Mutex::new(false), Condvar::new()),
            on_change: Box::new(move || {
                counted.fetch_add(1, Ordering::SeqCst);
            }),
        };
        // The first reading is reported even when it equals the default, so a
        // caller waiting for it is woken.
        shared.publish(Preferences::default());
        assert_eq!(changes.load(Ordering::SeqCst), 1);
        shared.publish(Preferences::default());
        assert_eq!(changes.load(Ordering::SeqCst), 1);
        let dark = Preferences {
            color_scheme: Some(ColorScheme::Dark),
            reduced_motion: Some(true),
        };
        shared.publish(dark);
        assert_eq!(changes.load(Ordering::SeqCst), 2);
        assert_eq!(
            *shared
                .current
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            dark
        );
    }

    #[test]
    fn the_watcher_answers_the_platform_or_the_documented_nothing() {
        let watcher = Watcher::start(|| {});
        watcher.wait_for_first_reading(Duration::from_secs(5));
        // Whatever the machine says is fine; what is asserted is that asking
        // did not hang and that a reading is a reading.
        let _ = watcher.current();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn portal_values_are_read_through_their_wrapping_variant() {
        use zbus::zvariant::Value;

        let wrapped = |inner: Value<'static>| Value::Value(Box::new(Value::Value(Box::new(inner))));
        assert_eq!(
            platform::color_scheme(&Value::U32(1)),
            Some(ColorScheme::Dark)
        );
        assert_eq!(
            platform::color_scheme(&wrapped(Value::U32(2))),
            Some(ColorScheme::Light)
        );
        assert_eq!(platform::color_scheme(&Value::U32(0)), None);
        assert_eq!(platform::color_scheme(&Value::Str("dark".into())), None);
        assert_eq!(
            platform::reduced_motion(&wrapped(Value::Bool(false))),
            Some(true)
        );
        assert_eq!(platform::reduced_motion(&Value::Bool(true)), Some(false));
        assert_eq!(platform::reduced_motion(&Value::U32(1)), None);
    }
}
