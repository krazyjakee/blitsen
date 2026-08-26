//! A surface destroyed and recreated under a live document (issue #146).
//!
//! # What this establishes
//!
//! The handlers in `blitsen_host::surface_lifecycle` model Android's lifecycle.
//! CI builds and inspects an APK, but does not run its lifecycle on a device or
//! emulator, so the cycle is driven synthetically instead —
//! [`WindowSession::lose_surface`] and `restore_surface` queue the *real*
//! handlers, which run from `about_to_wait` with the *real* `ActiveEventLoop`,
//! against a *real* winit window. `View::suspend` drops the wgpu surface, the
//! swapchain and the `vello::Renderer`; `View::resume` builds all three again.
//! Nothing about the teardown is faked: `renderer_is_active()` reads the
//! renderer's own state machine, not Blitsen's bookkeeping about it.
//!
//! # What this does not establish
//!
//! * **Not that Android works.** The synthetic events are the ones winit's
//!   Android backend calls — that mapping is read out of `winit-android`
//!   0.31.0-beta.2 — and their order follows Android's documented Activity
//!   lifecycle, which is inference rather than a reading. And a device also
//!   changes the surface size, the DPI, the safe-area insets and the GPU's
//!   mood, none of which is here. The procedure this cannot replace is at the
//!   bottom of the file.
//! * **Not that rotation works.** With `configChanges` declared (see the
//!   `surface_lifecycle` module comment) rotation is not a surface loss at all;
//!   it is a resize, which the desktop window-drag path already covers.
//! * **Not a memory measurement.** The leak assertion counts references to the
//!   window handle, which is exact; it says nothing about wgpu's own pools.
//!
//! Needs a display: the whole point is a real surface. Skipped, loudly, without
//! one — a silent skip is how a test that measures nothing looks from CI.
//!
//! Runs without libtest (`harness = false` in `Cargo.toml`) because winit
//! refuses to build an event loop off the main thread and libtest runs every
//! test on one it spawned.

use std::path::Path;
use std::time::Duration;

use blitsen_host::app::AppFiles;
use blitsen_host::runtime_services::RuntimeServices;
use blitsen_host::surface_lifecycle::SurfaceState;
use blitsen_host::{OpenDirectoryOptions, WindowSession};
use blitsen_js::JsEngine;
use blitsen_quickjs::QuickJs;

/// An application that animates, keeps a timer running, and holds DOM state.
///
/// All three are read after the cycle, so all three have to be things a cycle
/// could plausibly break: a `requestAnimationFrame` chain, a self-rearming
/// `setTimeout`, and a node the script mutated before the surface went away.
const DOCUMENT: &str = r#"<!doctype html>
<html><head><title>surface lifecycle</title></head>
<body>
<p id="marker">untouched</p>
<script>
globalThis.probe = { frames: 0, timers: 0, resizes: 0 };
document.getElementById("marker").textContent = "set by script";
const frame = () => { globalThis.probe.frames += 1; requestAnimationFrame(frame); };
requestAnimationFrame(frame);
const tick = () => { globalThis.probe.timers += 1; setTimeout(tick, 1); };
setTimeout(tick, 1);
addEventListener("resize", () => { globalThis.probe.resizes += 1; });
</script>
</body></html>
"#;

/// Advances the session the way `blitsen-runtime`'s loop does, `turns` times.
fn pump(
    session: &mut WindowSession<QuickJs>,
    services: &RuntimeServices<QuickJs>,
    engine: &mut QuickJs,
    turns: usize,
) {
    for _ in 0..turns {
        services.run_expired_timers(engine).expect("timers run");
        session
            .pump_for(Some(Duration::from_millis(2)))
            .expect("the window pumps");
    }
}

fn probe(engine: &mut QuickJs, name: &str) -> f64 {
    let value = engine
        .evaluate_script(
            &format!("globalThis.probe.{name}"),
            "blitsen:surface-lifecycle-probe",
        )
        .expect("the probe reads");
    engine.to_number(&value).expect("the probe is a number")
}

fn marker(engine: &mut QuickJs) -> String {
    let value = engine
        .evaluate_script(
            "document.getElementById('marker').textContent",
            "blitsen:surface-lifecycle-marker",
        )
        .expect("the marker reads");
    engine.to_string(&value).expect("the marker is a string")
}

fn main() {
    // GitHub's Windows runner has no interactive desktop/GPU session. Winit can
    // create a window there, but wgpu faults in the process while creating or
    // restoring its surface (STATUS_ACCESS_VIOLATION), before Rust can report
    // an error. Keep exercising Windows on real desktop sessions while making
    // the hosted runner honest about the coverage it cannot provide.
    if cfg!(target_os = "windows") && std::env::var_os("GITHUB_ACTIONS").is_some() {
        eprintln!(
            "SKIPPED surface_lifecycle: GitHub's Windows runner has no interactive GPU session"
        );
        return;
    }

    // macOS and Windows create windows natively without a DISPLAY variable, so
    // the environment check only means anything on the platforms that use one.
    let has_display = cfg!(any(target_os = "macos", target_os = "windows"))
        || std::env::var_os("DISPLAY").is_some()
        || std::env::var_os("WAYLAND_DISPLAY").is_some();
    if !has_display {
        // A CI job that arranged a display sets this to assert the test really
        // ran, so an arrangement that quietly broke fails instead of skipping.
        if std::env::var_os("BLITSEN_REQUIRE_DISPLAY").is_some_and(|value| value == "1") {
            panic!(
                "BLITSEN_REQUIRE_DISPLAY=1, but there is no DISPLAY or \
                 WAYLAND_DISPLAY: this environment promised a display and \
                 did not provide one"
            );
        }
        eprintln!(
            "SKIPPED surface_lifecycle: a surface cycle needs a real window, and \
             there is no DISPLAY or WAYLAND_DISPLAY. This test measures nothing \
             in this environment; run it on a desktop session."
        );
        return;
    }

    let directory = tempfile::tempdir().expect("a fixture directory");
    let entrypoint = directory.path().join("index.html");
    std::fs::write(&entrypoint, DOCUMENT).expect("the fixture writes");
    run_cycle(&entrypoint);
    println!("surface cycle verified: ten destroy/recreate rounds, no leak");
}

fn run_cycle(entrypoint: &Path) {
    let mut engine = QuickJs::new().expect("an engine");
    let services = RuntimeServices::install(&mut engine).expect("the services install");
    let files = AppFiles::directory(entrypoint).expect("the fixture opens");
    let options = OpenDirectoryOptions {
        root: entrypoint
            .parent()
            .expect("the fixture has a directory")
            .to_string_lossy()
            .into_owned(),
        entrypoint: entrypoint.to_string_lossy().into_owned(),
        width: 320,
        height: 240,
        title: "surface lifecycle".to_owned(),
        directory: entrypoint.to_string_lossy().into_owned(),
        storage_identity: "surface-lifecycle-test".to_owned(),
        window: Default::default(),
        tray: None,
        menu: None,
        activation: Default::default(),
    };
    let mut session = WindowSession::open(&mut engine, files, options).expect("a window opens");

    // Settle: the window is up, the surface is built, and frames are turning.
    pump(&mut session, &services, &mut engine, 30);
    assert_eq!(session.surface(), SurfaceState::Present);
    assert!(
        session.renderer_is_active(),
        "the renderer never built a surface, so there is nothing to destroy"
    );
    assert!(
        session.startup_revealed(),
        "the first complete frame never revealed the native window"
    );
    assert_eq!(marker(&mut engine), "set by script");
    let frames_before = probe(&mut engine, "frames");
    let timers_before = probe(&mut engine, "timers");
    assert!(frames_before > 0.0, "the animation never started");
    assert!(timers_before > 0.0, "the timer never started");
    let references_before = session.window_references();

    // The surface goes away. This is the assertion the whole test rests on: a
    // cycle that did nothing would leave the renderer active here, and every
    // later assertion would pass without anything having been torn down.
    session.lose_surface();
    pump(&mut session, &services, &mut engine, 2);
    assert_eq!(session.surface(), SurfaceState::Lost);
    assert!(
        !session.renderer_is_active(),
        "the wgpu surface survived `destroy_surfaces`"
    );

    // While it is gone: no frames, but the document, the heap and the timers
    // are all still here and the timer queue is still being served.
    let frames_at_loss = probe(&mut engine, "frames");
    let timers_at_loss = probe(&mut engine, "timers");
    pump(&mut session, &services, &mut engine, 20);
    assert_eq!(
        probe(&mut engine, "frames"),
        frames_at_loss,
        "requestAnimationFrame ran with no surface to present to"
    );
    assert!(
        probe(&mut engine, "timers") > timers_at_loss,
        "setTimeout stopped when the surface did"
    );
    assert_eq!(marker(&mut engine), "set by script");

    // And it comes back.
    session.restore_surface();
    pump(&mut session, &services, &mut engine, 30);
    assert_eq!(session.surface(), SurfaceState::Present);
    assert!(
        session.renderer_is_active(),
        "the surface was never rebuilt"
    );
    assert!(
        session.startup_revealed(),
        "restoring a surface hid the window behind the startup gate again"
    );
    assert_eq!(
        marker(&mut engine),
        "set by script",
        "the document did not survive the cycle"
    );
    assert!(
        probe(&mut engine, "frames") > frames_at_loss,
        "requestAnimationFrame did not resume once the surface was back"
    );

    // Nine more cycles. One leak would be invisible; ten would not, because
    // every orphaned surface holds its own clone of the window handle.
    for _ in 0..9 {
        session.lose_surface();
        pump(&mut session, &services, &mut engine, 2);
        assert!(!session.renderer_is_active());
        session.restore_surface();
        pump(&mut session, &services, &mut engine, 20);
        assert!(session.renderer_is_active());
    }
    assert_eq!(
        session.window_references(),
        references_before,
        "ten surface cycles leaked references to the window"
    );
    assert_eq!(marker(&mut engine), "set by script");
    assert!(probe(&mut engine, "frames") > frames_before);
}

// # The device run this cannot replace
//
// Everything below needs a physical Android device or emulator and the entry
// point from #142. Written out so #143 or #149 executes it rather than
// rediscovering it.
//
// Preparation: build the cdylib for the device's ABI, install, and
// `adb logcat -c` before each case. A `RUST_LOG=winit=trace` build makes the
// `MainEvent` sequence visible, which is what every case below is really
// asserting about.
//
// 1. **Rotation with `configChanges` declared.** Rotate the device with an
//    animation running and text in an `<input>`. Expect: no `TerminateWindow`
//    and no `InitWindow` in the log — only `ConfigChanged` and
//    `WindowResized` — the animation continues, the text is still there, and
//    `window.innerWidth`/`innerHeight` report the new orientation. A
//    `TerminateWindow` here means the manifest is wrong.
// 2. **Rotation with `configChanges` removed.** The control for (1), run once
//    and never again. Expect the Activity to restart and `android_main` to be
//    re-entered — a fresh document and an empty `<input>`. This is what the
//    manifest declaration is buying, and it should be seen once so nobody has
//    to take the module comment's word for it.
// 3. **Backgrounding.** Home button, wait 30 s, return. Expect `Stop` then
//    `TerminateWindow` on the way out, `InitWindow` then `Start` on the way
//    back; the document intact; a `setTimeout` chain having advanced by roughly
//    30 s worth of ticks; `requestAnimationFrame` having advanced by none; and
//    the first rAF timestamp after the return larger than the last one before
//    it by about 30 000, not reset to zero.
// 4. **Backgrounding mid-gesture.** Press and hold a finger on an element that
//    logs pointer events, then press Home without lifting. Expect a
//    `pointercancel` for that `pointerId` before the app goes, and no stuck
//    `:active`/capture state on return. This is the one behaviour the
//    synthetic cycle cannot reach at all, because desktop winit has no touch
//    contact to leave open.
// 5. **CPU while backgrounded.** With an animation running, background the app
//    and sample `adb shell top -p $(pidof …)` for 30 s. Expect approximately
//    0%. Anything at or near a steady 60 Hz means `paces_a_frame` is not being
//    consulted, or the entry point is not using `blitsen-runtime`'s loop.
// 6. **Memory warning.** `adb shell am send-trim-memory <package> RUNNING_LOW`
//    with a large JavaScript heap allocated. Expect `memory_warning` in the log,
//    a drop in `dumpsys meminfo` native heap, and — this is the part worth
//    watching — no dropped frame and no visual change.
// 7. **Surface loss under memory pressure.** Open several heavy apps until
//    Android reclaims this one's surface while it is still notionally alive.
//    Expect the same `TerminateWindow`/`InitWindow` pair as (3) and a correct
//    first frame on return. This is the case the synthetic cycle models most
//    directly and the one most likely to differ, because the GPU is genuinely
//    under pressure when it happens.
