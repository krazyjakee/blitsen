//! Test-only entry point for comparing the Rust and JavaScript bundle writers.

use std::path::Path;

fn main() {
    let mut arguments = std::env::args_os().skip(1);
    let runtime = arguments.next().expect("runtime path");
    let output = arguments.next().expect("output path");
    assert!(
        arguments.next().is_none(),
        "expected runtime and output paths"
    );
    blitsen_core::bundle::write_bundle(Path::new(&runtime), Path::new(&output), &[])
        .expect("write reference bundle");
}
