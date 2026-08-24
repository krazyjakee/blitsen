//! The `#[napi]` surface: what the JavaScript test suite and CLI call.
//!
//! Each of these is a thin adapter — take an environment, hand `blitsen-host`
//! an engine over it, and serialize the result. The assertions themselves are
//! the host's, so Phase 2 runs the same ones without going through Node-API.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use blitsen_core::{WindowState, WrapperTable};
use blitsen_dom::DomBackend;
use blitsen_host::harness::{self, active_document_harness};
use blitsen_host::{dom_error, replay};
use blitsen_js::{
    ExternalId, JsEngine, JsError, JsType, NativeClass, NativeMethod, TypedArray, TypedArrayKind,
};
use blitz::dom::NodeId;
use napi::{Env, Status, sys};
use napi_derive::napi;

use crate::engine::{check, raw};
use crate::{NodeApiEngine, NodeWeakRef, napi_error};

fn engine(env: Env) -> NodeApiEngine {
    NodeApiEngine::new(env)
}

/// The error every adapter below reports a failure of its own as.
fn failure(message: impl std::fmt::Display) -> napi::Error {
    napi::Error::new(Status::GenericFailure, message.to_string())
}

/// Serializes a harness result, which is how every one of them crosses back.
///
/// The boundary carries a string rather than a Node-API object: the shapes are
/// the host's `Serialize` types, and the suite parses them straight back.
fn json(value: &impl serde::Serialize) -> napi::Result<String> {
    serde_json::to_string(value).map_err(failure)
}

/// The document a `run_document_scripts_harness` call left loaded.
///
/// Absent when the suite asserts before it has loaded anything, which is a test
/// bug rather than a runtime one, so it is named as such.
fn active_harness() -> napi::Result<harness::ActiveDocumentHarness> {
    active_document_harness().ok_or_else(|| failure("no document harness is active"))
}

/// A frame count, capped so a mistyped argument cannot render for an hour.
fn frame_count(frames: Option<u32>, default: u32) -> napi::Result<u32> {
    let frames = frames.unwrap_or(default);
    if frames > 10_000 {
        return Err(napi::Error::new(
            Status::InvalidArg,
            "the animation harness is limited to 10000 frames",
        ));
    }
    Ok(frames)
}

/// Creates a directory frames are about to be written into.
fn recording_directory(directory: String) -> napi::Result<PathBuf> {
    let directory = PathBuf::from(directory);
    std::fs::create_dir_all(&directory)
        .map_err(|error| failure(format!("could not create {}: {error}", directory.display())))?;
    Ok(directory)
}

/// Boots Blitz headlessly, runs JavaScript DOM mutations, and returns the Rust
/// tree state as JSON for cross-platform CI assertions.
#[napi]
pub fn run_bridge_harness(
    env: Env,
    html: String,
    script: String,
    width: Option<u32>,
    height: Option<u32>,
) -> napi::Result<String> {
    let (snapshot, _) = harness::execute_bridge_harness(engine(env), html, script, width, height)
        .map_err(napi_error)?;
    json(&snapshot)
}

/// Advances a document through a deterministic sequence of animation frames.
#[napi]
pub fn run_animation_harness(
    env: Env,
    html: String,
    script: String,
    frames: Option<u32>,
    width: Option<u32>,
    height: Option<u32>,
) -> napi::Result<String> {
    let snapshots = harness::execute_animation_harness(
        engine(env),
        html,
        script,
        frame_count(frames, 3)?,
        width.unwrap_or(800),
        height.unwrap_or(600),
    )
    .map_err(napi_error)?;
    json(&snapshots)
}

/// Loads a real HTML entrypoint and executes its collected script elements.
#[napi]
pub fn run_document_scripts_harness(
    env: Env,
    entrypoint: String,
    width: Option<u32>,
    height: Option<u32>,
) -> napi::Result<String> {
    let snapshot = harness::execute_document_harness(
        engine(env),
        Path::new(&entrypoint),
        width.unwrap_or(800),
        height.unwrap_or(600),
    )
    .map_err(napi_error)?;
    json(&snapshot)
}

/// Evaluates a script against the most recently loaded document harness.
#[napi]
pub fn evaluate_document_harness(env: Env, script: String) -> napi::Result<()> {
    active_harness()?;
    NodeApiEngine::new(env)
        .evaluate_script(&script, "document-harness-evaluation.js")
        .map(|_| ())
        .map_err(napi_error)
}

/// Snapshots the most recently loaded document after the host event loop has advanced.
#[napi]
pub fn snapshot_document_harness() -> napi::Result<String> {
    let (document, width, height) = active_harness()?;
    let snapshot = harness::snapshot_and_render(document, width, height)
        .map(|(snapshot, _)| snapshot)
        .map_err(napi_error)?;
    json(&snapshot)
}

/// Serializes the tree of the most recently loaded document harness.
///
/// Backs the layout conformance corpus, whose framework cases are the markup a
/// real bundle actually built rather than a hand-written imitation of it. The
/// serialization happens after the document's scripts have run, so what comes
/// back is the rendered tree, not the near-empty root element the bundle ships.
#[napi]
pub fn capture_document_harness_html() -> napi::Result<String> {
    let (document, _, _) = active_harness()?;
    let document = document.borrow();
    let root = document
        .document_element()
        .ok_or_else(|| failure("document has no root"))?;
    document
        .inner_html(root)
        .map_err(|error| napi_error(dom_error(error)))
}

/// Loads a real HTML entrypoint and advances its animation loop at 60 Hz.
#[napi]
pub fn run_document_animation_harness(
    env: Env,
    entrypoint: String,
    setup_script: String,
    frames: Option<u32>,
    width: Option<u32>,
    height: Option<u32>,
) -> napi::Result<String> {
    let snapshots = harness::execute_document_animation_harness(
        engine(env),
        Path::new(&entrypoint),
        &setup_script,
        frame_count(frames, 60)?,
        width.unwrap_or(800),
        height.unwrap_or(600),
        None,
    )
    .map_err(napi_error)?;
    json(&snapshots)
}

/// Advances a document's animation loop and writes every rendered frame as a PNG.
///
/// Backs the recorded demos in the documentation. The frames are the same ones the
/// acceptance harness asserts on, so a published recording cannot drift away from
/// what the tests actually verify.
#[napi]
pub fn record_document_animation_harness(
    env: Env,
    entrypoint: String,
    setup_script: String,
    directory: String,
    frames: Option<u32>,
    width: Option<u32>,
    height: Option<u32>,
) -> napi::Result<u32> {
    let frames = frame_count(frames, 60)?;
    let directory = recording_directory(directory)?;
    harness::execute_document_animation_harness(
        engine(env),
        Path::new(&entrypoint),
        &setup_script,
        frames,
        width.unwrap_or(800),
        height.unwrap_or(600),
        Some(&directory),
    )
    .map_err(napi_error)?;
    Ok(frames)
}

/// Replays a recorded input trace at a fixed timestep.
///
/// Deterministic by construction: JavaScript only ever sees timestamps derived
/// from the trace, never the wall clock, while the wall clock measures what each
/// frame actually cost. The returned report carries a digest sequence to compare
/// against a golden and a frame-time histogram to record.
#[napi]
pub fn replay_document_frames(
    env: Env,
    entrypoint: String,
    trace: String,
    record_into: Option<String>,
    record_frames: Option<Vec<u32>>,
) -> napi::Result<String> {
    let trace = blitsen_core::replay::InputTrace::from_json(&trace)
        .map_err(|error| napi::Error::new(Status::InvalidArg, error.to_string()))?;
    let directory = record_into.map(recording_directory).transpose()?;
    let report = replay::replay(
        engine(env),
        Path::new(&entrypoint),
        trace,
        directory.as_deref(),
        &record_frames.unwrap_or_default(),
    )
    .map_err(napi_error)?;
    json(&report)
}

/// Renders the post-JavaScript frame as a base64-encoded PNG.
#[napi]
pub fn render_bridge_harness_png(
    env: Env,
    html: String,
    script: String,
    width: Option<u32>,
    height: Option<u32>,
) -> napi::Result<String> {
    let (_, png) = harness::execute_bridge_harness(engine(env), html, script, width, height)
        .map_err(napi_error)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(png))
}

/// Exercises the real Node-API weak-reference and finalizer identity path.
///
/// Returns how many of the wrappers were **still live** after collection was
/// driven — zero on a runner that finished, and the survivors on one that did
/// not. A count rather than a verdict because the two ways this can go wrong are
/// not the same thing: a tail of survivors is a collector that did not finish,
/// and no survivors collected at all is a finalizer that never ran, which would
/// mean `WrapperTable` retains every DOM node an application touches. Issue #136
/// exists because a boolean could not tell those apart.
///
/// An identity failure is an error rather than a count, because it is not a
/// question of degree.
#[napi]
pub fn wrapper_identity_smoke(env: Env) -> napi::Result<u32> {
    let mut engine = NodeApiEngine::new(env);
    let class = engine
        .register_class(NativeClass::new("IdentityNode"))
        .map_err(napi_error)?;
    let table = WrapperTable::<NodeId, NodeWeakRef>::new();
    let raw_env = engine.raw_env();
    let weak_map_works = Env::from_raw(raw_env).run_in_scope(|| {
        let node = NodeId::from_u64(1);
        let first = table
            .get_or_create(&mut engine, node, |engine, finalizer| {
                engine.instantiate(&class, ExternalId(node.as_u64()), Some(finalizer))
            })
            .map_err(napi_error)?;
        let second = table
            .get_or_create(&mut engine, node, |_, _| {
                Err(JsError::new("identity table created a duplicate wrapper"))
            })
            .map_err(napi_error)?;
        let mut strictly_equal = false;
        check(
            unsafe {
                sys::napi_strict_equals(raw_env, raw(&first), raw(&second), &mut strictly_equal)
            },
            "compare wrapper identity",
        )
        .map_err(napi_error)?;
        if !strictly_equal {
            return Ok(false);
        }
        engine
            .set_global("__blitsenIdentityFirst", &first)
            .and_then(|_| engine.set_global("__blitsenIdentitySecond", &second))
            .map_err(napi_error)?;
        engine
            .evaluate_script(
                "(() => { const identityMap = new WeakMap([[__blitsenIdentityFirst, 42]]); return identityMap.get(__blitsenIdentitySecond) === 42; })()",
                "blitsen:identity-weak-map",
            )
            .and_then(|value| engine.to_boolean(&value))
            .map_err(napi_error)
    })?;
    if !weak_map_works {
        return Err(napi_error(JsError::new(
            "a wrapper did not survive a WeakMap round trip: identity is not preserved",
        )));
    }

    for slot in 2..=100_001_u64 {
        Env::from_raw(raw_env).run_in_scope(|| {
            let node = NodeId::from_u64(slot);
            table
                .get_or_create(&mut engine, node, |engine, finalizer| {
                    engine.instantiate(&class, ExternalId(node.as_u64()), Some(finalizer))
                })
                .map(|_| ())
                .map_err(napi_error)
        })?;
    }
    if table.len() != 100_001 {
        return Err(napi_error(JsError::new(format!(
            "the identity table holds {} wrappers, expected 100001",
            table.len()
        ))));
    }
    engine
        .evaluate_script(
            "delete globalThis.__blitsenIdentityFirst; delete globalThis.__blitsenIdentitySecond",
            "blitsen:identity-drop",
        )
        .map_err(napi_error)?;
    // Collected, not collected-in-a-fixed-number-of-passes. No garbage collector
    // promises to finish a heap this size in a set number of calls, and the
    // runners disagree about whether it does: `linux-x64` and `darwin-arm64`
    // drain, `win32-x64` and `linux-arm64` do not, which is neither an operating
    // system nor an architecture line (#136). Drive it until it drains, bounded
    // so a collector that never frees anything returns rather than hanging, and
    // report what survived either way.
    for _ in 0..32 {
        engine
            .evaluate_script(
                "globalThis.Bun?.gc?.(true) ?? globalThis.gc?.()",
                "blitsen:identity-gc",
            )
            .map_err(napi_error)?;
        table.prune_collected(&mut engine).map_err(napi_error)?;
        if table.is_empty() {
            break;
        }
    }
    Ok(u32::try_from(table.len()).unwrap_or(u32::MAX))
}

/// Turns a failed smoke assertion into the capability name the runner needs to
/// diagnose it.
fn smoke_check(condition: bool, capability: &str) -> napi::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(failure(format!(
            "Node-API smoke check failed: {capability}"
        )))
    }
}

fn smoke_values(engine: &mut NodeApiEngine) -> napi::Result<()> {
    let string = engine.string("42").map_err(napi_error)?;
    smoke_check(
        engine.to_number(&string).map_err(napi_error)? == 42.0,
        "string-to-number conversion returned the wrong value",
    )?;
    let one = engine.number(1.0);
    let two = engine.number(2.0);
    let array = engine.array(&[one, two]).map_err(napi_error)?;
    smoke_check(
        engine.to_array(&array).map_err(napi_error)?.len() == 2,
        "array round trip returned the wrong length",
    )?;
    let object = engine.object().map_err(napi_error)?;
    let property = engine.string("nul-safe").map_err(napi_error)?;
    engine
        .set_property(&object, "before\0after", &property)
        .map_err(napi_error)?;
    let property = engine
        .get_property(&object, "before\0after")
        .and_then(|value| engine.to_string(&value))
        .map_err(napi_error)?;
    smoke_check(
        property == "nul-safe",
        "property access changed an embedded-NUL name",
    )?;
    for (kind, bytes) in [
        (TypedArrayKind::Int8, vec![0x80, 0x7f]),
        (TypedArrayKind::Uint8, vec![1, 2, 3]),
        (TypedArrayKind::Uint8Clamped, vec![0, 127, 255]),
        (TypedArrayKind::Int16, (-12_345_i16).to_ne_bytes().to_vec()),
        (TypedArrayKind::Uint16, 54_321_u16.to_ne_bytes().to_vec()),
        (
            TypedArrayKind::Int32,
            (-123_456_789_i32).to_ne_bytes().to_vec(),
        ),
        (
            TypedArrayKind::Uint32,
            3_000_000_000_u32.to_ne_bytes().to_vec(),
        ),
        (TypedArrayKind::Float32, (-12.5_f32).to_ne_bytes().to_vec()),
        (
            TypedArrayKind::Float64,
            std::f64::consts::PI.to_ne_bytes().to_vec(),
        ),
        (
            TypedArrayKind::BigInt64,
            (-9_000_000_000_i64).to_ne_bytes().to_vec(),
        ),
        (
            TypedArrayKind::BigUint64,
            18_000_000_000_u64.to_ne_bytes().to_vec(),
        ),
    ] {
        let expected = TypedArray::new(kind, bytes).map_err(napi_error)?;
        let value = engine.typed_array(&expected).map_err(napi_error)?;
        smoke_check(
            engine.to_typed_array(&value).map_err(napi_error)? == expected,
            &format!("{kind:?} round trip changed its kind or bytes"),
        )?;
    }
    let result = engine
        .evaluate_script("21 * 2", "smoke.js")
        .and_then(|value| engine.to_number(&value))
        .map_err(napi_error)?;
    smoke_check(result == 42.0, "script evaluation returned the wrong value")?;

    let identity = engine
        .define_function("identity", Box::new(|call| Ok(call.arguments[0])))
        .map_err(napi_error)?;
    let argument = engine.string("callback").map_err(napi_error)?;
    let result = engine
        .call(&identity, None, &[argument])
        .and_then(|value| engine.to_string(&value))
        .map_err(napi_error)?;
    smoke_check(
        result == "callback",
        "native callback argument/result round trip changed the value",
    )?;

    let strict_receiver = engine
        .evaluate_script(
            "(function () { 'use strict'; return this; })",
            "receiver-smoke.js",
        )
        .map_err(napi_error)?;
    let receiver = engine.number(17.0);
    let result = engine
        .call(&strict_receiver, Some(&receiver), &[])
        .and_then(|value| engine.to_number(&value))
        .map_err(napi_error)?;
    smoke_check(
        result == 17.0,
        "function invocation changed a primitive receiver",
    )
}

fn smoke_class_and_weak_ref(engine: &mut NodeApiEngine) -> napi::Result<()> {
    let class = engine
        .register_class(NativeClass::new("SmokeNode").with_method(NativeMethod::new(
            "identity",
            Box::new(|call| Ok(call.this)),
        )))
        .map_err(napi_error)?;
    let instance = engine
        .instantiate(&class, ExternalId(42), None)
        .map_err(napi_error)?;
    smoke_check(
        engine.external_id(&instance).map_err(napi_error)? == ExternalId(42),
        "native class instance lost its external identity",
    )?;
    let method = engine
        .get_property(&instance, "identity")
        .map_err(napi_error)?;
    engine
        .call(&method, Some(&instance), &[])
        .map_err(napi_error)?;
    let weak = engine.downgrade(&instance).map_err(napi_error)?;
    smoke_check(
        engine.upgrade(&weak).map_err(napi_error)?.is_some(),
        "weak reference did not upgrade while its instance was still live",
    )
}

fn smoke_globals_and_window(engine: &mut NodeApiEngine) -> napi::Result<()> {
    let global_value = engine.string("visible").map_err(napi_error)?;
    engine
        .set_global("__blitsenSmoke", &global_value)
        .map_err(napi_error)?;
    let global_result = engine
        .evaluate_script("globalThis.__blitsenSmoke", "global-smoke.js")
        .and_then(|value| engine.to_string(&value))
        .map_err(napi_error)?;
    smoke_check(
        global_result == "visible",
        "global set/evaluate/read round trip changed the value",
    )?;

    let document = engine.object().map_err(napi_error)?;
    let mut window_state = WindowState::new(800, 600, 2.0);
    let window = window_state
        .install(engine, &document)
        .map_err(napi_error)?;
    let window_check = engine
        .evaluate_script(
            "window === globalThis && window.document !== undefined && innerWidth === 800 && innerHeight === 600 && devicePixelRatio === 2 && !('location' in window) && !('history' in window) && !('navigator' in window) && !('localStorage' in window)",
            "window-smoke.js",
        )
        .and_then(|value| engine.to_boolean(&value))
        .map_err(napi_error)?;
    smoke_check(
        window_check,
        "window installation exposed the wrong globals or dimensions",
    )?;
    window_state.resize(1024, 768);
    window_state.sync(engine, &window).map_err(napi_error)?;
    let resized = engine
        .evaluate_script(
            "innerWidth === 1024 && innerHeight === 768",
            "resize-smoke.js",
        )
        .and_then(|value| engine.to_boolean(&value))
        .map_err(napi_error)?;
    smoke_check(
        resized,
        "window resize did not synchronize the global dimensions",
    )
}

fn smoke_error_propagation(engine: &mut NodeApiEngine) -> napi::Result<()> {
    let throwing = engine
        .define_function(
            "throwing",
            Box::new(|_| Err(JsError::new("native callback failed"))),
        )
        .map_err(napi_error)?;
    let error = match engine.call(&throwing, None, &[]) {
        Ok(_) => {
            return Err(failure(
                "Node-API smoke check failed: native callback returned instead of throwing",
            ));
        }
        Err(error) => error,
    };
    smoke_check(
        error.message().contains("native callback failed"),
        "native callback error lost its message",
    )?;

    let throwing = engine
        .evaluate_script(
            "(function () { throw new Error('receiver call failed'); })",
            "receiver-error-smoke.js",
        )
        .map_err(napi_error)?;
    let receiver = engine.object().map_err(napi_error)?;
    let error = match engine.call(&throwing, Some(&receiver), &[]) {
        Ok(_) => return Err(failure("the JavaScript function did not throw")),
        Err(error) => error,
    };
    smoke_check(
        error.message().contains("receiver call failed"),
        "JavaScript callback error with a receiver lost its message",
    )
}

fn smoke_large_module(engine: &mut NodeApiEngine) -> napi::Result<()> {
    // Keep this comfortably above common filesystem component limits. Bun used
    // to mistake the data URL carrying a production-sized inline module for a
    // package path, then fail with NameTooLong before evaluating it.
    let large_module_source = format!(
        "globalThis.__blitsenLargeModule = 42;\n/* {} */\nexport const answer = 42",
        "module-padding".repeat(1024)
    );
    let module = engine
        .evaluate_module(&large_module_source, "smoke-module.js")
        .map_err(napi_error)?;
    smoke_check(
        engine.value_type(&module).map_err(napi_error)? == JsType::Object,
        "large inline module evaluation returned a non-object",
    )?;
    let large_module_ran = engine
        .evaluate_script(
            "globalThis.__blitsenLargeModule === 42",
            "large-module-smoke.js",
        )
        .and_then(|value| engine.to_boolean(&value))
        .map_err(napi_error)?;
    smoke_check(large_module_ran, "large inline module did not execute")
}

/// Runs the load-bearing Node-API subset used by the trait implementation.
///
/// This is exported for the Bun compatibility test and is not public package API.
/// The established JavaScript contract remains `true` on success; a failed
/// capability now throws an error naming itself instead of returning `false`.
#[napi]
pub fn node_api_smoke(env: Env) -> napi::Result<bool> {
    let mut engine = NodeApiEngine::new(env);
    smoke_values(&mut engine)?;
    smoke_class_and_weak_ref(&mut engine)?;
    smoke_globals_and_window(&mut engine)?;
    smoke_error_propagation(&mut engine)?;
    let is_bun = engine
        .evaluate_script("typeof Bun !== 'undefined'", "blitsen:host-check")
        .and_then(|value| engine.to_boolean(&value))
        .map_err(napi_error)?;
    // The value/reference/function half is portable Node-API and runs in both
    // hosts. Module loading deliberately exercises Bun's createRequire support
    // for Blob URLs, so it remains a Bun-only capability check.
    if is_bun {
        smoke_large_module(&mut engine)?;
    }
    engine.drain_microtasks().map_err(napi_error)?;
    engine.pump_event_loop().map_err(napi_error)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::smoke_check;

    #[test]
    fn smoke_checks_succeed_or_name_the_failed_capability() {
        smoke_check(true, "successful capability").expect("a successful check continues");

        let string_failure =
            smoke_check(false, "string conversion").expect_err("a failed string check is an error");
        let module_failure =
            smoke_check(false, "module evaluation").expect_err("a failed module check is an error");
        assert_eq!(
            string_failure.reason,
            "Node-API smoke check failed: string conversion"
        );
        assert_eq!(
            module_failure.reason,
            "Node-API smoke check failed: module evaluation"
        );
        assert_ne!(string_failure.reason, module_failure.reason);
    }
}
