//! Android's NativeActivity entry point.
//!
//! Desktop platforms execute `blitsen-runtime`, which finds the application
//! appended to its own executable. Android instead loads this crate's `cdylib`
//! from the APK and calls [`android_main`] with an activity handle. Keeping the
//! entry artifact separate lets both paths converge on the same runtime session.
//!
//! # The session is shared; only the way in diverges
//!
//! [`blitsen_runtime::session::run_assets`] owns the engine, runtime services,
//! module registry, window session and frame loop. This crate supplies only the
//! platform-specific inputs that session cannot discover itself:
//!
//! - The activity has to be handed to the loop and notification bridge before the loop exists.
//!   `create_default_event_loop`'s Android branch reads it back out of a
//!   `OnceLock` and unwraps it, so `blitsen_host::native_window::set_android_app`
//!   must run before the session opens a window.
//! - Application files come from `AAssetManager`, exposed through
//!   [`blitsen_host::apk::ApkAssets`], rather than from an executable bundle.
//! - Android supplies no command line or useful stderr, so the session receives
//!   no overrides and failures are written through [`logcat`].
//!
//! # Verification boundary
//!
//! `android_main` may be re-entered in one process when the Activity is
//! recreated. The upstream activity slot is a `OnceLock`, so a second entry can
//! still panic. CI cross-compiles the default ABIs, verifies this symbol, builds
//! and inspects an APK, but does not boot that APK in an emulator or on a device.

pub mod logcat;

/// The symbol Android calls, and the whole of what is different about Android.
///
/// `android-activity`'s glue declares this in an `extern "Rust"` block and calls
/// it from the thread it starts for the application, inside a `catch_unwind` so
/// that a panic finishes the Activity rather than aborting the process. Returning
/// from it is how an application ends: the glue calls `ANativeActivity_finish`
/// on the way out. So the session's exit code has nowhere to go and is dropped,
/// which is the same thing that happens to it on the desktop when the loop ends
/// normally.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub fn android_main(app: android_activity::AndroidApp) {
    // First, and before anything can build an event loop: the loop's Android
    // branch reads the activity out of a `OnceLock` and unwraps it. The host
    // also retains a clone for its `android-activity`/JNI notification bridge.
    // Cloned because the asset manager is asked for below and `AndroidApp` is a
    // handle, not the activity itself.
    blitsen_host::native_window::set_android_app(app.clone());

    let assets = blitsen_host::apk::ApkAssets::open(
        app.asset_manager(),
        blitsen_host::apk::DEFAULT_ASSET_ROOT,
    );
    // Written before the window is attempted, so that a start which fails
    // somewhere with no message of its own still leaves a line saying what was
    // found in the APK — which is the difference between "the application is not
    // packaged" and "the application did not run".
    logcat::info(&format!("starting {assets:?}"));

    // No argv: an APK is launched by the system, and there is nothing to pass.
    if let Err(error) = blitsen_runtime::session::run_assets(assets, &[]) {
        logcat::error(&error);
    }
}
