//! The small native half of the standard pointer-lock and fullscreen APIs.
//!
//! Their observable state, events and promises belong to the DOM bootstrap.
//! This callback only asks winit to apply the corresponding window mode.  The
//! test harness accepts the commands without a platform window so the DOM state
//! machine can be exercised deterministically.

use blitsen_js::{JsEngine, JsError};

pub(super) fn install<E: JsEngine + 'static>(
    engine: &mut E,
    test_harness: bool,
) -> Result<(), JsError> {
    engine.define_global_function(
        "__blitsenWindowMode",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let action = call
                .arguments
                .first()
                .ok_or_else(|| JsError::new("window mode action is required"))?;
            let action = engine.to_string(action)?;
            let pointer_lock_supported =
                test_harness || cfg!(any(target_os = "windows", target_os = "macos"));
            let fullscreen_supported = test_harness || cfg!(not(target_os = "android"));
            if action == "pointerLockSupported" {
                return Ok(engine.boolean(pointer_lock_supported));
            }
            if action == "fullscreenSupported" {
                return Ok(engine.boolean(fullscreen_supported));
            }
            if (action == "lockPointer" || action == "unlockPointer")
                && !pointer_lock_supported
            {
                return Err(JsError::new(
                    "pointer lock is supported on Windows and macOS; pinned winit cannot lock on X11",
                ));
            }
            if (action == "enterFullscreen" || action == "exitFullscreen")
                && !fullscreen_supported
            {
                return Err(JsError::new(
                    "the standard fullscreen API is not supported on Android",
                ));
            }
            if !test_harness {
                crate::dom_bridge::window::web_mode(&action)?;
            }
            Ok(engine.undefined())
        }),
    )
}

#[cfg(test)]
mod tests {
    use blitsen_blitz::BlitzDom;
    use blitsen_js::JsEngine;
    use blitz::dom::DocumentConfig;
    use blitz::traits::shell::{ColorScheme, Viewport};

    type Realm = (
        blitsen_quickjs::QuickJs,
        crate::runtime_services::RuntimeServices<blitsen_quickjs::QuickJs>,
    );

    fn realm(mode: crate::dom_bridge::DocumentMode) -> Realm {
        let mut engine = blitsen_quickjs::QuickJs::new().expect("an engine");
        let services = crate::runtime_services::RuntimeServices::install(&mut engine)
            .expect("runtime services");
        let dom = BlitzDom::from_html(
            r#"<!doctype html><html><head><style>
              html, body { margin: 0; width: 200px; height: 100px }
              #outer, #other-parent { position: absolute; top: 0; width: 80px; height: 80px }
              #outer { left: 0 }
              #other-parent { left: 100px }
              #target, #other { width: 80px; height: 80px }
            </style></head><body>
              <div id='outer'><div id='target'></div></div>
              <div id='other-parent'><div id='other'></div></div>
            </body></html>"#,
            DocumentConfig {
                viewport: Some(Viewport::new(200, 100, 1.0, ColorScheme::Light)),
                ..Default::default()
            },
        );
        let runtime = crate::DomRuntime::new(dom);
        crate::dom_bridge::install(
            &mut engine,
            runtime,
            crate::dom_bridge::InstallOptions::new(200, 100, 1.0, mode, None),
        )
        .expect("the bridge installs");
        (engine, services)
    }

    fn settle(
        engine: &mut blitsen_quickjs::QuickJs,
        services: &crate::runtime_services::RuntimeServices<blitsen_quickjs::QuickJs>,
    ) {
        for _ in 0..4 {
            services.run_expired_timers(engine).expect("mode tasks run");
            engine.drain_microtasks().expect("promise reactions run");
        }
    }

    fn seen(engine: &mut blitsen_quickjs::QuickJs) -> String {
        let value = engine
            .evaluate_script("globalThis.__seen.join('|')", "blitsen:test-observations")
            .expect("observations are readable");
        engine.to_string(&value).expect("observations are text")
    }

    fn json(engine: &mut blitsen_quickjs::QuickJs, script: &str) -> serde_json::Value {
        let value = engine
            .evaluate_script(script, "blitsen:test-pointer-path")
            .expect("the pointer path test runs");
        let value = engine.to_string(&value).expect("the result is JSON text");
        serde_json::from_str(&value).expect("the result is valid JSON")
    }

    #[test]
    fn native_hit_path_skips_parent_bridge_calls_for_pointer_and_compatibility_mouse() {
        let (mut engine, _services) = realm(crate::dom_bridge::DocumentMode::TestHarness);
        let result = json(
            &mut engine,
            r#"
          (() => {
            const outer = document.getElementById("outer");
            const target = document.getElementById("target");
            const order = [];
            for (const [node, name] of [[window, "window"], [document, "document"],
              [outer, "outer"], [target, "target"]]) {
              for (const type of ["pointermove", "mousemove"]) {
                node.addEventListener(type, () => order.push(`${type}:${name}:capture`), true);
                node.addEventListener(type, () => order.push(`${type}:${name}:bubble`));
              }
            }
            // Refuse only mousedown's focus default, which otherwise dispatches
            // four unrelated focus events through their ordinary DOM paths.
            // Pointer compatibility remains enabled, including the click.
            target.addEventListener("mousedown", event => event.preventDefault());
            const clicks = [];
            target.addEventListener("click", event => {
              clicks.push(`target:${event.target.id}`);
              event.preventDefault();
            });
            outer.addEventListener("click", event => clicks.push(`outer:${event.target.id}`));
            const parents = __blitsenDomCallCount("parentNode");
            const connected = __blitsenDomCallCount("isConnected");
            const hit = __blitsenInjectPointerAt("pointermove", 10, 10, {
              pointerId: 1, pointerType: "mouse", isPrimary: true,
            });
            __blitsenInjectPointerAt("pointerdown", 10, 10, {
              pointerId: 1, pointerType: "mouse", isPrimary: true, button: 0,
            });
            __blitsenInjectPointerAt("pointerup", 10, 10, {
              pointerId: 1, pointerType: "mouse", isPrimary: true, button: 0,
            });
            return JSON.stringify({
              target: hit?.target.id ?? null,
              parents: __blitsenDomCallCount("parentNode") - parents,
              connected: __blitsenDomCallCount("isConnected") - connected,
              order, clicks,
            });
          })()
        "#,
        );
        assert_eq!(result["target"], "target");
        assert_eq!(result["parents"], 0);
        assert_eq!(result["connected"], 0);
        assert_eq!(
            result["clicks"],
            serde_json::json!(["target:target", "outer:target"])
        );
        assert_eq!(
            result["order"],
            serde_json::json!([
                "pointermove:window:capture",
                "pointermove:document:capture",
                "pointermove:outer:capture",
                "pointermove:target:capture",
                "pointermove:target:bubble",
                "pointermove:outer:bubble",
                "pointermove:document:bubble",
                "pointermove:window:bubble",
                "mousemove:window:capture",
                "mousemove:document:capture",
                "mousemove:outer:capture",
                "mousemove:target:capture",
                "mousemove:target:bubble",
                "mousemove:outer:bubble",
                "mousemove:document:bubble",
                "mousemove:window:bubble",
            ])
        );
    }

    #[test]
    fn pointer_capture_retargets_and_recomputes_the_hit_path() {
        let (mut engine, _services) = realm(crate::dom_bridge::DocumentMode::TestHarness);
        let result = json(
            &mut engine,
            r#"
          (() => {
            const outer = document.getElementById("outer");
            const target = document.getElementById("target");
            const otherParent = document.getElementById("other-parent");
            const other = document.getElementById("other");
            target.addEventListener("pointerdown", event => other.setPointerCapture(event.pointerId));
            __blitsenInjectPointerAt("pointerdown", 10, 10, {
              pointerId: 1, pointerType: "mouse", isPrimary: true, button: 0,
            });
            const reached = [];
            for (const [node, name] of [[outer, "outer"], [target, "target"],
              [otherParent, "other-parent"], [other, "other"]]) {
              for (const type of ["pointermove", "mousemove"])
                node.addEventListener(type, event => reached.push(`${type}:${name}:${event.target.id}`));
            }
            const parents = __blitsenDomCallCount("parentNode");
            const connected = __blitsenDomCallCount("isConnected");
            __blitsenInjectPointerAt("pointermove", 10, 10, {
              pointerId: 1, pointerType: "mouse", isPrimary: true,
            });
            return JSON.stringify({
              parents: __blitsenDomCallCount("parentNode") - parents,
              connected: __blitsenDomCallCount("isConnected") - connected,
              reached,
            });
          })()
        "#,
        );
        assert!(result["parents"].as_u64().unwrap() > 0, "{result}");
        assert!(result["connected"].as_u64().unwrap() > 0, "{result}");
        assert_eq!(
            result["reached"],
            serde_json::json!([
                "pointermove:other:other",
                "pointermove:other-parent:other",
                "mousemove:other:other",
                "mousemove:other-parent:other",
            ])
        );
    }

    #[test]
    fn pointer_lock_retargets_and_recomputes_the_hit_path() {
        let (mut engine, services) = realm(crate::dom_bridge::DocumentMode::TestHarness);
        engine
            .evaluate_script(
                r#"
          const target = document.getElementById("target");
          target.addEventListener("pointerdown", () => target.requestPointerLock(), { once: true });
          __blitsenInjectPointerAt("pointerdown", 10, 10, {
            pointerId: 1, pointerType: "mouse", isPrimary: true, button: 0,
          });
        "#,
                "blitsen:test-pointer-path-lock",
            )
            .expect("pointer lock is requested from a trusted press");
        settle(&mut engine, &services);
        let result = json(
            &mut engine,
            r#"
          (() => {
            const outer = document.getElementById("outer");
            const target = document.getElementById("target");
            const otherParent = document.getElementById("other-parent");
            const other = document.getElementById("other");
            const reached = [];
            for (const [node, name] of [[outer, "outer"], [target, "target"],
              [otherParent, "other-parent"], [other, "other"]]) {
              for (const type of ["pointermove", "mousemove"])
                node.addEventListener(type, event => reached.push(`${type}:${name}:${event.target.id}`));
            }
            const parents = __blitsenDomCallCount("parentNode");
            const connected = __blitsenDomCallCount("isConnected");
            __blitsenInjectPointerAt("pointermove", 110, 10, {
              pointerId: 1, pointerType: "mouse", isPrimary: true,
            });
            return JSON.stringify({
              parents: __blitsenDomCallCount("parentNode") - parents,
              connected: __blitsenDomCallCount("isConnected") - connected,
              reached,
            });
          })()
        "#,
        );
        assert!(result["parents"].as_u64().unwrap() > 0, "{result}");
        assert!(result["connected"].as_u64().unwrap() > 0, "{result}");
        assert_eq!(
            result["reached"],
            serde_json::json!([
                "pointermove:target:target",
                "pointermove:outer:target",
                "mousemove:target:target",
                "mousemove:outer:target",
            ])
        );
    }

    #[test]
    fn pointer_lock_tasks_capture_escape_disconnect_and_reacquisition_are_ordered() {
        let (mut engine, services) = realm(crate::dom_bridge::DocumentMode::TestHarness);
        engine
            .evaluate_script(
                r#"
          const target = document.getElementById("target");
          const other = document.getElementById("other");
          globalThis.__seen = [];
          const record = value => __seen.push(value);
          document.addEventListener("pointerlockchange", () =>
            record(`change:${document.pointerLockElement?.id ?? "none"}`));
          document.addEventListener("pointerlockerror", () => record("error"));
          target.addEventListener("lostpointercapture", () => record("lost:active"));
          other.addEventListener("lostpointercapture", () => record("lost:pending"));
          target.addEventListener("mousemove", event =>
            record(`move:${event.movementX},${event.movementY}:${event.clientX},${event.clientY}`));
          target.addEventListener("pointerdown", () => {
            target.setPointerCapture(1);
          }, { once: true });
          const raw = Object.getOwnPropertySymbols(target)
            .map(symbol => target[symbol]).find(value => typeof value === "string");
          globalThis.__rawTarget = raw;
          __blitsenDispatchPointerEvent("pointerdown", raw, {
            pointerId: 1, pointerType: "mouse", isPrimary: true,
            clientX: 12, clientY: 14, screenX: 112, screenY: 114, button: 0,
          });
          __blitsenDispatchPointerEvent("pointermove", raw, {
            pointerId: 1, pointerType: "mouse", isPrimary: true,
            clientX: 12, clientY: 14, screenX: 112, screenY: 114,
          });
          target.addEventListener("pointerdown", () => {
            other.setPointerCapture(1);
            target.requestPointerLock().then(() => record("promise"));
            document.addEventListener("pointerlockchange", () => record("late"), { once: true });
          }, { once: true });
          __blitsenDispatchPointerEvent("pointerdown", raw, {
            pointerId: 1, pointerType: "mouse", isPrimary: true,
            clientX: 12, clientY: 14, screenX: 112, screenY: 114, button: 2,
          });
          record(`immediate:${document.pointerLockElement?.id ?? "none"}`);
        "#,
                "blitsen:test-pointer-lock",
            )
            .expect("the request is made");
        assert_eq!(seen(&mut engine), "move:0,0:12,14|immediate:none");
        settle(&mut engine, &services);
        assert_eq!(
            seen(&mut engine),
            "move:0,0:12,14|immediate:none|lost:active|lost:pending|change:target|late|promise"
        );

        engine.evaluate_script(r#"
          target.requestPointerLock().then(() => record("duplicate"));
          __blitsenDispatchLockedPointerMotion(7, -3);
          document.body.addEventListener("keydown", event => {
            if (event.key === "Escape") record(`escape:${document.pointerLockElement?.id ?? "none"}`);
          }, { once: true });
          __blitsenDispatchKeyboardEvent("keydown", { key: "Escape", code: "Escape" });
        "#, "blitsen:test-escape")
            .expect("Escape exits");
        engine.drain_microtasks().expect("duplicate resolves");
        settle(&mut engine, &services);
        assert_eq!(
            seen(&mut engine),
            "move:0,0:12,14|immediate:none|lost:active|lost:pending|change:target|late|promise|move:7,-3:12,14|escape:none|duplicate|change:none"
        );

        // An explicit exit does not poison a later acquisition, and no second
        // change is raised for a request naming the element already locked.
        engine
            .evaluate_script(
                r#"
          target.addEventListener("pointerdown", () =>
            target.requestPointerLock().then(() => record("reacquired")), { once: true });
          __blitsenDispatchPointerEvent("pointerdown", __rawTarget, {
            pointerId: 1, pointerType: "mouse", isPrimary: true, button: 0,
          });
        "#,
                "blitsen:test-reacquire",
            )
            .expect("a second gesture reacquires");
        settle(&mut engine, &services);
        engine
            .evaluate_script("document.exitPointerLock();", "blitsen:test-explicit-exit")
            .expect("explicit exit works");
        settle(&mut engine, &services);
        assert!(seen(&mut engine).ends_with("change:target|reacquired|change:none"));

        engine
            .evaluate_script(
                r#"
          target.addEventListener("pointerdown", () =>
            target.requestPointerLock().then(() => record("disconnect-request")), { once: true });
          __blitsenDispatchPointerEvent("pointerdown", __rawTarget, {
            pointerId: 1, pointerType: "mouse", isPrimary: true, button: 0,
          });
        "#,
                "blitsen:test-disconnect-lock",
            )
            .expect("third acquisition starts");
        settle(&mut engine, &services);
        engine
            .evaluate_script(
                r#"
          target.remove();
          record(`disconnected:${document.pointerLockElement?.id ?? "none"}`);
        "#,
                "blitsen:test-disconnect",
            )
            .expect("disconnect releases immediately");
        assert!(seen(&mut engine).ends_with("disconnect-request|disconnected:none"));
        settle(&mut engine, &services);
        assert!(seen(&mut engine).ends_with("disconnect-request|disconnected:none|change:none"));
    }

    #[test]
    fn mode_errors_and_fullscreen_events_precede_promise_settlement() {
        let (mut engine, services) = realm(crate::dom_bridge::DocumentMode::TestHarness);
        engine
            .evaluate_script(
                r#"
          const root = document.documentElement;
          const target = document.getElementById("target");
          globalThis.__seen = [];
          const record = value => __seen.push(value);
          target.requestPointerLock({ unadjustedMovement: true })
            .catch(error => record(`reject:${error.name}`));
          document.addEventListener("pointerlockerror", () => record("error"));
          document.body.addEventListener("keydown", () => {
            root.requestFullscreen().then(() => record("full:promise"));
            root.addEventListener("fullscreenchange", () => record("full:late"), { once: true });
          }, { once: true });
          __blitsenDispatchKeyboardEvent("keydown", { key: "Enter", code: "Enter" });
          record(`full:immediate:${document.fullscreenElement === null}`);
        "#,
                "blitsen:test-mode-task-order",
            )
            .expect("requests are queued");
        assert_eq!(seen(&mut engine), "full:immediate:true");
        settle(&mut engine, &services);
        assert_eq!(
            seen(&mut engine),
            "full:immediate:true|error|reject:NotSupportedError|full:late|full:promise"
        );

        engine.evaluate_script(r#"
          document.body.addEventListener("keydown", event => {
            if (event.key === "Escape") record(`full:escape:${document.fullscreenElement === null}`);
          }, { once: true });
          __blitsenDispatchKeyboardEvent("keydown", { key: "Escape", code: "Escape" });
        "#, "blitsen:test-fullscreen-escape")
            .expect("Escape exits fullscreen");
        settle(&mut engine, &services);
        assert!(seen(&mut engine).ends_with("full:escape:true"));
    }

    #[test]
    fn production_mode_dispatch_and_window_authority_are_not_globals() {
        let (mut engine, _services) = realm(crate::dom_bridge::DocumentMode::Application);
        let value = engine
            .evaluate_script(
                r#"
          ["__blitsenDispatchPointerEvent", "__blitsenDispatchKeyboardEvent",
           "__blitsenDispatchLockedPointerMotion", "__blitsenReleaseWindowModes",
           "__blitsenWindowMode"].every(name => !(name in globalThis))
        "#,
                "blitsen:test-private-host-hooks",
            )
            .expect("global privacy is testable");
        assert!(engine.to_boolean(&value).expect("the result is boolean"));
    }
}
