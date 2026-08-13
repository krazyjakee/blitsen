//! Head-to-head: the same JavaScript, the same machine, two engines.
//!
//! A remembered benchmark number is not evidence, so this runs the identical
//! sources through the incumbent (JavaScriptCore, dynamically loaded exactly as
//! a Phase 2 export loads it) and the candidate (QuickJS-ng, statically linked).
//! P4 is the question behind it: 60 fps means a 16.7 ms frame budget, and what
//! matters is how much of that budget the JavaScript tier eats.
use std::time::Instant;

use blitsen_js::JsEngine;
use blitsen_jsc::JavaScriptCore;
use s8_quickjs::QuickJs;

const BENCHMARKS: &[(&str, &str)] = &[
    (
        "property + arithmetic, 3M",
        "(() => { const o = {x:0}; for (let i=0;i<3000000;i++) o.x = o.x + i % 7; return o.x })()",
    ),
    (
        "array alloc + sum, 300k",
        "(() => { let s=0; for (let i=0;i<300000;i++) { const a=[i,i+1,i+2]; s+=a[0]+a[1]+a[2] } return s })()",
    ),
    (
        "string building, 200k",
        "(() => { let s=''; for (let i=0;i<200000;i++) s = (s.length > 60 ? '' : s) + 'x'; return s.length })()",
    ),
    (
        "function calls, 2M",
        "(() => { const f = (a,b) => a+b; let s=0; for (let i=0;i<2000000;i++) s=f(s,1); return s })()",
    ),
    (
        "object churn, 500k",
        "(() => { let s=0; for (let i=0;i<500000;i++) { const p={x:i,y:i*2}; s+=p.x+p.y } return s })()",
    ),
    (
        "Math, 2M",
        "(() => { let s=0; for (let i=0;i<2000000;i++) s+=Math.sqrt(i)*Math.sin(i); return s })()",
    ),
];

/// Median of five, after one discarded warm-up: a JIT needs a lap before it is
/// showing its steady state, and reporting its first lap would flatter QuickJS.
fn time<E: JsEngine>(engine: &mut E, source: &str) -> f64 {
    engine.evaluate_script(source, "warmup").expect("warmup");
    let mut samples = Vec::new();
    for _ in 0..5 {
        let started = Instant::now();
        engine.evaluate_script(source, "bench").expect("bench");
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(f64::total_cmp);
    samples[2]
}

fn main() {
    let mut quickjs = QuickJs::new().expect("quickjs");
    let mut jsc = match JavaScriptCore::load() {
        Ok(engine) => engine,
        Err(error) => {
            eprintln!("no JavaScriptCore to compare against: {error}");
            std::process::exit(1);
        }
    };

    println!("| benchmark | JavaScriptCore | QuickJS-ng | ratio |");
    println!("| --- | ---: | ---: | ---: |");
    let mut ratios = Vec::new();
    for (name, source) in BENCHMARKS {
        let jsc_ms = time(&mut jsc, source);
        let qjs_ms = time(&mut quickjs, source);
        let ratio = qjs_ms / jsc_ms;
        ratios.push(ratio);
        println!("| {name} | {jsc_ms:.1} ms | {qjs_ms:.1} ms | {ratio:.1}× slower |");
    }
    ratios.sort_by(f64::total_cmp);
    println!("\nmedian ratio: {:.1}× slower", ratios[ratios.len() / 2]);
}
