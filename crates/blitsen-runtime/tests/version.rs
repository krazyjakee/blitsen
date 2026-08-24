//! The executable and library reports share the distribution version boundary.

use std::process::Command;

#[test]
fn command_report_matches_the_runtime_identity() {
    let output = Command::new(env!("CARGO_BIN_EXE_blitsen-runtime"))
        .arg("--version")
        .output()
        .expect("run blitsen-runtime --version");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).expect("UTF-8 version output"),
        format!("{}\n", blitsen_core::runtime_identity())
    );
}
