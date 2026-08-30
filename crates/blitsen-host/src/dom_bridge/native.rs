//! Host half of the `blitsen/*` modules, below the namespace the bootstrap builds.
//!
//! Every function here is installed under a `__blitsenNative…` name and only if
//! this platform can implement it properly. That is what makes the namespace
//! honest: the bootstrap drops any member whose host function is missing, so a
//! capability this build does not have reads as `undefined` and feature
//! detection selects a fallback (COMPATIBILITY.md, "Capability tiers").
//!
//! Android is where that sentence stops being a formality, because the platform
//! answers "no" to most of it. `os`, focused `input` snapshots, notifications
//! and — since #248 gave it a `UsbManager` backend — raw HID survive there; app,
//! clipboard, dialog, window and tray remain absent for the reasons their module
//! or platform documentation states.

mod app;
mod clipboard;
mod dialog;
mod hid;
mod input;
mod menu;
mod notify;
mod os;
mod tray;
mod window;

use blitsen_js::{JsEngine, JsError};
#[cfg(not(target_os = "android"))]
use blitsen_platform::PlatformError;

#[cfg(not(target_os = "android"))]
fn failed(error: PlatformError) -> JsError {
    JsError::new(error.message().to_owned())
}

/// Installs the host functions the native namespace is assembled from.
pub(super) fn install<E: JsEngine + 'static>(engine: &mut E) -> Result<(), JsError> {
    app::install(engine)?;
    clipboard::install(engine)?;
    window::install(engine)?;
    tray::install(engine)?;
    menu::install(engine)?;
    hid::install(engine)?;
    notify::install(engine)?;
    input::install(engine)?;
    os::install(engine)?;
    dialog::install(engine)
}
