use blitsen_quickjs::QuickJs;

use super::*;

fn ime_document(html: &str) -> (QuickJs, Rc<RefCell<BlitzDom>>) {
    let dom = BlitzDom::from_html(
        html,
        DocumentConfig {
            viewport: Some(Viewport::new(400, 160, 1.0, ColorScheme::Light)),
            ..Default::default()
        },
    );
    let runtime = DomRuntime::new(dom);
    let document = runtime.document();
    let mut engine = QuickJs::new().expect("a QuickJS runtime");
    let _services = crate::runtime_services::RuntimeServices::install(&mut engine)
        .expect("the embedded runtime services install");
    dom_bridge::install(
        &mut engine,
        runtime,
        InstallOptions::new(400, 160, 1.0, DocumentMode::TestHarness, None),
    )
    .expect("the DOM bridge installs");
    document
        .borrow_mut()
        .flush_layout()
        .expect("the controls lay out");
    (engine, document)
}

fn json_result(engine: &mut QuickJs, script: &str) -> serde_json::Value {
    let result = engine
        .evaluate_script(script, "blitsen:test-result")
        .and_then(|value| engine.to_string(&value))
        .expect("the test script evaluates");
    serde_json::from_str(&result).expect("the test result is JSON")
}

#[test]
fn resize_observers_measure_all_targets_in_one_bridge_call_per_frame() {
    let markup = (0..100)
        .map(|index| format!("<div id=target-{index}></div>"))
        .collect::<String>();
    let (mut engine, _) = ime_document(&markup);
    let result = json_result(
        &mut engine,
        r#"
                const targets = [...document.querySelectorAll("div")];
                const deliveries = [];
                const observer = new ResizeObserver(entries => deliveries.push(entries.length));
                for (const target of targets) observer.observe(target);
                __blitsenAnimationFrameTick(0);
                __blitsenAnimationFrameTick(16);
                JSON.stringify({
                  deliveries,
                  batches: __blitsenDomCallCount("resizeObserverMetrics"),
                  individualMetrics: __blitsenDomCallCount("layoutMetrics"),
                  connectedChecks: __blitsenDomCallCount("isConnected"),
                });
            "#,
    );

    assert_eq!(result["deliveries"], serde_json::json!([100]));
    assert_eq!(result["batches"], 2, "one metrics batch is made per frame");
    assert_eq!(result["individualMetrics"], 0);
    assert_eq!(result["connectedChecks"], 0);
}

#[test]
fn batched_resize_observation_preserves_boxes_targets_and_animation_changes() {
    let (mut engine, _) = ime_document(
        r#"<style>
                 @keyframes grow { from { width: 20px } to { width: 60px } }
                 #static { box-sizing: content-box; width: 40px; height: 20px;
                           padding: 3px; border: 2px solid black }
                 #second { width: 30px; height: 10px }
                 #animated { width: 20px; height: 10px; animation: grow 1s linear both }
               </style>
               <div id=static></div><div id=second></div><div id=animated></div>"#,
    );
    let result = json_result(
        &mut engine,
        r#"
                const fixed = document.getElementById("static");
                const second = document.getElementById("second");
                const animated = document.getElementById("animated");
                const deliveries = [];
                const record = name => entries => deliveries.push([name, entries.map(entry => ({
                  id: entry.target.id,
                  content: [entry.contentRect.width, entry.contentRect.height],
                  contentBox: [entry.contentBoxSize[0].inlineSize,
                               entry.contentBoxSize[0].blockSize],
                  borderBox: [entry.borderBoxSize[0].inlineSize,
                              entry.borderBoxSize[0].blockSize],
                }))]);
                const content = new ResizeObserver(record("content"));
                const border = new ResizeObserver(record("border"));
                content.observe(fixed);
                content.observe(second);
                content.observe(animated);
                border.observe(fixed, { box: "border-box" });

                __blitsenAnimationFrameTick(0);
                fixed.style.borderWidth = "4px";
                __blitsenAnimationFrameTick(0);
                fixed.style.width = "50px";
                second.style.width = "35px";
                __blitsenAnimationFrameTick(0);

                content.unobserve(second);
                second.style.width = "70px";
                __blitsenAnimationFrameTick(0);
                fixed.remove();
                fixed.style.width = "80px";
                __blitsenAnimationFrameTick(0);

                // No DOM mutation occurs between these frames. Advancing only
                // the CSS animation clock must still produce new geometry.
                __blitsenAnimationFrameTick(500);
                const beforeDisconnect = deliveries.length;
                content.disconnect();
                border.disconnect();
                __blitsenAnimationFrameTick(750);
                JSON.stringify({ deliveries, beforeDisconnect,
                  afterDisconnect: deliveries.length });
            "#,
    );

    assert_eq!(
        result["deliveries"],
        serde_json::json!([
            ["content", [
                {"id":"static", "content":[40,20], "contentBox":[40,20], "borderBox":[50,30]},
                {"id":"second", "content":[30,10], "contentBox":[30,10], "borderBox":[30,10]},
                {"id":"animated", "content":[20,10], "contentBox":[20,10], "borderBox":[20,10]}
            ]],
            ["border", [
                {"id":"static", "content":[40,20], "contentBox":[40,20], "borderBox":[50,30]}
            ]],
            ["border", [
                {"id":"static", "content":[40,20], "contentBox":[40,20], "borderBox":[54,34]}
            ]],
            ["content", [
                {"id":"static", "content":[50,20], "contentBox":[50,20], "borderBox":[64,34]},
                {"id":"second", "content":[35,10], "contentBox":[35,10], "borderBox":[35,10]}
            ]],
            ["border", [
                {"id":"static", "content":[50,20], "contentBox":[50,20], "borderBox":[64,34]}
            ]],
            ["content", [
                {"id":"animated", "content":[40,10], "contentBox":[40,10], "borderBox":[40,10]}
            ]]
        ])
    );
    assert_eq!(result["beforeDisconnect"], result["afterDisconnect"]);
}

#[test]
fn element_view_preserves_tree_and_attribute_order() {
    let mut document = BlitzDom::from_html(
        "<main data-last=2 id=first>one<span class=inner>two</span></main>",
        DocumentConfig {
            viewport: Some(Viewport::new(320, 200, 1.0, ColorScheme::Light)),
            ..Default::default()
        },
    );
    let layout = document.flush_layout().expect("the fixture lays out");
    let mut seen = Vec::new();
    visit_elements(&document, |element| {
        if matches!(element.tag(), "main" | "span") {
            let attributes = element
                .attributes()
                .map(|attribute| {
                    (
                        attribute.name.local.to_string(),
                        attribute.value.to_string(),
                    )
                })
                .collect::<Vec<_>>();
            seen.push((
                element.tag().to_owned(),
                attributes,
                element.inline_style()?,
                element.text_content()?,
                element.bounding_rect(layout)?,
            ));
        }
        Ok(())
    })
    .expect("the elements resolve");

    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].0, "main");
    assert_eq!(
        seen[0].1,
        [
            ("data-last".into(), "2".into()),
            ("id".into(), "first".into())
        ]
    );
    assert_eq!(seen[0].2, "");
    assert_eq!(seen[0].3, "onetwo");
    assert!(seen[0].4.width > 0.0);
    assert_eq!(seen[1].0, "span");
    assert_eq!(seen[1].3, "two");
}

#[test]
fn node_wrapper_cache_is_weak_identity_storage() {
    let (mut engine, document) = ime_document("<body></body>");
    let baseline_nodes = document.borrow().document_ref().tree().len();
    let identity = engine
        .evaluate_script(
            r#"
                globalThis.wrapperProbe = document.createElement("section");
                wrapperProbe.id = "wrapper-probe";
                document.body.appendChild(wrapperProbe);
                globalThis.oldWrapperWeak = new WeakRef(wrapperProbe);
                globalThis.oldCacheProbe = __blitsenWrapperCacheProbe(wrapperProbe);
                wrapperProbe === document.getElementById("wrapper-probe");
                "#,
            "blitsen:test-wrapper-identity",
        )
        .and_then(|value| engine.to_boolean(&value))
        .unwrap();
    assert!(identity, "a live node keeps strict wrapper identity");
    let node = document
        .borrow()
        .get_element_by_id("wrapper-probe")
        .unwrap()
        .unwrap();

    engine
        .evaluate_script(
            "globalThis.wrapperProbe = null",
            "blitsen:test-drop-first-wrapper",
        )
        .unwrap();
    engine.collect_garbage().unwrap();

    // Install a replacement before deliberately delivering the old cache
    // cleanup token. It must not evict the new generation.
    let race = engine
        .evaluate_script(
            r#"
                const oldCollected = oldWrapperWeak.deref() === undefined;
                globalThis.wrapperProbe = document.getElementById("wrapper-probe");
                globalThis.newWrapperWeak = new WeakRef(wrapperProbe);
                __blitsenFinalizeWrapperCacheEntry(oldCacheProbe);
                const sameReplacement = wrapperProbe === document.getElementById("wrapper-probe");
                JSON.stringify({ oldCollected, sameReplacement });
                "#,
            "blitsen:test-wrapper-stale-finalizer",
        )
        .and_then(|value| engine.to_string(&value))
        .unwrap();
    let race: serde_json::Value = serde_json::from_str(&race).unwrap();
    assert_eq!(
        race,
        serde_json::json!({
            "oldCollected": true,
            "sameReplacement": true,
        })
    );

    engine
        .evaluate_script(
            r#"
                wrapperProbe.remove();
                globalThis.wrapperProbe = null;
                globalThis.oldCacheProbe = null;
                "#,
            "blitsen:test-drop-replacement",
        )
        .unwrap();
    for _ in 0..3 {
        engine.collect_garbage().unwrap();
        engine.drain_microtasks().unwrap();
    }
    let collected = engine
        .evaluate_script(
            "newWrapperWeak.deref() === undefined",
            "blitsen:test-wrapper-collected",
        )
        .and_then(|value| engine.to_boolean(&value))
        .unwrap();
    assert!(collected, "the weak cache does not retain the wrapper");
    assert!(
        document.borrow().node_kind(node).is_err(),
        "finalization releases the detached arena node"
    );

    engine
        .evaluate_script(
            r#"
                globalThis.newWrapperWeak = null;
                globalThis.churnWeak = [];
                (() => {
                  const body = document.body;
                  for (let index = 0; index < 512; index++) {
                    const child = document.createElement("i");
                    body.appendChild(child);
                    churnWeak.push(new WeakRef(child));
                    child.remove();
                  }
                })();
                "#,
            "blitsen:test-wrapper-churn",
        )
        .unwrap();
    for _ in 0..3 {
        engine.collect_garbage().unwrap();
        engine.drain_microtasks().unwrap();
    }
    let churn = engine
        .evaluate_script(
            r#"
                const wrappersCollected = churnWeak.every(reference =>
                  reference.deref() === undefined);
                globalThis.churnWeak = null;
                JSON.stringify({ wrappersCollected,
                  cacheEntries: __blitsenWrapperCacheSize() });
                "#,
            "blitsen:test-wrapper-churn-result",
        )
        .and_then(|value| engine.to_string(&value))
        .unwrap();
    let churn: serde_json::Value = serde_json::from_str(&churn).unwrap();
    assert_eq!(
        churn,
        serde_json::json!({ "wrappersCollected": true, "cacheEntries": 0 })
    );
    assert_eq!(
        document.borrow().document_ref().tree().len(),
        baseline_nodes,
        "wrapper churn leaves neither cache entries nor arena nodes"
    );
}

#[test]
fn node_results_choose_interfaces_without_description_calls() {
    let (mut engine, _) = ime_document(
        r#"<main id=scope>
                 <button id=button class=target>go</button>
                 <canvas class=target></canvas>
                 <span id=content>text<!--note--></span>
               </main>"#,
    );
    engine
        .evaluate_script(
            r##"
                const kinds = __blitsenDomCallCount("kind");
                const tags = __blitsenDomCallCount("tagName");
                const queries = __blitsenDomCallCount("querySelectorAll");
                const queried = document.querySelectorAll(".target");
                const queryCalls = __blitsenDomCallCount("querySelectorAll") - queries;
                const scope = document.getElementById("scope");
                const content = scope.querySelector("#content");
                const children = content.childNodes;
                const added = document.createElement("input");
                const descriptionKindCalls = __blitsenDomCallCount("kind") - kinds;
                const descriptionTagCalls = __blitsenDomCallCount("tagName") - tags;
                globalThis.nodeDescriptionResult = {
                  interfaces: [
                    queried[0] instanceof HTMLButtonElement,
                    queried[1] instanceof HTMLCanvasElement,
                    children[0] instanceof Text,
                    children[1] instanceof Comment,
                    added instanceof HTMLInputElement,
                  ],
                  strictIdentity: queried[0] === document.getElementById("button"),
                  mutationIdentity: false,
                  queryCalls,
                  kindCalls: descriptionKindCalls,
                  tagCalls: descriptionTagCalls,
                };
                new MutationObserver(records => {
                  nodeDescriptionResult.mutationIdentity =
                    records[0].addedNodes[0] === added && scope.lastChild === added;
                }).observe(scope, { childList: true });
                scope.appendChild(added);
                "##,
            "blitsen:test-node-result-descriptions",
        )
        .unwrap();
    engine.drain_microtasks().unwrap();
    let result = engine
        .evaluate_script(
            "JSON.stringify(nodeDescriptionResult)",
            "blitsen:test-node-result-descriptions-result",
        )
        .and_then(|value| engine.to_string(&value))
        .unwrap();
    let result: serde_json::Value = serde_json::from_str(&result).unwrap();

    assert_eq!(
        result,
        serde_json::json!({
            "interfaces": [true, true, true, true, true],
            "strictIdentity": true,
            "mutationIdentity": true,
            "queryCalls": 1,
            "kindCalls": 0,
            "tagCalls": 0,
        })
    );
}

#[test]
fn record_frame_owns_the_filename_png_and_write_error() {
    let directory = tempfile::tempdir().expect("a scratch directory");

    let pixels = [0x12, 0x34, 0x56, 0xff];
    let path = record_frame(directory.path(), 12, &pixels, 1, 1).expect("the frame records");
    assert_eq!(
        path.file_name().and_then(|name| name.to_str()),
        Some("frame-00012.png")
    );
    assert!(
        std::fs::read(&path)
            .expect("the PNG is readable")
            .starts_with(&[0x89, b'P', b'N', b'G'])
    );

    let blocker = directory.path().join("not-a-directory");
    std::fs::write(&blocker, []).expect("the blocker is written");
    let error = record_frame(&blocker, 13, &pixels, 1, 1).expect_err("writing below a file fails");
    assert!(error.message().starts_with("could not record frame 13:"));
}

#[test]
fn canvas_stream_reuses_grown_storage_and_preserves_submissions() {
    let (mut engine, _) = ime_document("<canvas id=surface width=32 height=16></canvas>");
    let result = engine
        .evaluate_script(
            r##"
                const surface = document.getElementById("surface");
                const context = surface.getContext("2d");
                const stream = context._stream;
                const numbers = stream.numbers;
                const strings = stream.strings;
                const sources = stream.sources;
                const sourceIndices = stream.sourceIndices;
                const pixelChunks = stream.pixels;

                // Reset clears every piece of per-batch state without replacing
                // the storage that a subsequent frame can reuse.
                stream.text("stale");
                stream.element(surface);
                stream.imageData(new ImageData(1, 1));
                stream.reset();
                const resetState = {
                  sameNumbers: stream.numbers === numbers,
                  sameStrings: stream.strings === strings,
                  sameSources: stream.sources === sources,
                  sameSourceIndices: stream.sourceIndices === sourceIndices,
                  samePixelChunks: stream.pixels === pixelChunks,
                  length: stream.length,
                  strings: stream.strings.length,
                  sources: stream.sources.length,
                  sourceIndices: stream.sourceIndices.size,
                  pixelChunks: stream.pixels.length,
                  pixelLength: stream.pixelLength,
                };

                context.fillStyle = "#ff0000";
                for (let index = 0; index < 80; index++) context.fillRect(0, 0, 32, 16);
                context.fillText("first batch", 1, 10);
                const grownNumbers = stream.numbers;
                const grownCapacity = grownNumbers.length;
                context._flush();
                const afterFirst = {
                  grew: grownCapacity > 1024,
                  sameNumbers: stream.numbers === grownNumbers,
                  sameStrings: stream.strings === strings,
                  empty: stream.length === 0 && stream.strings.length === 0,
                };

                for (let index = 0; index < 80; index++) context.fillRect(0, 0, 32, 16);
                context.fillText("second batch", 1, 10);
                const reusedBeforeSubmit = stream.numbers === grownNumbers
                  && stream.numbers.length === grownCapacity;
                context._flush();
                const pixel = Array.from(context.getImageData(20, 8, 1, 1).data);
                JSON.stringify({
                  resetState,
                  afterFirst,
                  reusedBeforeSubmit,
                  reusedAfterSubmit: stream.numbers === grownNumbers
                    && stream.numbers.length === grownCapacity,
                  pixel,
                });
                "##,
            "blitsen:test-canvas-stream-reuse",
        )
        .and_then(|value| engine.to_string(&value))
        .unwrap();
    let result: serde_json::Value = serde_json::from_str(&result).unwrap();

    assert_eq!(
        result,
        serde_json::json!({
            "resetState": {
                "sameNumbers": true,
                "sameStrings": true,
                "sameSources": true,
                "sameSourceIndices": true,
                "samePixelChunks": true,
                "length": 0,
                "strings": 0,
                "sources": 0,
                "sourceIndices": 0,
                "pixelChunks": 0,
                "pixelLength": 0,
            },
            "afterFirst": {
                "grew": true,
                "sameNumbers": true,
                "sameStrings": true,
                "empty": true,
            },
            "reusedBeforeSubmit": true,
            "reusedAfterSubmit": true,
            "pixel": [255, 0, 0, 255],
        })
    );
}

#[test]
fn synthetic_ime_events_keep_dom_order_state_and_is_composing() {
    let (mut engine, _) = ime_document("<input id=field>");
    engine
        .evaluate_script(
            r#"
                globalThis.field = document.getElementById("field");
                globalThis.imeLog = [];
                for (const type of ["compositionstart", "compositionupdate", "beforeinput",
                                    "input", "compositionend"]) {
                  field.addEventListener(type, event => imeLog.push({
                    type: event.type,
                    data: event.data,
                    inputType: event instanceof InputEvent ? event.inputType : "",
                    isComposing: event instanceof InputEvent ? event.isComposing : null,
                    value: field.value,
                  }));
                }
                field.focus();
                "#,
            "blitsen:test-ime-listeners",
        )
        .unwrap();
    for script in [
        r#"__blitsenDispatchImeEvent("preedit", { data: "🙂", cursorStart: 4, cursorEnd: 4 })"#,
        r#"__blitsenDispatchImeEvent("preedit", { data: "🙂🙂", cursorStart: 4, cursorEnd: 8 })"#,
        r#"__blitsenDispatchImeEvent("commit", { data: "🙂" })"#,
    ] {
        engine
            .evaluate_script(script, "blitsen:test-ime-event")
            .unwrap();
    }
    let result = engine
        .evaluate_script(
            "JSON.stringify({ log: imeLog, value: field.value, start: field.selectionStart })",
            "blitsen:test-ime-result",
        )
        .and_then(|value| engine.to_string(&value))
        .unwrap();
    let result: serde_json::Value = serde_json::from_str(&result).unwrap();

    assert_eq!(
        result["log"],
        serde_json::json!([
            {"type":"compositionstart", "data":"", "inputType":"", "isComposing":null, "value":""},
            {"type":"compositionupdate", "data":"🙂", "inputType":"", "isComposing":null, "value":""},
            {"type":"beforeinput", "data":"🙂", "inputType":"insertCompositionText", "isComposing":true, "value":""},
            {"type":"input", "data":"🙂", "inputType":"insertCompositionText", "isComposing":true, "value":"🙂"},
            {"type":"compositionupdate", "data":"🙂🙂", "inputType":"", "isComposing":null, "value":"🙂"},
            {"type":"beforeinput", "data":"🙂🙂", "inputType":"insertCompositionText", "isComposing":true, "value":"🙂"},
            {"type":"input", "data":"🙂🙂", "inputType":"insertCompositionText", "isComposing":true, "value":"🙂🙂"},
            {"type":"beforeinput", "data":"🙂", "inputType":"insertFromComposition", "isComposing":true, "value":"🙂🙂"},
            {"type":"input", "data":"🙂", "inputType":"insertFromComposition", "isComposing":true, "value":"🙂"},
            {"type":"compositionend", "data":"🙂", "inputType":"", "isComposing":null, "value":"🙂"},
        ])
    );
    assert_eq!(result["value"], "🙂");
    assert_eq!(result["start"], 2, "DOM selection offsets remain UTF-16");
}

#[test]
fn focus_change_cancels_preedit_and_readonly_never_starts_one() {
    let (mut engine, _) =
        ime_document("<input id=field><input id=locked readonly><textarea id=notes></textarea>");
    let result = engine
            .evaluate_script(
                r#"
                const field = document.getElementById("field");
                const locked = document.getElementById("locked");
                const notes = document.getElementById("notes");
                const ended = [];
                field.addEventListener("compositionend", event => ended.push(event.data));
                field.focus();
                __blitsenDispatchImeEvent("preedit", { data: "draft", cursorStart: 5, cursorEnd: 5 });
                notes.focus();
                locked.focus();
                const readonlyHandled = __blitsenDispatchImeEvent("preedit",
                  { data: "blocked", cursorStart: 7, cursorEnd: 7 });
                notes.focus();
                __blitsenDispatchImeEvent("preedit", { data: "ok", cursorStart: 2, cursorEnd: 2 });
                __blitsenDispatchImeEvent("commit", { data: "ok" });
                field.focus();
                __blitsenDispatchImeEvent("preedit",
                  { data: "cancel", cursorStart: 6, cursorEnd: 6 });
                __blitsenDispatchImeEvent("preedit", { data: "" });
                JSON.stringify({ field: field.value, notes: notes.value, locked: locked.value,
                                 ended, readonlyHandled });
                "#,
                "blitsen:test-ime-focus",
            )
            .and_then(|value| engine.to_string(&value))
            .unwrap();
    let result: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(
        result,
        serde_json::json!({
            "field": "",
            "notes": "ok",
            "locked": "",
            "ended": ["", ""],
            "readonlyHandled": false,
        })
    );
}

#[test]
fn keydown_default_action_inserts_printable_text_once() {
    let (mut engine, _) = ime_document("<input id=field>");
    let value = engine
        .evaluate_script(
            r#"
                const field = document.getElementById("field");
                field.focus();
                __blitsenDispatchKeyboardEvent("keydown",
                  { key: "a", code: "KeyA", bubbles: true, cancelable: true });
                field.value;
                "#,
            "blitsen:test-keydown-default-action",
        )
        .and_then(|value| engine.to_string(&value))
        .unwrap();
    assert_eq!(value, "a");
}

#[test]
fn text_history_restores_unicode_values_selections_and_ime_transactions() {
    let (mut engine, _) =
        ime_document(r#"<input id=field value="A🙂B"><textarea id=notes>line</textarea>"#);
    let result = engine
        .evaluate_script(
            r#"
                const field = document.getElementById("field");
                const notes = document.getElementById("notes");
                const historyLog = [];
                for (const type of ["beforeinput", "input"]) {
                  notes.addEventListener(type, event => {
                    if (event.inputType.startsWith("history")) historyLog.push({
                      type, inputType: event.inputType, data: event.data,
                      isComposing: event.isComposing, value: notes.value,
                      start: notes.selectionStart, end: notes.selectionEnd,
                      direction: notes.selectionDirection,
                    });
                  });
                }
                const key = (key, options = {}) => __blitsenDispatchKeyboardEvent("keydown",
                  { key, code: `Key${key.toUpperCase()}`, ...options });

                notes.focus();
                notes.value = "A🙂B";
                notes.setSelectionRange(1, 3, "backward");
                __blitsenDispatchImeEvent("preedit",
                  { data: "n", cursorStart: 1, cursorEnd: 1 });
                __blitsenDispatchImeEvent("preedit",
                  { data: "ni", cursorStart: 2, cursorEnd: 2 });
                __blitsenDispatchImeEvent("commit", { data: "你" });
                const committed = { value: notes.value, start: notes.selectionStart,
                                    end: notes.selectionEnd };
                key("z", { ctrlKey: true });
                const undone = { value: notes.value, start: notes.selectionStart,
                                 end: notes.selectionEnd,
                                 direction: notes.selectionDirection };
                key("z", { metaKey: true, shiftKey: true });
                const redone = { value: notes.value, start: notes.selectionStart,
                                 end: notes.selectionEnd };

                field.focus();
                field.setSelectionRange(1, 3, "forward");
                key("界");
                key("z", { ctrlKey: true });
                const inputUndone = { value: field.value, start: field.selectionStart,
                                     end: field.selectionEnd,
                                     direction: field.selectionDirection };
                JSON.stringify({ committed, undone, redone, inputUndone, historyLog });
                "#,
            "blitsen:test-text-history-unicode",
        )
        .and_then(|value| engine.to_string(&value))
        .unwrap();
    let result: serde_json::Value = serde_json::from_str(&result).unwrap();

    assert_eq!(
        result["committed"],
        serde_json::json!({"value":"A你B", "start":2, "end":2})
    );
    assert_eq!(
        result["undone"],
        serde_json::json!({"value":"A🙂B", "start":1, "end":3, "direction":"backward"})
    );
    assert_eq!(
        result["redone"],
        serde_json::json!({"value":"A你B", "start":2, "end":2})
    );
    assert_eq!(
        result["inputUndone"],
        serde_json::json!({"value":"A🙂B", "start":1, "end":3, "direction":"forward"})
    );
    assert_eq!(
        result["historyLog"],
        serde_json::json!([
            {"type":"beforeinput", "inputType":"historyUndo", "data":null,
             "isComposing":false, "value":"A你B", "start":2, "end":2, "direction":"none"},
            {"type":"input", "inputType":"historyUndo", "data":null,
             "isComposing":false, "value":"A🙂B", "start":1, "end":3,
             "direction":"backward"},
            {"type":"beforeinput", "inputType":"historyRedo", "data":null,
             "isComposing":false, "value":"A🙂B", "start":1, "end":3,
             "direction":"backward"},
            {"type":"input", "inputType":"historyRedo", "data":null,
             "isComposing":false, "value":"A你B", "start":2, "end":2, "direction":"none"},
        ])
    );
}

#[test]
fn text_history_has_control_local_bounded_and_controlled_value_boundaries() {
    let (mut engine, _) =
        ime_document("<input id=field><input id=other><textarea id=bounded></textarea>");
    let result = engine
        .evaluate_script(
            r#"
                const field = document.getElementById("field");
                const other = document.getElementById("other");
                const bounded = document.getElementById("bounded");
                const key = (key, options = {}) => __blitsenDispatchKeyboardEvent("keydown",
                  { key, code: `Key${key.toUpperCase()}`, ...options });

                // A controlled component's same-value echo retains history.
                field.addEventListener("input", event => {
                  if (event.inputType === "insertText") field.value = field.value;
                });
                let cancelHistory = true;
                field.addEventListener("beforeinput", event => {
                  if (cancelHistory && event.inputType === "historyUndo") event.preventDefault();
                });
                field.focus();
                key("a");
                key("b");
                key("z", { ctrlKey: true });
                const canceledUndo = field.value;
                cancelHistory = false;
                other.focus();
                field.focus();
                key("z", { ctrlKey: true });
                const focusRetained = field.value;
                key("y", { ctrlKey: true });
                const ctrlYRedone = field.value;
                key("z", { ctrlKey: true });
                key("x");
                key("y", { ctrlKey: true });
                const branchClearedRedo = field.value;
                field.value = "controlled";
                key("z", { ctrlKey: true });
                const replacementClearedHistory = field.value;

                bounded.focus();
                for (let index = 0; index < 105; index++) key("x");
                for (let index = 0; index < 101; index++) key("z", { ctrlKey: true });
                JSON.stringify({ canceledUndo, focusRetained, ctrlYRedone, branchClearedRedo,
                                 replacementClearedHistory, bounded: bounded.value.length });
                "#,
            "blitsen:test-text-history-boundaries",
        )
        .and_then(|value| engine.to_string(&value))
        .unwrap();
    let result: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(
        result,
        serde_json::json!({
            "canceledUndo": "ab",
            "focusRetained": "a",
            "ctrlYRedone": "ab",
            "branchClearedRedo": "ax",
            "replacementClearedHistory": "controlled",
            "bounded": 5,
        })
    );
}
