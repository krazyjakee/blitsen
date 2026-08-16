//! The headless self-check an exported application answers.
//!
//! `BLITSEN_STANDALONE_CHECK=1` boots the document, lets its asynchronous work
//! settle, optionally runs a script and an assertion, renders a frame and says
//! so. No window opens, which is what makes it usable on a CI runner with no
//! display.
//!
//! It exists on both hosts and prints the same two lines, because issue #90
//! turns on an exported artifact behaving identically across the swap. The
//! Phase 1 version is generated into the Bun launcher (`export.mjs`); this is
//! the same sequence with Blitsen's own event loop under it.

use std::rc::Rc;
use std::sync::Arc;

use blitsen_js::{JsEngine, JsError};
use blitz::traits::net::NetProvider;

use crate::app::{AppFiles, LoadOptions, LoadedDocument, load_document};
use crate::dom_bridge::DocumentMode;
use crate::harness::snapshot_and_render;
use crate::runtime_services::RuntimeServices;

/// Whether this process was started to check itself rather than to run.
pub fn requested() -> bool {
    std::env::var("BLITSEN_STANDALONE_CHECK").is_ok_and(|value| value == "1")
}

/// What the check reports about the application it is checking.
pub struct Reported<'a> {
    /// Viewport the document is booted at.
    pub width: u32,
    /// Viewport the document is booted at.
    pub height: u32,
    /// How many of the application's own files the export collected.
    pub assets: usize,
    /// `embedded` or `side-loaded`.
    pub layout: &'a str,
    /// The runtime naming itself, for the second line.
    pub runtime: &'a str,
}

/// Runs the check and prints what an exported application is expected to print.
pub fn run<E: JsEngine + Clone + 'static>(
    engine: &mut E,
    services: &RuntimeServices<E>,
    files: &AppFiles,
    reported: &Reported<'_>,
) -> Result<(), JsError> {
    let Reported {
        width,
        height,
        assets,
        layout,
        runtime,
    } = *reported;
    let net_provider = files.net_provider().unwrap_or_else(|| {
        Arc::new(blitsen_blitz::resources::LocalResources) as Arc<dyn NetProvider>
    });
    let loaded = load_document(
        engine,
        files,
        net_provider,
        LoadOptions::new(width, height, DocumentMode::Application),
    )?;
    engine.evaluate_script(
        "globalThis.__blitsenDispatchLifecycleEvent('load')",
        "blitsen:load",
    )?;

    let delay = millis("BLITSEN_STANDALONE_CHECK_DELAY", 50);
    settle(engine, services, &loaded, width, height, delay)?;
    if let Ok(script) = std::env::var("BLITSEN_STANDALONE_CHECK_SCRIPT") {
        engine.evaluate_script(&script, "blitsen:standalone-check-script")?;
    }
    settle(engine, services, &loaded, width, height, delay)?;
    if let Ok(assertion) = std::env::var("BLITSEN_STANDALONE_CHECK_ASSERT") {
        engine.evaluate_script(&assertion, "blitsen:standalone-check-assert")?;
    }

    println!("Blitsen standalone check passed ({assets} {layout} assets)");
    println!("Blitsen runtime: {runtime}");
    Ok(())
}

/// Turns the loop for `delay`, so timers, fetches and image decodes land.
///
/// The same landing point a windowed frame uses — the animation-frame tick —
/// so what settles here is what would settle in a real frame, rather than a
/// separate drain that could diverge from it.
fn settle<E: JsEngine + Clone + 'static>(
    engine: &mut E,
    services: &RuntimeServices<E>,
    loaded: &LoadedDocument,
    width: u32,
    height: u32,
    delay: std::time::Duration,
) -> Result<(), JsError> {
    let deadline = std::time::Instant::now() + delay;
    let mut frame = 0_u32;
    loop {
        services.run_expired_timers(engine)?;
        loaded
            .document
            .borrow_mut()
            .document_mut()
            .handle_messages();
        let timestamp = services.now_ms();
        engine.evaluate_script(
            &format!("globalThis.__blitsenAnimationFrameTick({timestamp})"),
            "blitsen:standalone-check-frame",
        )?;
        engine.drain_microtasks()?;
        frame += 1;
        let now = std::time::Instant::now();
        if now >= deadline {
            break;
        }
        // One frame's worth of waiting, or the next timer if it is sooner.
        let step = std::time::Duration::from_millis(4).min(deadline - now);
        let wait = services
            .next_timer_delay()
            .map_or(step, |timer| step.min(timer));
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
    }
    debug_assert!(frame > 0, "the check must turn the loop at least once");

    // Rendering is part of the check: a document that lays out but cannot paint
    // is exactly the failure an exported binary needs to catch before shipping.
    snapshot_and_render(Rc::clone(&loaded.document), width, height)?;
    Ok(())
}

fn millis(name: &str, fallback: u64) -> std::time::Duration {
    let value = std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(fallback);
    std::time::Duration::from_millis(value)
}
