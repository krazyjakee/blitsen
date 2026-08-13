//! Where the small engine stops winning.
//!
//! `frame` found QuickJS ahead of JavaScriptCore on pong, which is only
//! surprising until you notice what pong's frame is made of: almost no
//! JavaScript and four crossings into native code. This separates the two
//! costs and sweeps the ratio between them, so the answer is a threshold rather
//! than an anecdote — how much JavaScript per frame an app can do before the
//! JIT is the thing that matters.
use std::time::Instant;

use blitsen_js::{JsEngine, JsError};
use blitsen_jsc::JavaScriptCore;
use s8_quickjs::QuickJs;

fn install<E: JsEngine>(engine: &mut E) -> Result<(), JsError> {
    let writer = engine.define_function(
        "__write",
        Box::new(|call| {
            let mut engine = E::from_value(&call.this);
            let value = call.argument(0, "value")?;
            let text = engine.to_string(value)?;
            std::hint::black_box(text.len());
            Ok(engine.undefined())
        }),
    )?;
    engine.set_global("__write", &writer)
}

fn time<E: JsEngine>(engine: &mut E, source: &str, iterations: usize) -> f64 {
    engine.evaluate_script(source, "warmup").expect("warmup");
    let mut samples = Vec::new();
    for _ in 0..5 {
        let started = Instant::now();
        engine.evaluate_script(source, "bench").expect("bench");
        samples.push(started.elapsed().as_secs_f64() * 1e9 / iterations as f64);
    }
    samples.sort_by(f64::total_cmp);
    samples[2]
}

fn main() {
    let mut jsc = JavaScriptCore::load().expect("JavaScriptCore");
    let mut qjs = QuickJs::new().expect("quickjs");
    install(&mut jsc).expect("jsc stub");
    install(&mut qjs).expect("quickjs stub");

    println!("cost of one native callback crossing (string argument converted in Rust)");
    let crossing = "(() => { for (let i = 0; i < 200000; i++) __write('12.5px') })()";
    let jsc_call = time(&mut jsc, crossing, 200_000);
    let qjs_call = time(&mut qjs, crossing, 200_000);
    println!("  JavaScriptCore  {jsc_call:>7.0} ns");
    println!("  QuickJS-ng      {qjs_call:>7.0} ns   ({:.1}× {})",
        if qjs_call > jsc_call { qjs_call / jsc_call } else { jsc_call / qjs_call },
        if qjs_call > jsc_call { "slower" } else { "faster" });

    println!("\ncost of pure JavaScript work (one arithmetic + property unit)");
    let work = "(() => { const o = {x:0}; for (let i = 0; i < 1000000; i++) o.x = o.x + i % 7; return o.x })()";
    let jsc_op = time(&mut jsc, work, 1_000_000);
    let qjs_op = time(&mut qjs, work, 1_000_000);
    println!("  JavaScriptCore  {jsc_op:>7.1} ns");
    println!("  QuickJS-ng      {qjs_op:>7.1} ns   ({:.1}× slower)", qjs_op / jsc_op);

    println!("\na frame with 4 native writes and N units of JavaScript");
    println!("| JS units/frame | JavaScriptCore | QuickJS-ng | winner |");
    println!("| ---: | ---: | ---: | --- |");
    for units in [0usize, 100, 1_000, 10_000, 100_000] {
        let jsc_us = (4.0 * jsc_call + units as f64 * jsc_op) / 1000.0;
        let qjs_us = (4.0 * qjs_call + units as f64 * qjs_op) / 1000.0;
        let winner = if qjs_us < jsc_us {
            format!("QuickJS by {:.1}×", jsc_us / qjs_us)
        } else {
            format!("JSC by {:.1}×", qjs_us / jsc_us)
        };
        println!("| {units} | {jsc_us:.1} µs | {qjs_us:.1} µs | {winner} |");
    }

    // The number the product actually needs: how much JavaScript fits in a
    // frame before each engine has spent the whole 16.7 ms budget on it.
    println!("\nJavaScript units that fit in one 16.7 ms frame");
    println!("  JavaScriptCore  {:>12.0}", 16_700_000.0 / jsc_op);
    println!("  QuickJS-ng      {:>12.0}", 16_700_000.0 / qjs_op);
    let breakeven = 4.0 * (jsc_call - qjs_call) / (qjs_op - jsc_op);
    println!("\n  break-even: {breakeven:.0} JavaScript units per frame");
}
