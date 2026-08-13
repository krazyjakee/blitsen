//! Linking an application into this runtime and reading it back (issue #88).
//!
//! The pair the format depends on: `write_bundle` appends, and the shipped
//! executable reads what was appended out of itself at startup. Both halves run
//! here, against the real binary rather than a stub, because the reader has to
//! survive whatever else a linked executable happens to contain — including its
//! own copy of the format's magic number.

use std::path::{Path, PathBuf};
use std::process::Command;

fn runtime_binary() -> Option<PathBuf> {
    // `CARGO_BIN_EXE_` names the binary this test was built alongside.
    let path = PathBuf::from(env!("CARGO_BIN_EXE_blitsen-runtime"));
    path.is_file().then_some(path)
}

fn workspace_temp(name: &str) -> PathBuf {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/tmp")
        .join(format!("linked-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("temp directory");
    directory.join(name)
}

fn link(files: &[(String, Vec<u8>)], name: &str) -> Option<PathBuf> {
    let runtime = runtime_binary()?;
    let output = workspace_temp(name);
    blitsen_core::bundle::write_bundle(&runtime, &output, files).expect("link");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&output, std::fs::Permissions::from_mode(0o755))
            .expect("make the linked application executable");
    }
    Some(output)
}

fn sample() -> Vec<(String, Vec<u8>)> {
    vec![
        (
            "index.html".to_owned(),
            b"<!doctype html><body><main id=x>waiting</main>\
              <script src=\"assets/app.js\"></script></body>"
                .to_vec(),
        ),
        (
            "assets/app.js".to_owned(),
            b"document.querySelector('#x').textContent = 'linked'".to_vec(),
        ),
        (
            "blitsen.runtime.json".to_owned(),
            br#"{"width":640,"height":480,"title":"Linked"}"#.to_vec(),
        ),
    ]
}

#[test]
fn a_linked_executable_reports_the_application_it_carries() {
    let Some(application) = link(&sample(), "reported") else {
        eprintln!("skipping: the runtime binary was not built");
        return;
    };
    let output = Command::new(&application)
        .arg("--bundle-report")
        .output()
        .expect("run the linked application");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("the report is JSON");
    assert_eq!(report["bundled"], true);
    assert_eq!(report["verified"], true);
    assert_eq!(report["formatVersion"], 1);
    let files: Vec<&str> = report["files"]
        .as_array()
        .expect("files")
        .iter()
        .map(|file| file["path"].as_str().expect("path"))
        .collect();
    assert_eq!(
        files,
        ["assets/app.js", "blitsen.runtime.json", "index.html"]
    );
    assert_eq!(report["digest"].as_str().expect("digest").len(), 64);
}

#[test]
fn an_unlinked_runtime_reports_no_application_rather_than_a_damaged_one() {
    let Some(runtime) = runtime_binary() else {
        eprintln!("skipping: the runtime binary was not built");
        return;
    };
    let output = Command::new(&runtime)
        .arg("--bundle-report")
        .output()
        .expect("run the runtime");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("the report is JSON");
    assert_eq!(report["bundled"], false);
}

#[test]
fn a_linked_application_runs_its_scripts_out_of_the_executable() {
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        eprintln!("skipping: opening a window needs a display");
        return;
    }
    let mut files = sample();
    for (path, bytes) in &mut files {
        if path == "assets/app.js" {
            *bytes = b"const el = document.querySelector('#x');\
                       el.textContent = 'linked';\
                       console.log('script ran from the bundle, textContent =', el.textContent);"
                .to_vec();
        }
    }
    let Some(application) = link(&files, "running") else {
        eprintln!("skipping: the runtime binary was not built");
        return;
    };
    let output = Command::new(&application)
        .env("BLITSEN_STANDALONE_FRAMES", "2")
        .output()
        .expect("run the linked application");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}\n{stdout}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("script ran from the bundle, textContent = linked"),
        "the bundled script did not run: {stdout}"
    );
}
