use super::*;

#[test]
fn document_scripts_run_in_order_with_local_module_identity() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spikes/s7/fixture/index.html");
    let document = MockScripts(vec![
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
    ]);
    let mut engine = RecordingScriptEngine::default();
    assert_eq!(
        execute_document_scripts(&document, &mut engine, &fixture).unwrap(),
        vec![1, 2]
    );
    assert_eq!(engine.evaluations[0].0, "classic");
    assert!(engine.evaluations[0].2.ends_with("index.html#script-1"));
    assert_eq!(engine.evaluations[1].0, "module");
    assert!(engine.evaluations[1].2.ends_with("src/math.js"));
    assert!(!engine.evaluations[1].1.is_empty());
}

#[test]
fn document_scripts_reject_a_server_root_source() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixture/index.html");
    let document = MockScripts(vec![DocumentScript {
        source: String::new(),
        src: Some("/assets/app.js".into()),
        script_type: None,
        async_attribute: false,
        defer_attribute: false,
    }]);
    let error =
        execute_document_scripts(&document, &mut RecordingScriptEngine::default(), &fixture)
            .unwrap_err();
    assert!(error.message().contains("must be relative"));
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
        let document = MockScripts(vec![script(Some(remote), ""), script(None, "1")]);
        let mut engine = RecordingScriptEngine::default();
        assert_eq!(
            execute_document_scripts(&document, &mut engine, &fixture).unwrap(),
            vec![1],
            "the inline script after {remote} still runs"
        );
        assert_eq!(engine.evaluations.len(), 1);
    }
}
