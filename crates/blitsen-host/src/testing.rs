//! Explicit application UI testing over a private, process-owned command pipe.
//!
//! Uses the window's document loader, retained native input callbacks, async
//! handoff and layout. Only presentation is replaced with CPU rasterization.

use std::io::{BufRead, Write};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use base64::Engine as _;
use blitsen_dom::DomBackend;
use blitsen_js::{JsEngine, JsError};
use serde_json::{Value, json};

use crate::app::{AppFiles, LoadOptions, LoadedWindowDocument, load_window_document};
use crate::dom_bridge::DocumentMode;
use crate::native_window::{InputBootstrap, call_input, hit_test_document};
use crate::runtime_services::RuntimeServices;

const PREFIX: &str = "BLITSEN_TEST:";

/// Test mode is opt-in for the process, never a document global or run default.
pub fn requested() -> bool {
    std::env::var("BLITSEN_TEST_MODE").is_ok_and(|value| value == "1")
}

fn failure(error: impl std::fmt::Display) -> JsError {
    JsError::new(error.to_string())
}

/// Runs an application without a display server or GPU until its driver closes.
pub fn run<E: JsEngine + Clone + 'static>(
    engine: &mut E,
    services: &RuntimeServices<E>,
    files: &AppFiles,
    storage: &crate::storage::LocalStorage,
    width: u32,
    height: u32,
) -> Result<(), JsError> {
    if !requested() {
        return Err(failure(
            "application UI testing requires BLITSEN_TEST_MODE=1",
        ));
    }
    let dimension = |name: &str, fallback: u32| -> Result<u32, JsError> {
        match std::env::var(name) {
            Ok(value) => value
                .parse()
                .map_err(|_| failure(format!("{name} must be a positive integer"))),
            Err(_) => Ok(fallback),
        }
    };
    let width = dimension("BLITSEN_TEST_WIDTH", width)?;
    let height = dimension("BLITSEN_TEST_HEIGHT", height)?;
    if width == 0 || height == 0 || width > 8192 || height > 8192 {
        return Err(failure(
            "test viewport dimensions must be between 1 and 8192",
        ));
    }
    let net = files
        .net_provider()
        .unwrap_or_else(|| Arc::new(blitsen_blitz::resources::LocalResources));
    let loaded = load_window_document(
        engine,
        files,
        net,
        LoadOptions::new(width, height, DocumentMode::HeadlessApplication)
            .with_storage(storage.clone()),
    )?;
    let log = engine.retained_value(&loaded.host_hooks.event_log)?;
    engine.call(&log, None, &[])?;
    let lifecycle = engine.retained_value(&loaded.host_hooks.lifecycle)?;
    let load = engine.string("load")?;
    engine.call(&lifecycle, None, &[load])?;
    let inspector =
        engine.evaluate_script(include_str!("testing/inspect.js"), "blitsen:test-inspector")?;
    let inspector = engine.retain(&inspector)?;
    settle(engine, services, &loaded, Duration::from_millis(50))?;
    respond(&json!({"ready": true, "width": width, "height": height, "devicePixelRatio": 1}))?;

    // Reading stdin must not starve application timers, workers or fetches.
    // EOF tears down the process even if the driver disappeared mid-step.
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            if send.send(line).is_err() {
                break;
            }
        }
    });
    loop {
        let line = match receive.recv_timeout(Duration::from_millis(8)) {
            Ok(line) => line.map_err(failure)?,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                turn(engine, services, &loaded)?;
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let request: Value = serde_json::from_str(&line).map_err(failure)?;
        let id = request["id"].clone();
        let command = request["command"].as_str().unwrap_or("");
        let result = (|| -> Result<Value, JsError> {
            match command {
                "close" => Ok(Value::Null),
                "settle" => {
                    let millis = request["ms"].as_u64().unwrap_or(50);
                    if millis > 30_000 {
                        return Err(failure("settle is limited to 30000 ms per call"));
                    }
                    settle(engine, services, &loaded, Duration::from_millis(millis))?;
                    Ok(Value::Null)
                }
                "query" => {
                    loaded
                        .document
                        .borrow_mut()
                        .flush_layout()
                        .map_err(failure)?;
                    let hook = engine.retained_value(&inspector)?;
                    let argument = engine.string(&request["locator"].to_string())?;
                    let result = engine.call(&hook, None, &[argument])?;
                    serde_json::from_str(&engine.to_string(&result)?).map_err(failure)
                }
                "evaluate" => {
                    loaded
                        .document
                        .borrow_mut()
                        .flush_layout()
                        .map_err(failure)?;
                    let source = request["expression"]
                        .as_str()
                        .ok_or_else(|| failure("evaluate needs an expression"))?;
                    let value = engine.evaluate_script(
                        &format!("(() => {{ const result = ({source}); \
                            if (result && typeof result.then === 'function') \
                            throw new TypeError('test expressions must be synchronous; use waitFor for asynchronous work'); \
                            return JSON.stringify(result ?? null); }})()"),
                        "blitsen:test-assertion",
                    )?;
                    serde_json::from_str(&engine.to_string(&value)?).map_err(failure)
                }
                "pointer" => {
                    let kind = request["type"].as_str().unwrap_or("");
                    if ![
                        "pointermove",
                        "pointerdown",
                        "pointerup",
                        "pointercancel",
                        "wheel",
                    ]
                    .contains(&kind)
                    {
                        return Err(failure("unknown pointer event type"));
                    }
                    let x = request["x"]
                        .as_f64()
                        .ok_or_else(|| failure("pointer needs x"))?;
                    let y = request["y"]
                        .as_f64()
                        .ok_or_else(|| failure("pointer needs y"))?;
                    let hit = hit_test_document(&loaded.document, x, y)
                        .map_err(failure)?
                        .ok_or_else(|| {
                            failure("pointer is outside the viewport or hits no element")
                        })?;
                    if let Some(expected) = request["target"].as_str()
                        && !hit
                            .path
                            .iter()
                            .any(|node| crate::DomRuntime::serialize_handle(*node) == expected)
                    {
                        return Err(failure("element is covered by another element"));
                    }
                    let mut init = request["init"].as_object().cloned().unwrap_or_default();
                    init.extend(json!({"bubbles":true,"cancelable":true,"clientX":x,"clientY":y,
                        "screenX":x,"screenY":y,"offsetX":hit.offset_x,"offsetY":hit.offset_y,
                        "pointerId":1,"pointerType":"mouse","isPrimary":true,
                        "propagationPath":hit.path.iter().map(|node| crate::DomRuntime::serialize_handle(*node)).collect::<Vec<_>>()})
                        .as_object().unwrap().clone());
                    let entry = if kind == "wheel" {
                        InputBootstrap::Mouse
                    } else {
                        InputBootstrap::Pointer
                    };
                    call_input(
                        engine,
                        &loaded.host_hooks,
                        entry,
                        &(kind, crate::DomRuntime::serialize_handle(hit.target), init),
                    )?;
                    Ok(Value::Null)
                }
                "key" => {
                    let kind = request["type"].as_str().unwrap_or("");
                    if !["keydown", "keyup"].contains(&kind) {
                        return Err(failure("unknown keyboard event type"));
                    }
                    let mut init = request["init"].as_object().cloned().unwrap_or_default();
                    init.extend(
                        json!({"bubbles":true,"cancelable":true})
                            .as_object()
                            .unwrap()
                            .clone(),
                    );
                    call_input(
                        engine,
                        &loaded.host_hooks,
                        InputBootstrap::Keyboard,
                        &(kind, init),
                    )?;
                    Ok(Value::Null)
                }
                "text" => {
                    let data = request["text"]
                        .as_str()
                        .ok_or_else(|| failure("text needs a string"))?;
                    call_input(
                        engine,
                        &loaded.host_hooks,
                        InputBootstrap::Ime,
                        &("commit", json!({"data":data})),
                    )?;
                    Ok(Value::Null)
                }
                "screenshot" => {
                    let viewport = loaded.document.borrow().document_ref().viewport().clone();
                    let (_, png) = crate::harness::snapshot_and_render(
                        loaded.document.clone(),
                        viewport.window_size.0,
                        viewport.window_size.1,
                    )?;
                    Ok(json!(base64::engine::general_purpose::STANDARD.encode(png)))
                }
                "eventLog" => {
                    let log = engine.retained_value(&loaded.host_hooks.event_log)?;
                    let result = engine.call(&log, None, &[])?;
                    serde_json::from_str(&engine.to_string(&result)?).map_err(failure)
                }
                _ => Err(failure("unknown application test command")),
            }
        })();
        match result {
            Ok(result) => respond(&json!({"id":id,"result":result}))?,
            Err(error) => respond(&json!({"id":id,"error":error.to_string()}))?,
        }
        if command == "close" {
            break;
        }
        turn(engine, services, &loaded)?;
    }
    engine.evaluate_script(
        "globalThis.__blitsenDisposeContext?.()",
        "blitsen:test-close",
    )?;
    Ok(())
}

fn respond(value: &Value) -> Result<(), JsError> {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{PREFIX}{value}").map_err(failure)?;
    stdout.flush().map_err(failure)
}

fn turn<E: JsEngine + Clone + 'static>(
    engine: &mut E,
    services: &RuntimeServices<E>,
    loaded: &LoadedWindowDocument<E::StrongRef>,
) -> Result<(), JsError> {
    services.run_expired_timers(engine)?;
    loaded
        .document
        .borrow_mut()
        .document_mut()
        .handle_messages();
    engine.drain_microtasks()?;
    let tick = engine.retained_value(&loaded.host_hooks.animation_frame_tick)?;
    let timestamp = engine.number(services.now_ms());
    engine.call(&tick, None, &[timestamp])?;
    engine.drain_microtasks()?;
    loaded
        .document
        .borrow_mut()
        .flush_layout()
        .map_err(failure)?;
    Ok(())
}

fn settle<E: JsEngine + Clone + 'static>(
    engine: &mut E,
    services: &RuntimeServices<E>,
    loaded: &LoadedWindowDocument<E::StrongRef>,
    duration: Duration,
) -> Result<(), JsError> {
    let deadline = Instant::now() + duration;
    loop {
        turn(engine, services, loaded)?;
        if Instant::now() >= deadline {
            return Ok(());
        }
        std::thread::sleep(
            Duration::from_millis(4).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}
