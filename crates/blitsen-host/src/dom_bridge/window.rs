//! The live application window, as `native:window` and `native:dialog` reach it.
//!
//! The window belongs to the winit session `pumpWindow` turns, and that same
//! call is what runs JavaScript: a `native:window` call arrives from inside
//! `pump_app_events`, with the session already borrowed by the pump that made
//! it. So it does not reach the session at all. It reaches this slot, which
//! holds the same `Arc<dyn Window>` the session's view holds — published when
//! the surface is created and dropped when the window closes.
//!
//! Thread-local because the window is only safe to drive from the thread that
//! owns the event loop, which is the thread that published it and the thread
//! JavaScript runs on. The current session has exactly one. Issue #105 decides
//! that future windows keep isolated JavaScript contexts on this same thread;
//! before `create` can ship, this singleton must become a calling-context to
//! window mapping rather than exposing another context's window here.
//!
//! Nothing here reports the window's size or scale factor. `innerWidth`,
//! `innerHeight` and `devicePixelRatio` already do, and the `resize` event
//! already says when they changed; a second answer that could disagree with
//! them would be worse than no answer. What is genuinely new is the monitors —
//! including the scale factor of the ones the window is not on.

use std::cell::{Cell, RefCell};
use std::sync::Arc;

use winit::dpi::PhysicalSize;
use winit::window::Window;

// Everything a `native:window` or `native:dialog` call reaches is compiled off
// Android; see the note above `with`. What is left is the slot itself, which the
// session publishes into and reads back whatever the platform.
#[cfg(not(target_os = "android"))]
use blitsen_js::JsError;
#[cfg(not(target_os = "android"))]
use serde_json::{Value, json};
#[cfg(not(target_os = "android"))]
use std::str::FromStr;
#[cfg(not(target_os = "android"))]
use winit::cursor::CursorIcon;
#[cfg(not(target_os = "android"))]
use winit::dpi::LogicalSize;
#[cfg(not(target_os = "android"))]
use winit::monitor::{Fullscreen, MonitorHandle};
#[cfg(not(target_os = "android"))]
use winit::window::{CursorGrabMode, WindowLevel};

thread_local! {
    static CURRENT: RefCell<Option<Arc<dyn Window>>> = const { RefCell::new(None) };
    /// A resize winit applied outright instead of reporting as an event.
    static APPLIED_RESIZE: Cell<Option<PhysicalSize<u32>>> = const { Cell::new(None) };
    /// A close requested by application-drawn window chrome.
    ///
    /// JavaScript runs while the winit session is already borrowed, so the
    /// bridge cannot remove the window in-place. The session consumes this at
    /// the end of the same pump turn instead.
    static CLOSE_REQUESTED: Cell<bool> = const { Cell::new(false) };
    /// Modes entered through the standard DOM APIs, kept separately from the
    /// native window module so lifecycle loss only undoes modes it owns.
    static WEB_POINTER_LOCKED: Cell<bool> = const { Cell::new(false) };
    static WEB_FULLSCREEN: Cell<bool> = const { Cell::new(false) };
    /// Desired state owned by `native:window`. DOM modes temporarily override
    /// these values and restore the latest desired state on exit.
    #[cfg(not(target_os = "android"))]
    static NATIVE_CURSOR_GRAB: Cell<CursorGrabMode> = const { Cell::new(CursorGrabMode::None) };
    static NATIVE_CURSOR_VISIBLE: Cell<bool> = const { Cell::new(true) };
    static NATIVE_FULLSCREEN: Cell<bool> = const { Cell::new(false) };
}

/// Publishes the window `native:` calls act on, or `None` once it has gone.
pub(crate) fn publish(window: Option<Arc<dyn Window>>) {
    #[cfg(not(target_os = "android"))]
    if CURRENT.with_borrow(|current| current.is_none())
        && let Some(window) = window.as_deref()
    {
        NATIVE_CURSOR_GRAB.set(CursorGrabMode::None);
        NATIVE_CURSOR_VISIBLE.set(true);
        NATIVE_FULLSCREEN.set(window.fullscreen().is_some());
    }
    CURRENT.with_borrow_mut(|current| *current = window);
    APPLIED_RESIZE.set(None);
    CLOSE_REQUESTED.set(false);
    if CURRENT.with_borrow(|current| current.is_none()) {
        WEB_POINTER_LOCKED.set(false);
        WEB_FULLSCREEN.set(false);
    }
}

/// Applies one standard web window-mode command.
#[cfg(not(target_os = "android"))]
pub(crate) fn web_mode(action: &str) -> Result<(), JsError> {
    match action {
        "lockPointer" => with(|window| {
            window
                .set_cursor_grab(CursorGrabMode::Locked)
                .map_err(|error| JsError::new(format!("could not lock the pointer: {error}")))?;
            window.set_cursor_visible(false);
            WEB_POINTER_LOCKED.set(true);
            Ok(())
        }),
        "unlockPointer" => {
            let released = with(|window| {
                let result = window
                    .set_cursor_grab(NATIVE_CURSOR_GRAB.get())
                    .map_err(|error| {
                        JsError::new(format!("could not release the pointer: {error}"))
                    });
                window.set_cursor_visible(NATIVE_CURSOR_VISIBLE.get());
                result
            });
            // Stop raw routing even if the compositor refused the restoration;
            // an explicit exit has ended the DOM lock either way.
            WEB_POINTER_LOCKED.set(false);
            released
        }
        "enterFullscreen" => with(|window| {
            // The web API has no resolution/refresh-rate selector. Choosing an
            // exclusive video mode here would therefore be arbitrary and can
            // reconfigure the display. Use the monitor containing this window,
            // with winit's primary/default fallback when it cannot identify one.
            let monitor = window
                .current_monitor()
                .or_else(|| window.primary_monitor());
            window.set_fullscreen(Some(Fullscreen::Borderless(monitor)));
            WEB_FULLSCREEN.set(true);
            Ok(())
        }),
        "exitFullscreen" => with(|window| {
            window.set_fullscreen(
                NATIVE_FULLSCREEN
                    .get()
                    .then_some(Fullscreen::Borderless(None)),
            );
            WEB_FULLSCREEN.set(false);
            Ok(())
        }),
        other => Err(JsError::new(format!(
            "unknown web window mode action: {other}"
        ))),
    }
}

#[cfg(target_os = "android")]
pub(crate) fn web_mode(_action: &str) -> Result<(), blitsen_js::JsError> {
    Err(blitsen_js::JsError::new(
        "pointer lock and the standard fullscreen API are not supported on Android",
    ))
}

/// Whether raw device motion should be routed to the locked DOM element.
pub(crate) fn web_pointer_locked() -> bool {
    WEB_POINTER_LOCKED.get()
}

#[cfg(not(target_os = "android"))]
fn native_fullscreen_requested(on: bool) -> bool {
    NATIVE_FULLSCREEN.set(on);
    !WEB_FULLSCREEN.get()
}

#[cfg(not(target_os = "android"))]
fn native_cursor_visibility_requested(on: bool) -> bool {
    NATIVE_CURSOR_VISIBLE.set(on);
    !WEB_POINTER_LOCKED.get()
}

#[cfg(not(target_os = "android"))]
fn native_cursor_grab_requested(mode: CursorGrabMode) -> bool {
    NATIVE_CURSOR_GRAB.set(mode);
    !WEB_POINTER_LOCKED.get()
}

/// Releases modes owned by the web APIs, returning which DOM states changed.
///
/// The flags are cleared even if a platform release reports an error: focus or
/// surface loss is a security boundary and raw movement must stop immediately.
pub(crate) fn release_web_modes() -> (bool, bool) {
    let pointer = WEB_POINTER_LOCKED.replace(false);
    let fullscreen = WEB_FULLSCREEN.replace(false);
    #[cfg(not(target_os = "android"))]
    CURRENT.with_borrow(|current| {
        if let Some(window) = current.as_deref() {
            if pointer {
                let _ = window.set_cursor_grab(NATIVE_CURSOR_GRAB.get());
                window.set_cursor_visible(NATIVE_CURSOR_VISIBLE.get());
            }
            if fullscreen {
                window.set_fullscreen(
                    NATIVE_FULLSCREEN
                        .get()
                        .then_some(Fullscreen::Borderless(None)),
                );
            }
        }
    });
    (pointer, fullscreen)
}

/// Takes the size winit resized the surface to without raising an event.
///
/// Wayland applies a requested size immediately and sends nothing back; X11
/// asks the server and the answer arrives as `SurfaceResized`. The session
/// feeds this one through the same path so the viewport, `innerWidth` and the
/// `resize` event cannot be left describing the size before last.
pub(crate) fn take_applied_resize() -> Option<PhysicalSize<u32>> {
    APPLIED_RESIZE.take()
}

/// Takes an application-drawn close request after JavaScript yields back to
/// the event-loop pump that owns the session.
pub(crate) fn take_close_requested() -> bool {
    CLOSE_REQUESTED.take()
}

/// Runs `operation` against the window, or reports that there is not one yet.
///
/// This half of the module is compiled off Android, because winit's window there
/// answers every question below and none of the answers are true. Read from its
/// source at the pinned version rather than assumed: `set_fullscreen` logs
/// "Cannot set fullscreen on Android" and returns, while `fullscreen()` answers
/// `None` — so `isFullscreen()` reports false on a surface that fills the
/// screen. `set_decorations` is empty and `is_decorated()` returns `true`, so
/// `setDecorations(false)` is followed by `isDecorated()` saying it is decorated,
/// on a platform with no decorations to remove. `request_surface_size` ignores
/// the size and hands back the current one, which this module would record as a
/// resize that was applied. `set_window_level`, `set_cursor` and
/// `set_cursor_visible` are all empty bodies, and the cursor is not a thing the
/// device has.
///
/// The monitors go with them, and that one is worth naming because it looks like
/// the survivor: `available_monitors()` on Android is `iter::empty()` and both
/// `primary_monitor()` and `current_monitor()` are `None`. So `monitors()` would
/// return `[]` — an application reading that learns the device has no display,
/// which is a wrong answer rather than a missing one. The scale factor is
/// reachable there, but it is already `devicePixelRatio` and is not this
/// module's to spell a second time.
///
/// So `native:window` is absent whole on Android rather than trimmed. The
/// capabilities that *are* real — immersive mode, orientation, the cutout inset
/// — are not these under another name (#146, #147).
#[cfg(not(target_os = "android"))]
pub(super) fn with<T>(
    operation: impl FnOnce(&dyn Window) -> Result<T, JsError>,
) -> Result<T, JsError> {
    CURRENT.with_borrow(|current| match current.as_deref() {
        Some(window) => operation(window),
        // Scripts run before the window is created, so this is a real state an
        // application can be in rather than a broken build.
        None => Err(JsError::new(
            "there is no application window yet: native:window and native:dialog are usable \
             from the load event onwards, in an application run by blitsen",
        )),
    })
}

/// A property of the window, already parsed and range-checked.
#[cfg(not(target_os = "android"))]
enum Setting {
    Fullscreen(bool),
    Decorations(bool),
    Minimized(bool),
    Maximized(bool),
    AlwaysOnTop(bool),
    Cursor(CursorIcon),
    CursorVisible(bool),
    CursorGrab(CursorGrabMode),
}

#[cfg(not(target_os = "android"))]
fn boolean(property: &str, value: &str) -> Result<bool, JsError> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(JsError::new(format!(
            "{property} is a boolean, not {other:?}"
        ))),
    }
}

/// Parses a property before anything is asked of the window, so a mistyped
/// value is a rejection rather than a window left half-configured.
#[cfg(not(target_os = "android"))]
fn setting(property: &str, value: &str) -> Result<Setting, JsError> {
    Ok(match property {
        "fullscreen" => Setting::Fullscreen(boolean(property, value)?),
        "decorations" => Setting::Decorations(boolean(property, value)?),
        "minimized" => Setting::Minimized(boolean(property, value)?),
        "maximized" => Setting::Maximized(boolean(property, value)?),
        "alwaysOnTop" => Setting::AlwaysOnTop(boolean(property, value)?),
        "cursorVisible" => Setting::CursorVisible(boolean(property, value)?),
        "cursor" => Setting::Cursor(
            CursorIcon::from_str(value)
                .map_err(|_| JsError::new(format!("{value:?} is not a CSS cursor keyword")))?,
        ),
        "cursorGrab" => Setting::CursorGrab(match value {
            "none" => CursorGrabMode::None,
            "confined" => CursorGrabMode::Confined,
            "locked" => CursorGrabMode::Locked,
            other => {
                return Err(JsError::new(format!(
                    "{other:?} is not a cursor grab mode: none, confined or locked"
                )));
            }
        }),
        other => return Err(JsError::new(format!("unknown window property: {other}"))),
    })
}

/// Applies `property`.
#[cfg(not(target_os = "android"))]
pub(super) fn set(property: &str, value: &str) -> Result<(), JsError> {
    let setting = setting(property, value)?;
    with(|window| {
        match setting {
            Setting::Fullscreen(on) => {
                if native_fullscreen_requested(on) {
                    window.set_fullscreen(on.then_some(Fullscreen::Borderless(None)));
                }
            }
            Setting::Decorations(on) => window.set_decorations(on),
            Setting::Minimized(on) => window.set_minimized(on),
            Setting::Maximized(on) => window.set_maximized(on),
            Setting::AlwaysOnTop(on) => window.set_window_level(if on {
                WindowLevel::AlwaysOnTop
            } else {
                WindowLevel::Normal
            }),
            Setting::Cursor(icon) => window.set_cursor(icon.into()),
            Setting::CursorVisible(on) => {
                if native_cursor_visibility_requested(on) {
                    window.set_cursor_visible(on);
                }
            }
            Setting::CursorGrab(mode) => {
                let previous = NATIVE_CURSOR_GRAB.get();
                if native_cursor_grab_requested(mode)
                    && let Err(error) = window.set_cursor_grab(mode)
                {
                    NATIVE_CURSOR_GRAB.set(previous);
                    return Err(JsError::new(format!("could not grab the cursor: {error}")));
                }
            }
        }
        Ok(())
    })
}

/// Reads back a property winit can be asked for.
#[cfg(not(target_os = "android"))]
pub(super) fn get(property: &str) -> Result<bool, JsError> {
    match property {
        "fullscreen" | "decorations" | "maximized" => {}
        other => return Err(JsError::new(format!("unknown window property: {other}"))),
    }
    with(|window| {
        Ok(match property {
            "fullscreen" => window.fullscreen().is_some(),
            "decorations" => window.is_decorated(),
            _ => window.is_maximized(),
        })
    })
}

/// Runs an action that is not a persistent window property.
#[cfg(not(target_os = "android"))]
pub(super) fn command(command: &str) -> Result<(), JsError> {
    match command {
        "startDrag" => with(|window| {
            window
                .drag_window()
                .map_err(|error| JsError::new(format!("could not drag the window: {error}")))
        }),
        "close" => {
            // Resolve the live window first so a call made before `load` still
            // gets the same useful error as every other member.
            with(|_| Ok(()))?;
            CLOSE_REQUESTED.set(true);
            Ok(())
        }
        other => Err(JsError::new(format!("unknown window command: {other}"))),
    }
}

/// Asks for a new surface size, in CSS pixels.
#[cfg(not(target_os = "android"))]
pub(super) fn resize(width: f64, height: f64) -> Result<(), JsError> {
    let valid = |value: f64| value.is_finite() && value >= 1.0 && value <= f64::from(u32::MAX);
    if !valid(width) || !valid(height) {
        return Err(JsError::new(format!(
            "a window is at least 1x1 CSS pixels, not {width}x{height}"
        )));
    }
    with(|window| {
        if let Some(applied) = window.request_surface_size(LogicalSize::new(width, height).into()) {
            APPLIED_RESIZE.set(Some(applied));
        }
        Ok(())
    })
}

/// Enumerates the monitors, each with its own scale factor.
#[cfg(not(target_os = "android"))]
pub(super) fn monitors() -> Result<Value, JsError> {
    with(|window| {
        let current = window.current_monitor().map(|monitor| monitor.id());
        let primary = window.primary_monitor().map(|monitor| monitor.id());
        Ok(Value::Array(
            window
                .available_monitors()
                .map(|monitor| {
                    let id = Some(monitor.id());
                    describe(&monitor, id == current, id == primary)
                })
                .collect(),
        ))
    })
}

#[cfg(not(target_os = "android"))]
fn describe(monitor: &MonitorHandle, current: bool, primary: bool) -> Value {
    let mode = monitor.current_video_mode();
    let size = mode.as_ref().map(|mode| mode.size());
    let position = monitor.position();
    json!({
        "name": monitor.name().map(|name| name.into_owned()),
        "x": position.map(|position| position.x),
        "y": position.map(|position| position.y),
        "width": size.map(|size| size.width),
        "height": size.map(|size| size.height),
        "scaleFactor": monitor.scale_factor(),
        "refreshRate": mode
            .and_then(|mode| mode.refresh_rate_millihertz())
            .map(|rate| f64::from(rate.get()) / 1000.0),
        "current": current,
        "primary": primary,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every argument is checked before the window is looked for, so a headless
    /// build reports the mistake rather than the absence.
    #[test]
    fn a_bad_value_is_rejected_without_a_window() {
        for (property, value) in [
            ("fullscreen", "yes"),
            ("cursor", "wiggly"),
            ("cursorGrab", "everything"),
            ("elevation", "true"),
        ] {
            let error = setting(property, value)
                .err()
                .unwrap_or_else(|| panic!("{property}={value} must be refused"));
            assert!(!error.message().contains("no application window"));
        }
        assert!(matches!(
            setting("cursor", "not-allowed"),
            Ok(Setting::Cursor(CursorIcon::NotAllowed))
        ));
        assert!(matches!(
            setting("fullscreen", "true"),
            Ok(Setting::Fullscreen(true))
        ));
    }

    #[test]
    fn a_window_smaller_than_a_pixel_is_refused() {
        for (width, height) in [(0.0, 100.0), (100.0, f64::NAN), (-4.0, -4.0)] {
            assert!(resize(width, height).is_err());
        }
    }

    /// Without a window every operation says so, rather than doing nothing.
    #[test]
    fn operations_need_a_window() {
        for error in [
            set("fullscreen", "true").unwrap_err(),
            get("fullscreen").unwrap_err(),
            resize(640.0, 480.0).unwrap_err(),
            monitors().unwrap_err(),
        ] {
            assert!(error.message().contains("no application window"));
        }
    }

    #[test]
    fn dom_modes_preserve_and_arbitrate_native_window_state() {
        // A mode that was native before the DOM request remains the baseline.
        NATIVE_FULLSCREEN.set(true);
        NATIVE_CURSOR_GRAB.set(CursorGrabMode::Confined);
        NATIVE_CURSOR_VISIBLE.set(false);
        WEB_FULLSCREEN.set(true);
        WEB_POINTER_LOCKED.set(true);
        assert!(NATIVE_FULLSCREEN.get());
        assert_eq!(NATIVE_CURSOR_GRAB.get(), CursorGrabMode::Confined);
        assert!(!NATIVE_CURSOR_VISIBLE.get());

        // Native calls made during a DOM-owned mode update the state to restore
        // but are not applied over the active DOM mode.
        assert!(!native_fullscreen_requested(false));
        assert!(!native_cursor_grab_requested(CursorGrabMode::Locked));
        assert!(!native_cursor_visibility_requested(true));
        assert!(!NATIVE_FULLSCREEN.get());
        assert_eq!(NATIVE_CURSOR_GRAB.get(), CursorGrabMode::Locked);
        assert!(NATIVE_CURSOR_VISIBLE.get());

        // Reload uses this same release path. Even without a live test window,
        // ownership is cleared and the desired native baseline is retained for
        // the real-window restoration performed inside the function.
        assert_eq!(release_web_modes(), (true, true));
        assert!(!WEB_FULLSCREEN.get());
        assert!(!WEB_POINTER_LOCKED.get());
        assert!(!NATIVE_FULLSCREEN.get());
        assert_eq!(NATIVE_CURSOR_GRAB.get(), CursorGrabMode::Locked);
        assert!(NATIVE_CURSOR_VISIBLE.get());

        // Leave process-thread state at its default for later unit tests.
        NATIVE_CURSOR_GRAB.set(CursorGrabMode::None);
        NATIVE_CURSOR_VISIBLE.set(true);
    }
}
