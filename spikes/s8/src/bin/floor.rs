//! The smallest binary that still links the engine and proves it ran.
//!
//! The counterpart to `spikes/s0`'s `jsc-only` variant, built to be compared
//! with it: link the engine statically, execute real JavaScript through the
//! public API, and print the result so nothing can be dead-stripped away.
use blitsen_js::JsEngine;
use s8_quickjs::QuickJs;

fn main() {
    let mut engine = QuickJs::new().expect("runtime");
    let value = engine.evaluate_script("6 * 7", "floor").expect("eval");
    println!("js={}", engine.to_number(&value).expect("number"));
}
