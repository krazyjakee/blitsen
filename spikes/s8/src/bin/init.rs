use std::time::Instant;
use blitsen_js::JsEngine;
use blitsen_jsc::JavaScriptCore;
use s8_quickjs::QuickJs;
fn main() {
    let mut s = Vec::new();
    for _ in 0..20 { let t = Instant::now(); let mut e = QuickJs::new().unwrap();
        e.evaluate_script("1", "i").unwrap(); s.push(t.elapsed().as_secs_f64()*1e6); }
    s.sort_by(f64::total_cmp); println!("  QuickJS-ng      {:>8.0} µs to a usable context", s[10]);
    let mut s = Vec::new();
    for _ in 0..20 { let t = Instant::now(); let mut e = JavaScriptCore::load().unwrap();
        e.evaluate_script("1", "i").unwrap(); s.push(t.elapsed().as_secs_f64()*1e6); }
    s.sort_by(f64::total_cmp); println!("  JavaScriptCore  {:>8.0} µs to a usable context (library already resident)", s[10]);
}
