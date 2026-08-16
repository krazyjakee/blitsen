//! Android's entry point: a shared object the platform loads, not a program it
//! runs (issue #142).
//!
//! Every other Blitsen target starts the same way. The system execs a file, the
//! C runtime calls `main`, `main` asks `current_exe()` where it is and reads the
//! application appended to itself. None of those three steps exists here.
//! Android starts an application by loading a `.so` out of the installed APK and
//! calling `android_main(app)` on a thread of its own; nothing is exec'd, so
//! there is no executable to name and no argv to read. The artifact is a
//! `cdylib`, and a package emits one artifact kind per target, which is why this
//! is a crate beside `blitsen-runtime` rather than a `cfg` inside it.
//!
//! # The session is shared; only the way in diverges
//!
//! That was the open question issue #142 carried, and the answer is that the
//! program is one program. `blitsen-runtime` grew a library target whose four
//! entry points are the four shapes an application arrives in — a directory, a
//! bundle appended to an executable, a dev server, and an APK's `assets/` — and
//! everything after them is the same code: the engine, the worker launcher, the
//! runtime services, the module registry, the window session, and the outer loop
//! that alternates a macrotask turn with the frame it may have dirtied. None of
//! that names a platform, and none of it had to change to be called from here.
//! `blitsen_runtime::session::run_assets` is what `android_main` calls, and it is
//! the sibling of what `main` calls.
//!
//! What is left over is this file, and it is only what the platform makes
//! different:
//!
//! - **The activity has to be handed to the loop before the loop exists.**
//!   `create_default_event_loop`'s Android branch reads it back out of a
//!   `OnceLock` and unwraps it, so `blitsen_host::native_window::set_android_app`
//!   has to run before the session opens a window. This is the one ordering
//!   obligation that could not be pushed down into the shared path: the activity
//!   is handed to `android_main` and reaches no other function.
//! - **The files are somewhere else.** `AAssetManager` rather than a byte range
//!   in the running executable. Issue #144 already did that work, so this is two
//!   calls: `ApkAssets::open` with the activity's asset manager, then the
//!   session.
//! - **There is no argv.** So there is no `--width`, no `--replay`, no
//!   `--bundle-report`. The window settings an export recorded are still read
//!   from `blitsen.runtime.json` inside the assets, because that half never came
//!   from the command line; only the overriding half has no way to speak. The
//!   reports that mattered moved rather than vanished — `--bundle-report` is
//!   `ApkAssets::report`, and `--licenses` is a file travelling inside the signed
//!   archive (#144).
//! - **There is nowhere to print.** No stderr a user can read and no exit code
//!   anything waited for, so a failure that is not written to logcat is a blank
//!   screen and nothing else. See [`logcat`].
//!
//! # What is deliberately not here
//!
//! `android_main` may be re-entered in one process when the Activity is
//! recreated — a rotation, a configuration change the manifest did not claim.
//! `set_android_app` is a `OnceLock::set(..).unwrap()` upstream, so a second
//! entry panics, and there is no query on the other side of that boundary to
//! guard it with. Fixing it means either an upstream API or holding the loop
//! across recreation, both of which are the Android lifecycle work, and neither
//! belongs to an entry point that has not yet been seen to start once. It is
//! issue #143's to hit and to decide.
//!
//! # The obstacles between here and a build, none of them this crate's
//!
//! A `cargo ndk -t arm64-v8a check -p blitsen-android` on this revision stops
//! three times before it reaches this file, and each stop belongs to someone
//! else:
//!
//! - `arboard` has no Android backend and fails to compile. Issue #139's
//!   obstacle (2), being decided in #147.
//! - `rfd` is depended on and its module is compiled under the predicate
//!   `all(unix, not(target_os = "macos"))`, which matches Android. #139's
//!   obstacle (1), spelled in four places that have to move together.
//! - `rquickjs-sys` ships pre-generated bindings for sixteen targets and no
//!   Android triple is among them, so its `include!` of
//!   `bindings/aarch64-linux-android.rs` fails and it says to use the `bindgen`
//!   feature instead. **This one is not in #139's table**, because that result
//!   was measured against `blitsen-host`, which links no JavaScript engine —
//!   this crate is the first thing to pull one in for Android. Turning on
//!   `rquickjs-sys/bindgen` under a `cfg(target_os = "android")` dependency does
//!   clear it and costs the other six targets nothing, but it makes `libclang`
//!   a requirement for cross-compiling, which is a toolchain decision belonging
//!   with the packaging work rather than something to land silently here.
//!
//! # What has and has not been established
//!
//! This crate has been type-checked for `aarch64-linux-android` with throwaway
//! scaffolding for all three obstacles above, and **never linked and never
//! run**. No APK has been built from it — that is issue #148 — and nothing here
//! has been on a device or an emulator, which is #143. Everything the module
//! docs above say about what Android does at run time is read off the platform
//! documentation and the `android-activity` source, not measured.

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
    // branch reads the activity out of a `OnceLock` and unwraps it. Cloned
    // because the asset manager is asked for below and `AndroidApp` is a handle,
    // not the activity itself.
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
