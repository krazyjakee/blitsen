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
            let supported = test_harness || cfg!(not(target_os = "android"));
            if action == "supported" {
                return Ok(engine.boolean(supported));
            }
            if !supported {
                return Err(JsError::new(
                    "pointer lock and the standard fullscreen API are not supported on Android",
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
    const SCRIPT: &str = r#"
        const root = document.documentElement;
        const target = document.getElementById("target");
        const seen = [];
        const record = value => { seen.push(value); root.setAttribute("data-seen", seen.join("|")); };
        document.addEventListener("pointerlockchange", () =>
            record(`lock:${document.pointerLockElement?.id ?? "none"}`));
        document.addEventListener("pointerlockerror", () => record("lock:error"));
        root.addEventListener("fullscreenchange", () =>
            record(`full:${document.fullscreenElement === root ? "root" : "none"}`));
        root.addEventListener("fullscreenerror", () => record("full:error"));
        target.addEventListener("mousemove", event =>
            record(`move:${event.movementX},${event.movementY}:${event.clientX},${event.clientY}`));

        target.addEventListener("pointerdown", () =>
          target.requestPointerLock({ unadjustedMovement: true }).then(() => record("lock:promise")),
          { once: true });
        const rawTarget = Object.getOwnPropertySymbols(target)
          .map(symbol => target[symbol]).find(value => typeof value === "string");
        __blitsenDispatchPointerEvent("pointerdown", rawTarget, {
          pointerId: 1, pointerType: "mouse", isPrimary: true,
          clientX: 12, clientY: 14, screenX: 112, screenY: 114, button: 0,
        });
        if (document.pointerLockElement !== target) throw new Error("pointer lock target was not published");
        __blitsenDispatchLockedPointerMotion(7, -3);
        __blitsenReleaseWindowModes(true, false, "synthetic-focus-loss");

        document.body.addEventListener("keydown", () =>
          root.requestFullscreen().then(() => record("full:promise")), { once: true });
        __blitsenDispatchKeyboardEvent("keydown", { key: "Enter", code: "Enter" });
        if (document.fullscreenElement !== root || !document.fullscreenEnabled)
          throw new Error("root fullscreen state was not published");
        __blitsenReleaseWindowModes(false, true, "synthetic-surface-loss");

        target.requestPointerLock().catch(error => record(`lock:reject:${error.name}`));
        requestAnimationFrame(() => record("frame"));
    "#;

    #[test]
    fn promises_events_raw_motion_and_lifecycle_release_are_ordered() {
        let mut engine = blitsen_quickjs::QuickJs::new().expect("an engine");
        let _services = crate::runtime_services::RuntimeServices::install(&mut engine)
            .expect("runtime services");
        let snapshots = crate::harness::execute_animation_harness(
            engine,
            "<!doctype html><html><body><div id='target'></div></body></html>".to_owned(),
            SCRIPT.to_owned(),
            1,
            200,
            100,
        )
        .expect("the window mode harness runs");
        let value = serde_json::to_value(&snapshots[0]).expect("snapshot serializes");
        let seen = value["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .find(|node| node["tag"] == "html")
            .and_then(|node| node["attributes"]["data-seen"].as_str())
            .expect("the script records its observations");
        assert_eq!(
            seen,
            "lock:target|move:7,-3:12,14|lock:none|full:root|full:none|lock:error|\
             frame|lock:promise|full:promise|lock:reject:NotAllowedError"
        );
    }
}
