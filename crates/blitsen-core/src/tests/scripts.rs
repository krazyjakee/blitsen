use super::*;

/// A script read off disk is named by its path, which is the platform's own —
/// `\` on Windows. The identity under test is which file was reached, not which
/// separator the host spells it with, so both are compared in one spelling.
fn ends_with_path(identity: &str, suffix: &str) -> bool {
    identity.replace('\\', "/").ends_with(suffix)
}

#[test]
fn document_scripts_run_in_order_with_local_module_identity() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spikes/s7/fixture/index.html");
    let scripts = vec![
        DocumentScript {
            source: "globalThis.first = true".into(),
            src: None,
            script_type: None,
            async_attribute: false,
            defer_attribute: false,
        },
        DocumentScript {
            source: String::new(),
            src: Some("src/math.js".into()),
            script_type: Some("module".into()),
            async_attribute: true,
            defer_attribute: false,
        },
        DocumentScript {
            source: "ignored".into(),
            src: None,
            script_type: Some("application/json".into()),
            async_attribute: false,
            defer_attribute: false,
        },
    ];
    let mut engine = RecordingScriptEngine::default();
    assert_eq!(
        execute_collected_document_scripts_from(scripts, &mut engine, &fixture, &LocalScripts)
            .unwrap(),
        vec![1, 2]
    );
    assert_eq!(engine.evaluations[0].0, "classic");
    assert!(engine.evaluations[0].2.ends_with("index.html#script-1"));
    assert_eq!(engine.evaluations[1].0, "module");
    assert!(ends_with_path(&engine.evaluations[1].2, "src/math.js"));
    assert!(!engine.evaluations[1].1.is_empty());
}

#[test]
fn a_server_root_source_is_read_from_the_application_root() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spikes/s7/fixture/index.html");
    let script = |src: &str| {
        vec![DocumentScript {
            source: String::new(),
            src: Some(src.into()),
            script_type: Some("module".into()),
            async_attribute: false,
            defer_attribute: false,
        }]
    };
    // The leading slash is the application's root, not the filesystem's — the
    // meaning `blitsen build` rewrites it to and the application origin already
    // carries inside an export. A stock `vite build` emits nothing else.
    let mut engine = RecordingScriptEngine::default();
    execute_collected_document_scripts_from(
        script("/src/math.js"),
        &mut engine,
        &fixture,
        &LocalScripts,
    )
    .unwrap();
    assert!(ends_with_path(&engine.evaluations[0].2, "src/math.js"));
    assert!(!engine.evaluations[0].1.is_empty());

    // What it is not is a licence to read the disk. A path the application does
    // not ship is skipped, for the reason a remote one is: one source that does
    // not arrive must not stop every other script on the page.
    let mut engine = RecordingScriptEngine::default();
    execute_collected_document_scripts_from(
        script("/assets/app.js"),
        &mut engine,
        &fixture,
        &LocalScripts,
    )
    .unwrap();
    assert!(engine.evaluations.is_empty());
}

/// A remote script is skipped rather than fatal, so the rest of the document
/// still runs — one analytics tag used to stop every other script on the page.
#[test]
fn a_remote_script_skips_without_stopping_the_document() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixture/index.html");
    let script = |src: Option<&str>, source: &str| DocumentScript {
        source: source.into(),
        src: src.map(Into::into),
        script_type: None,
        async_attribute: false,
        defer_attribute: false,
    };
    for remote in ["https://example.com/gtag.js", "//cdn.example.com/a.js"] {
        let scripts = vec![script(Some(remote), ""), script(None, "1")];
        let mut engine = RecordingScriptEngine::default();
        assert_eq!(
            execute_collected_document_scripts_from(scripts, &mut engine, &fixture, &LocalScripts)
                .unwrap(),
            vec![1],
            "the inline script after {remote} still runs"
        );
        assert_eq!(engine.evaluations.len(), 1);
    }
}

/// The identifier a script evaluates under is handed back to a module resolver,
/// and Windows' `canonicalize` answers a spelling no resolver accepts. This is
/// what the Phase 1 host tripped over on Windows: `Cannot find module
/// '\\?\C:\…\module.js' from '\\?\C:\…\module.js'`, naming a file that was
/// plainly there (#134).
#[test]
fn a_canonical_path_is_simplified_where_the_platform_extends_it() {
    let ordinary = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spikes/s7/fixture/index.html");
    let canonical = ordinary.canonicalize().unwrap();
    let simple = crate::simplified(canonical.clone());
    // Same file either way, and nothing is dropped that a resolver needs.
    assert!(simple.is_file(), "{} is not a file", simple.display());
    assert!(!simple.to_string_lossy().starts_with(r"\\?\"));
    assert_eq!(crate::simplified(simple.clone()), simple, "idempotent");
    if cfg!(windows) {
        assert_eq!(simple, Path::new(&canonical.to_string_lossy()[4..]));
    } else {
        // Nothing to strip off Windows: the path is returned unchanged.
        assert_eq!(simple, canonical);
    }
}
