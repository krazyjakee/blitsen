//! Following the system's colour-scheme and motion preferences into the
//! document (#385).
//!
//! Two sources feed one answer. The platform watcher reads what the desktop
//! publishes — the settings portal on Linux, the accessibility and animation
//! settings on macOS and Windows — and winit reports the window's theme where
//! the platform has one. The platform's colour scheme wins where it answers,
//! because on Linux it is the only source; elsewhere it answers `None` and the
//! window's theme stands. Where nothing answers, the documented fallback does:
//! light, and no motion preference.
//!
//! The answer is applied to the document on the frame thread only, from the
//! event-loop callbacks, and then read by the bootstrap at the top of the next
//! frame turn — which is where a `MediaQueryList` learns it changed. A change
//! therefore reaches CSS and JavaScript on the same frame and never part-way
//! through one.

use blitsen_dom::{ColorScheme, DomBackend, MediaPreferences};
use blitsen_js::JsEngine;
use winit::event_loop::EventLoopProxy;
use winit::window::Theme;

use super::WindowApplication;

/// How long the first paint may wait for the platform's first reading.
///
/// A healthy portal answers in milliseconds; a missing one fails at once. The
/// bound is for the one in between, and a window that opens without the
/// reading is corrected on its first frame rather than never.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
const FIRST_READING: std::time::Duration = std::time::Duration::from_millis(250);

/// What the session knows about the preferences, and what it last applied.
pub(crate) struct Appearance {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    watcher: blitsen_platform::appearance::Watcher,
    /// The theme winit last reported for the window, once it has.
    theme: Option<Theme>,
    /// What the document was last told, so an unchanged answer costs nothing.
    applied: Option<MediaPreferences>,
}

impl Appearance {
    /// Starts following the platform, waking the event loop on a change.
    pub(crate) fn start(proxy: EventLoopProxy) -> Self {
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        let watcher = {
            let watcher = blitsen_platform::appearance::Watcher::start(move || proxy.wake_up());
            watcher.wait_for_first_reading(FIRST_READING);
            watcher
        };
        #[cfg(any(target_os = "android", target_os = "ios"))]
        drop(proxy);
        Self {
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            watcher,
            theme: None,
            applied: None,
        }
    }

    /// Records the theme winit reported.
    pub(crate) fn set_theme(&mut self, theme: Theme) {
        self.theme = Some(theme);
    }

    /// Forgets what was applied, for a document that has been replaced and
    /// so starts from the fallbacks again.
    pub(crate) fn forget_applied(&mut self) {
        self.applied = None;
    }

    /// The preferences the document should report right now.
    fn preferences(&self) -> MediaPreferences {
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        let platform = self.watcher.current();
        #[cfg(any(target_os = "android", target_os = "ios"))]
        let platform = blitsen_platform_fallback();
        let color_scheme = platform
            .color_scheme
            .map(|scheme| match scheme {
                blitsen_platform::appearance::ColorScheme::Light => ColorScheme::Light,
                blitsen_platform::appearance::ColorScheme::Dark => ColorScheme::Dark,
            })
            .or_else(|| {
                self.theme.map(|theme| match theme {
                    Theme::Light => ColorScheme::Light,
                    Theme::Dark => ColorScheme::Dark,
                })
            })
            .unwrap_or_default();
        MediaPreferences {
            color_scheme,
            reduced_motion: platform.reduced_motion.unwrap_or(false),
        }
    }
}

/// The platform reading on a target that has no watcher: nothing, so the
/// window's theme and the documented fallbacks answer.
#[cfg(any(target_os = "android", target_os = "ios"))]
fn blitsen_platform_fallback() -> blitsen_platform::appearance::Preferences {
    blitsen_platform::appearance::Preferences::default()
}

impl<Rend: anyrender::WindowRenderer, E: JsEngine + Clone> WindowApplication<Rend, E> {
    /// Applies the current preferences to the document if they changed.
    ///
    /// Called from the event-loop callbacks — after the window exists, after
    /// winit reports a theme change, and after the watcher wakes the loop —
    /// so the document is written on the frame thread and nowhere else. A
    /// change asks for a frame, which is the one that delivers it.
    pub(crate) fn sync_appearance(&mut self) {
        if self.appearance.theme.is_none()
            && let Some(theme) = self
                .inner
                .windows
                .values()
                .find_map(|view| view.window.theme())
        {
            self.appearance.theme = Some(theme);
        }
        let preferences = self.appearance.preferences();
        if self.appearance.applied == Some(preferences) {
            return;
        }
        self.appearance.applied = Some(preferences);
        if let Err(error) = self
            .document
            .borrow_mut()
            .set_media_preferences(preferences)
        {
            self.park_error(crate::dom_error(error));
            return;
        }
        for view in self.inner.windows.values() {
            view.window.request_redraw();
        }
    }
}
