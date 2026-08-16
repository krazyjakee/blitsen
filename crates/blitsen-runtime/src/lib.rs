//! The runtime as a library, so two artifacts can be one program.
//!
//! This package ships a `[[bin]]` — `fn main`, an argv, `current_exe`, an exit
//! code — and that is the whole of Blitsen on the six desktop targets. Android
//! has none of those things: the platform loads a `.so` out of the APK and calls
//! `android_main(app)`, and there is no executable to point at. The artifact
//! kind differs, so the Android entry point is a `cdylib` in a sibling crate
//! (`blitsen-android`, issue #142) rather than a `cfg` inside `main.rs`.
//!
//! Two artifacts is not two programs. This library is the program; `main.rs`
//! and `android_main` are two ways of arriving at it.
//!
//! # What the entry points share
//!
//! [`session`] holds everything between "here are the application's files" and
//! "the window closed": loading the engine, registering the launcher a `Worker`
//! starts a thread through, installing the runtime services and the module
//! registry, opening a [`blitsen_host::WindowSession`], and the outer loop that
//! alternates a macrotask turn with the frame it may have dirtied. Nothing in
//! any of that names a platform, and nothing in it had to change to be reached
//! from a second entry point — which is the answer to the question issue #142
//! actually asks. The four ways an application arrives are four sibling
//! functions ([`session::run_directory`], [`session::run_bundle`],
//! [`session::run_url`], [`session::run_assets`]), one per [`AppFiles`] shape,
//! and each of them ends in the same private `run`.
//!
//! [`AppFiles`]: blitsen_host::app::AppFiles
//!
//! # What stays behind in `main.rs`, and why
//!
//! Three things did not come across, and each is genuinely a console program's
//! rather than an application's:
//!
//! - **Argument parsing.** `--width`, `--replay`, a directory, a dev-server URL.
//!   An APK is launched by the system with no argv, so on Android these are not
//!   defaulted — they are untypeable. [`session::run_assets`] takes the same
//!   `&[String]` its siblings take and Android passes an empty one, so the
//!   window settings an export recorded are still read from
//!   `blitsen.runtime.json`; only the overriding half has no way to speak.
//! - **The reports.** `--bundle-report`, `--engine-report`, `--licenses` and
//!   `--replay` all answer a question someone typed. Issue #144 moved the two
//!   that an Android artifact still owes: `--bundle-report` becomes
//!   [`blitsen_host::apk::ApkAssets::report`], and `--licenses` becomes the
//!   notices file travelling inside the signed archive.
//! - **The process-global memory defaults.** They are `cfg(target_os =
//!   "linux")`, which does not match Android, so they compile away there. That
//!   is the right outcome but not for a transferable reason: the reasoning
//!   behind them is about a developer workstation with several Vulkan ICDs
//!   installed, and a phone has one driver and no `/sys/class/drm` to read it
//!   from. Sharing them would have been sharing a decision that was never made
//!   about this platform.
//!
//! One thing had to be added rather than shared, and it is the ordering
//! constraint: `blitz_shell::set_android_app` has to be called before the event
//! loop is built, because the loop's Android branch reads the activity back out
//! of a `OnceLock` and unwraps it. That call cannot live here — it needs an
//! `AndroidApp`, which only the entry point is handed — so
//! `blitsen_host::native_window::set_android_app` states the constraint beside
//! the code that builds the loop, and `blitsen-android` satisfies it.

pub mod engine;
pub mod loop_pacing;
pub mod session;
