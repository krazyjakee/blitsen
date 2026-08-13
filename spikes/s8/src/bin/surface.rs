//! What the shipping engine does with the APIs the profile calls absent.
use blitsen_js::JsEngine;
use s8_quickjs::QuickJs;
fn main() {
    let mut e = QuickJs::new().unwrap();
    for probe in [
        "typeof Intl",
        "typeof WebAssembly",
        "(1234.5).toLocaleString()",
        "new Date(0).toLocaleDateString()",
        "'a'.localeCompare('b')",
        "(1234.5).toLocaleString('de-DE')",
        "typeof (0).toFixed",
    ] {
        match e.evaluate_script(probe, "surface") {
            Ok(v) => println!("  {probe:<36} => {}", e.to_string(&v).unwrap_or_default()),
            Err(err) => println!("  {probe:<36} => THREW: {}", err.message()),
        }
    }
}
