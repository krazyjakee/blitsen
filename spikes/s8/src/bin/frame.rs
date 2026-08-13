//! What pong's own frame costs, on each engine.
//!
//! The synthetic loops in `compare` answer "how much slower is QuickJS"; this
//! answers the only question that decides P4: how much of a 16.7 ms frame does
//! the JavaScript tier actually take. It runs `examples/pong/game.js` unmodified
//! against a DOM stub whose style writes and `textContent` go through *native
//! callbacks*, because that is the shape of Blitsen's real bridge and the
//! callback boundary is where a small engine can lose.
use std::time::Instant;

use blitsen_js::{JsEngine, JsError};
use blitsen_jsc::JavaScriptCore;
use s8_quickjs::QuickJs;

/// A DOM small enough to be honest: every element property write lands in a
/// native callback, and nothing memoises what the real bridge would not.
const DOM_STUB: &str = r#"
function element() {
  const style = {};
  const node = { style, tagName: 'DIV' };
  for (const property of ['top', 'left']) {
    Object.defineProperty(style, property, {
      set(value) { __write(value) }, get() { return '0px' },
    });
  }
  Object.defineProperty(node, 'textContent', {
    set(value) { __write(value) }, get() { return '' },
  });
  node.setAttribute = (name, value) => __write(value);
  node.addEventListener = () => {};
  node.focus = () => {};
  node.classList = { add(){}, remove(){}, toggle(){} };
  return node;
}
globalThis.document = { getElementById: () => element(), body: element() };
globalThis.window = { addEventListener: () => {} };
globalThis.requestAnimationFrame = () => 0;
"#;

fn measure<E: JsEngine>(engine: &mut E, label: &str, game: &str) -> Result<(), JsError> {
    let writer = engine.define_function(
        "__write",
        Box::new(|call| {
            // A real bridge converts the value and hands it to Rust, so do the
            // same work: measuring an empty callback would measure nothing.
            let mut engine = E::from_value(&call.this);
            let value = call.argument(0, "value")?;
            let text = engine.to_string(value)?;
            std::hint::black_box(text.len());
            Ok(engine.undefined())
        }),
    )?;
    engine.set_global("__write", &writer)?;
    engine.evaluate_script(DOM_STUB, "dom-stub")?;
    engine.evaluate_script(game, "game.js")?;

    let frame = engine.evaluate_script("frame", "frame-lookup")?;
    // Start the round, so the ball moves and the collision path is exercised
    // rather than the idle branch.
    engine.evaluate_script("togglePlay()", "start")?;

    const FRAMES: usize = 2000;
    for index in 0..200 {
        let timestamp = engine.number(index as f64 * 16.7);
        engine.call(&frame, None, &[timestamp])?;
    }
    let started = Instant::now();
    for index in 0..FRAMES {
        let timestamp = engine.number((200 + index) as f64 * 16.7);
        engine.call(&frame, None, &[timestamp])?;
    }
    let elapsed = started.elapsed();
    let per_frame_us = elapsed.as_secs_f64() * 1e6 / FRAMES as f64;
    println!(
        "  {label:<16} {per_frame_us:>8.1} µs/frame   {:>6.3}% of a 16.7 ms budget",
        per_frame_us / 16_700.0 * 100.0
    );
    Ok(())
}

fn main() {
    let game = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/pong/game.js"),
    )
    .expect("examples/pong/game.js");

    println!("pong's own frame() — update + collision + native DOM writes\n");
    let mut jsc = JavaScriptCore::load().expect("JavaScriptCore");
    measure(&mut jsc, "JavaScriptCore", &game).expect("jsc frame");
    let mut quickjs = QuickJs::new().expect("quickjs");
    measure(&mut quickjs, "QuickJS-ng", &game).expect("quickjs frame");
}
