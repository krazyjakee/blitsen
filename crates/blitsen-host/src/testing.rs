//! Explicit application UI testing over a private, process-owned command pipe.
//!
//! Uses the window's document loader, retained native input callbacks, async
//! handoff and layout. Only presentation is replaced with CPU rasterization.

use std::sync::Arc;
use std::time::Instant;

use base64::Engine as _;
use blitsen_dom::DomBackend;
use blitsen_js::{JsEngine, JsError};
use serde_json::{Value, json};

use crate::app::{AppFiles, LoadOptions, LoadedWindowDocument, load_window_document};
use crate::dom_bridge::DocumentMode;
use crate::native_window::{InputBootstrap, call_input, hit_test_document};

/// Application UI testing is explicitly selected by the launcher.
pub fn requested() -> bool {
    std::env::var("BLITSEN_TEST_MODE").is_ok_and(|value| value == "1")
}
fn failure(error: impl std::fmt::Display) -> JsError {
    JsError::new(error.to_string())
}

/// Retained headless application state. Bun owns stdin and the asynchronous loop;
/// each native call performs one bounded command or frame and returns to Bun.
pub struct TestSession<E: JsEngine> {
    loaded: LoadedWindowDocument<E::StrongRef>,
    inspector: E::StrongRef,
    clock: Instant,
    /// Logical viewport width.
    pub width: u32,
    /// Logical viewport height.
    pub height: u32,
}

impl<E: JsEngine + Clone + 'static> TestSession<E> {
    /// Loads the application with native input and CPU rendering.
    pub fn new(
        engine: &mut E,
        files: &AppFiles,
        storage: &crate::storage::LocalStorage,
        width: u32,
        height: u32,
    ) -> Result<Self, JsError> {
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

        Ok(Self {
            loaded,
            inspector,
            clock: Instant::now(),
            width,
            height,
        })
    }

    /// Executes one command from the private test pipe.
    pub fn command(&mut self, engine: &mut E, request: &Value) -> Result<Value, JsError> {
        let command = request["command"].as_str().unwrap_or("");
        match command {
            "close" => Ok(Value::Null),
            "query" => {
                self.loaded
                    .document
                    .borrow_mut()
                    .flush_layout()
                    .map_err(failure)?;
                let hook = engine.retained_value(&self.inspector)?;
                let argument = engine.string(&request["locator"].to_string())?;
                let result = engine.call(&hook, None, &[argument])?;
                serde_json::from_str(&engine.to_string(&result)?).map_err(failure)
            }
            "evaluate" => {
                self.loaded
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
                let hit = hit_test_document(&self.loaded.document, x, y)
                    .map_err(failure)?
                    .ok_or_else(|| failure("pointer is outside the viewport or hits no element"))?;
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
                    &self.loaded.host_hooks,
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
                    &self.loaded.host_hooks,
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
                    &self.loaded.host_hooks,
                    InputBootstrap::Ime,
                    &("commit", json!({"data":data})),
                )?;
                Ok(Value::Null)
            }
            "screenshot" => {
                let viewport = self
                    .loaded
                    .document
                    .borrow()
                    .document_ref()
                    .viewport()
                    .clone();
                let (_, png) = crate::harness::snapshot_and_render(
                    self.loaded.document.clone(),
                    viewport.window_size.0,
                    viewport.window_size.1,
                )?;
                Ok(json!(base64::engine::general_purpose::STANDARD.encode(png)))
            }
            "eventLog" => {
                let log = engine.retained_value(&self.loaded.host_hooks.event_log)?;
                let result = engine.call(&log, None, &[])?;
                serde_json::from_str(&engine.to_string(&result)?).map_err(failure)
            }
            _ => Err(failure("unknown application test command")),
        }
    }

    /// Delivers one frame of native completions.
    pub fn turn(&self, engine: &mut E) -> Result<(), JsError> {
        self.loaded
            .document
            .borrow_mut()
            .document_mut()
            .handle_messages();
        let tick = engine.retained_value(&self.loaded.host_hooks.animation_frame_tick)?;
        let timestamp = engine.number(self.clock.elapsed().as_secs_f64() * 1000.0);
        engine.call(&tick, None, &[timestamp])?;
        Ok(())
    }

    /// Disposes the application context.
    pub fn close(&self, engine: &mut E) -> Result<(), JsError> {
        engine.evaluate_script(
            "globalThis.__blitsenDisposeContext?.()",
            "blitsen:test-close",
        )?;
        Ok(())
    }
}
