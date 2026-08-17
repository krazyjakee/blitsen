use super::*;

/// A sheet is its element's text, so the split has to be over source rather
/// than over anything reserialized, and it has to survive the punctuation
/// that appears inside a rule.
#[test]
fn a_sheet_splits_into_the_rules_a_browser_would_report() {
    let rules = BlitzDom::split_css_rules(
        r#"@charset "utf-8";
               /* a comment between rules belongs to no rule */
               a { content: "}" }
               @media (min-width: 10px) { b { color: red } c { color: blue } }
               @keyframes slide { from { left: 0 } to { left: 1px } }
               d[title='{'] { color: green }"#,
    );
    assert_eq!(
        rules,
        vec![
            r#"@charset "utf-8";"#.to_owned(),
            r#"a { content: "}" }"#.to_owned(),
            "@media (min-width: 10px) { b { color: red } c { color: blue } }".to_owned(),
            "@keyframes slide { from { left: 0 } to { left: 1px } }".to_owned(),
            "d[title='{'] { color: green }".to_owned(),
        ]
    );
    assert!(BlitzDom::split_css_rules("  /* nothing */  ").is_empty());
}

/// Inline CSS is a declaration grammar, not a semicolon-separated map. Values
/// routinely carry the same punctuation as the surrounding grammar, and an
/// unrelated CSSOM mutation must not corrupt them.
#[test]
fn inline_style_mutations_preserve_css_tokens_and_cascade_semantics() {
    let mut dom = fixture_document(
        r#"<div id='styled' style='
          background-image: url("data:image/svg+xml;charset=utf-8,%3Csvg%3E%3C/svg%3E");
          --quoted: "left;right:tail";
          --escaped: semi\;colon\:tail;
          --commented: left/* ; : */right;
          color: rgb(1, 2, 3) !important;
          color: blue;
          broken declaration;
          height: 11px
        '></div>"#,
        None,
    );
    let styled = dom.get_element_by_id("styled").unwrap().unwrap();
    let properties = [
        "background-image",
        "--quoted",
        "--escaped",
        "--commented",
        "color",
        "height",
    ];
    let before = properties.map(|property| dom.inline_style(styled, property).unwrap());

    assert!(before.iter().all(Option::is_some));
    assert!(
        before[0]
            .as_deref()
            .unwrap()
            .contains("data:image/svg+xml;charset=utf-8")
    );
    assert_eq!(before[1].as_deref(), Some(r#""left;right:tail""#));
    assert!(
        before[2]
            .as_deref()
            .unwrap()
            .contains("semi\\;colon\\:tail")
    );
    assert!(before[3].as_deref().unwrap().contains("left"));
    assert_eq!(before[4].as_deref(), Some("rgb(1, 2, 3)"));
    assert_eq!(
        dom.inline_style(styled, "broken declaration").unwrap(),
        None
    );

    assert!(dom.set_inline_style(styled, "width", "17px").unwrap());
    assert_eq!(
        properties.map(|property| dom.inline_style(styled, property).unwrap()),
        before
    );
    let css = dom.inline_style_text(styled).unwrap();
    assert!(css.contains("color: rgb(1, 2, 3) !important"));
    let positions = properties.map(|property| css.find(property).unwrap());
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));

    assert_eq!(
        dom.remove_inline_style(styled, "width").unwrap().as_deref(),
        Some("17px")
    );
    assert_eq!(
        properties.map(|property| dom.inline_style(styled, property).unwrap()),
        before
    );

    // Replacing an important property is replacement, not an appended normal
    // declaration that loses to the old value.
    assert!(dom.set_inline_style(styled, "color", "green").unwrap());
    assert_eq!(
        dom.inline_style(styled, "color").unwrap().as_deref(),
        Some("green")
    );
    assert!(
        !dom.inline_style_text(styled)
            .unwrap()
            .contains("green !important")
    );

    // An invalid replacement leaves every parsed declaration exactly as it was.
    let original = dom.inline_style_text(styled).unwrap();
    assert!(
        !dom.set_inline_style(styled, "height", "definitely-invalid")
            .unwrap()
    );
    assert_eq!(dom.inline_style_text(styled).unwrap(), original);
}

/// The whole point of the CSSOM sheet surface: a rule inserted from script
/// has to reach the stylesheet set Stylo cascades from, which is only
/// answerable in painted pixels.
#[test]
fn an_inserted_rule_changes_what_is_painted() {
    let mut dom = viewport_document(
        r#"<div id="box" style="width: 40px; height: 40px"></div>"#,
        1.0,
    );
    dom.flush_layout().unwrap();
    let colour = |dom: &mut BlitzDom| {
        dom.flush_layout().unwrap();
        pixel(&render(dom, 400, 300), 400, 20, 20)
    };
    assert_eq!(colour(&mut dom), [0, 0, 0, 0], "nothing paints the box yet");

    let sheet = dom.create_element(&DomName::html("style")).unwrap();
    let head = dom.query_selector(dom.document(), "head").unwrap().unwrap();
    dom.append_child(head, sheet).unwrap();
    assert!(dom.style_sheets().unwrap().contains(&sheet));
    assert!(dom.sheet_rules(sheet).unwrap().is_empty());

    dom.insert_sheet_rule(sheet, "#box { background: rgb(10, 20, 200) }", 0)
        .unwrap();
    assert_eq!(colour(&mut dom), [10, 20, 200, 255]);

    // Later rule, equal specificity: the cascade order the sheet's own text
    // gives it is the order the rules were inserted in.
    dom.insert_sheet_rule(sheet, "#box { background: rgb(200, 20, 10) }", 1)
        .unwrap();
    assert_eq!(dom.sheet_rules(sheet).unwrap().len(), 2);
    assert_eq!(colour(&mut dom), [200, 20, 10, 255]);

    dom.delete_sheet_rule(sheet, 1).unwrap();
    assert_eq!(colour(&mut dom), [10, 20, 200, 255], "the rule is gone");

    // Refused rather than written and silently ignored, and refusing must
    // not disturb the sheet.
    for refused in ["not a rule", "a { color: red } b { color: red }", ""] {
        assert!(matches!(
            dom.insert_sheet_rule(sheet, refused, 0),
            Err(DomError::Syntax(_))
        ));
    }
    assert!(matches!(
        dom.insert_sheet_rule(sheet, "a { color: red }", 2),
        Err(DomError::NotFound)
    ));
    assert!(matches!(
        dom.delete_sheet_rule(sheet, 1),
        Err(DomError::NotFound)
    ));
    assert_eq!(dom.sheet_rules(sheet).unwrap().len(), 1);
    assert_eq!(colour(&mut dom), [10, 20, 200, 255]);
}

/// A sheet whose source is a file rather than text in the tree says so.
///
/// Reporting no rules would be the silent answer; there is no rule list here
/// to report, so the read fails.
#[test]
fn a_stylesheet_loaded_from_a_url_refuses_its_rules() {
    let dom = fixture_document(
        r#"<link id="external" rel="stylesheet" href="missing.css">"#,
        None,
    );
    let link = dom.get_element_by_id("external").unwrap().unwrap();
    assert!(dom.style_sheets().unwrap().contains(&link));
    assert!(matches!(dom.sheet_rules(link), Err(DomError::Backend(_))));
    assert!(matches!(
        dom.sheet_rules(dom.body().unwrap()),
        Err(DomError::InvalidNodeType)
    ));
}

/// The case the CSSOM surface exists for: Svelte writes a `@keyframes` block
/// into a sheet it owns and puts `animation` on the element. Blitz animates
/// keyframes, but only against the clock it is resolved with, so the two
/// halves are only worth anything together.
#[test]
fn an_inserted_keyframes_rule_animates_with_the_frame_clock() {
    let mut dom = viewport_document(
        r#"<div id="box" style="width: 40px; height: 40px; background: #000"></div>"#,
        1.0,
    );
    let box_node = dom.get_element_by_id("box").unwrap().unwrap();
    let sheet = dom.create_element(&DomName::html("style")).unwrap();
    let head = dom.query_selector(dom.document(), "head").unwrap().unwrap();
    dom.append_child(head, sheet).unwrap();
    dom.insert_sheet_rule(
        sheet,
        "@keyframes __blitsen_slide { from { margin-left: 0px } to { margin-left: 200px } }",
        0,
    )
    .unwrap();
    dom.set_inline_style(box_node, "animation", "__blitsen_slide 2s linear both")
        .unwrap();

    let frame = |dom: &mut BlitzDom, seconds: f64| {
        dom.set_animation_time(seconds);
        dom.flush_layout().unwrap();
        inked_bounds(&render(dom, 400, 300), 400).expect("the box painted nothing")
    };
    assert_eq!(frame(&mut dom, 0.0), (0, 0, 40, 40));
    assert!(
        dom.is_animating(),
        "a running animation is what keeps a frame loop turning"
    );
    assert_eq!(frame(&mut dom, 1.0), (100, 0, 40, 40), "half way across");
    assert_eq!(frame(&mut dom, 2.0), (200, 0, 40, 40), "at the last frame");

    // And the rule is what is animating it: deleting it stops the animation
    // rather than leaving the box where the last frame left it.
    dom.delete_sheet_rule(sheet, 0).unwrap();
    assert_eq!(frame(&mut dom, 2.5), (0, 0, 40, 40));
}

/// Without a clock from the host every animation in the document is pinned
/// to its first frame, which is what made this API worth implementing at all.
#[test]
fn animations_stand_still_until_the_host_supplies_a_clock() {
    let mut dom = viewport_document(
        r#"<style>
                 @keyframes slide { from { margin-left: 0px } to { margin-left: 200px } }
                 #box { width: 40px; height: 40px; background: #000;
                        animation: slide 2s linear both }
               </style>
               <div id="box"></div>"#,
        1.0,
    );
    for _ in 0..4 {
        dom.flush_layout().unwrap();
        assert_eq!(
            inked_bounds(&render(&mut dom, 400, 300), 400),
            Some((0, 0, 40, 40)),
            "flushing layout must not advance the animation clock by itself"
        );
    }
    dom.set_animation_time(1.0);
    dom.flush_layout().unwrap();
    assert_eq!(
        inked_bounds(&render(&mut dom, 400, 300), 400),
        Some((100, 0, 40, 40))
    );
    // The clock only ever moves forward: a frame delivered out of order
    // would otherwise restart every animation that had already begun.
    dom.set_animation_time(0.5);
    dom.flush_layout().unwrap();
    assert_eq!(
        inked_bounds(&render(&mut dom, 400, 300), 400),
        Some((100, 0, 40, 40))
    );
}

/// The `<link>` half of the subresource state an application waits on, over
/// the four answers a `rel`/`href` pair can produce. A rel this renderer
/// fetches nothing for is idle rather than loaded: reporting an outcome for a
/// request nobody made is how a caller ends up waiting forever.
#[test]
fn a_linked_stylesheet_reports_whether_its_sheet_arrived() {
    let mut dom = fixture_document(
        r#"<link id="arrived" rel="stylesheet" href="linked.css">
               <link id="missing" rel="stylesheet" href="does-not-exist.css">
               <link id="remote" rel="stylesheet" href="https://example.com/theme.css">
               <link id="preload" rel="preload" href="linked.css">
               <link id="hrefless" rel="stylesheet">
               <p id="paragraph">not a link</p>"#,
        None,
    );
    let snapshot = dom.flush_layout().unwrap();
    let state = |id: &str| dom.link_state(dom.get_element_by_id(id).unwrap().unwrap(), snapshot);
    assert_eq!(state("arrived"), Ok(LinkState::LOADED));
    assert_eq!(state("missing"), Ok(LinkState::FAILED));
    assert_eq!(
        state("remote"),
        Ok(LinkState::FAILED),
        "a refused remote fetch is an error, not an unfinished one"
    );
    assert_eq!(
        state("preload"),
        Ok(LinkState::IDLE),
        "a rel this renderer requests nothing for owes no outcome"
    );
    assert_eq!(
        state("hrefless"),
        Ok(LinkState::IDLE),
        "a link with no address is not waiting on anything"
    );
    assert_eq!(state("paragraph"), Err(DomError::InvalidNodeType));
}

/// The state a script actually observes: in flight first, and complete only
/// once the sheet is in the cascade. The resolved width is the whole point of
/// the snapshot gate — a `load` handler that ran a flush too early would read
/// the style the sheet was about to replace.
#[test]
fn a_linked_stylesheet_is_in_the_cascade_by_the_time_it_says_it_loaded() {
    let network = DeferredResources::default();
    let mut dom = fixture_document(
        r#"<link id="sheet" rel="stylesheet" href="linked.css"><div id="box"></div>"#,
        Some(Arc::new(network.clone())),
    );
    let snapshot = dom.flush_layout().unwrap();
    let sheet = dom.get_element_by_id("sheet").unwrap().unwrap();
    let box_id = dom.get_element_by_id("box").unwrap().unwrap();
    assert_eq!(dom.link_state(sheet, snapshot), Ok(LinkState::LOADING));
    assert_eq!(
        dom.resolved_style(box_id, "width", snapshot),
        Ok(Some("400px".to_owned())),
        "the sheet has not applied yet, so the block is still viewport wide"
    );

    network.deliver();
    let snapshot = dom.flush_layout().unwrap();
    assert_eq!(dom.link_state(sheet, snapshot), Ok(LinkState::LOADED));
    assert_eq!(
        dom.resolved_style(box_id, "width", snapshot),
        Ok(Some("150px".to_owned())),
        "a sheet that reports itself loaded is one the cascade has already read"
    );
}

/// The path a theme switcher takes: an element built by script, given a `rel`
/// and an `href` and then connected, has to load exactly like a parsed one.
#[test]
fn a_scripted_link_loads_when_it_is_connected() {
    let mut dom = fixture_document(r#"<div id="box"></div>"#, None);
    let body = dom.body().unwrap();
    let link = dom.create_element(&DomName::html("link")).unwrap();
    dom.set_attribute(link, &DomName::attribute("rel"), "stylesheet")
        .unwrap();
    let snapshot = dom.flush_layout().unwrap();
    assert_eq!(
        dom.link_state(link, snapshot),
        Ok(LinkState::IDLE),
        "a link with no address has nothing to wait for, attached or not"
    );

    dom.set_attribute(link, &DomName::attribute("href"), "linked.css")
        .unwrap();
    let snapshot = dom.flush_layout().unwrap();
    assert_eq!(
        dom.link_state(link, snapshot),
        Ok(LinkState::LOADING),
        "a detached link is waiting on a request nothing has made yet"
    );

    dom.append_child(body, link).unwrap();
    let snapshot = dom.flush_layout().unwrap();
    assert_eq!(dom.link_state(link, snapshot), Ok(LinkState::LOADED));
    let box_id = dom.get_element_by_id("box").unwrap().unwrap();
    assert_eq!(
        dom.resolved_style(box_id, "width", snapshot),
        Ok(Some("150px".to_owned()))
    );

    dom.set_attribute(link, &DomName::attribute("media"), "all")
        .unwrap();
    assert_eq!(
        dom.link_state(link, snapshot),
        Err(DomError::LayoutNotFlushed),
        "a sheet enters the cascade while layout resolves, so the state is snapshot gated"
    );
}

/// `pointer-events` has nine values this cascade cannot parse and one of them
/// is the one every canvas library uses. Asserted through hit testing rather
/// than through the resolved value, because being reachable by a pointer is
/// what the declaration is for.
///
/// The shape is React Flow's: a container that takes no hits, holding elements
/// that declare that they do. Dropping the inner declaration left them
/// inheriting `none`, and nothing on the canvas could be hit at all.
#[test]
fn an_element_that_declares_it_takes_hits_inside_one_that_does_not_is_hit() {
    let mut dom = viewport_document(
        r#"<style>
             .nodes { position: absolute; inset: 0; pointer-events: none }
             .node { position: absolute; left: 20px; top: 20px; width: 80px; height: 40px;
                     pointer-events: all }
           </style>
           <div class="nodes"><div class="node" id="node"></div></div>"#,
        1.0,
    );
    let snapshot = dom.flush_layout().expect("layout");
    let node = dom
        .get_element_by_id("node")
        .expect("query")
        .expect("element exists");
    assert_eq!(
        resolved(&dom, snapshot, "node", "pointer-events"),
        "auto",
        "the declaration was dropped and the element inherited `none`"
    );
    assert_eq!(
        dom.hit_test(40.0, 40.0, snapshot)
            .expect("hit test")
            .map(|hit| hit.target),
        Some(node),
        "the element declared that it takes hits and took none"
    );
}

/// The same declaration, arriving every other way CSS can arrive: from a
/// script's `<style>`, from an inline style attribute, and from the CSSOM.
#[test]
fn a_declaration_written_after_the_parse_is_normalised_the_same_way() {
    let mut dom = viewport_document(
        r#"<style>.nodes { position: absolute; inset: 0; pointer-events: none }</style>
           <div class="nodes">
             <div class="node" id="scripted"></div>
             <div class="node" id="inline"></div>
           </div>"#,
        1.0,
    );
    let sheet = dom.create_element(&DomName::html("style")).expect("style");
    let head = dom
        .query_selector(dom.document(), "head")
        .expect("query")
        .expect("head exists");
    dom.append_child(head, sheet).expect("append");
    dom.set_text_content(
        sheet,
        "#scripted { position: absolute; left: 0; top: 0; width: 40px; height: 40px;
                     pointer-events: all }",
    )
    .expect("sheet text");
    let inline = dom
        .get_element_by_id("inline")
        .expect("query")
        .expect("element exists");
    dom.set_inline_style_text(
        inline,
        "position: absolute; left: 50px; top: 0; width: 40px; height: 40px; pointer-events: all",
    )
    .expect("inline style");
    let snapshot = dom.flush_layout().expect("layout");
    assert_eq!(
        resolved(&dom, snapshot, "scripted", "pointer-events"),
        "auto"
    );
    assert_eq!(resolved(&dom, snapshot, "inline", "pointer-events"), "auto");
    // And one property at a time, which never reaches the cascade as text.
    dom.set_inline_style(inline, "pointer-events", "visiblePainted")
        .expect("property")
        .then_some(())
        .expect("the property was refused rather than normalised");
    let snapshot = dom.flush_layout().expect("layout");
    assert_eq!(resolved(&dom, snapshot, "inline", "pointer-events"), "auto");
}

/// A linked sheet's text is bytes off a transfer rather than text in the tree,
/// so it is the one stylesheet that has to be normalised as it arrives.
#[test]
fn a_linked_stylesheet_is_normalised_on_its_way_into_the_cascade() {
    let mut dom = fixture_document(
        r#"<link rel="stylesheet" href="linked.css">
               <div style="pointer-events: none"><div id="hits"></div></div>"#,
        None,
    );
    // Inside a container that takes no hits, so a dropped declaration is
    // observable: the element would inherit `none` rather than resolve `auto`.
    let snapshot = dom.flush_layout().unwrap();
    assert_eq!(resolved(&dom, snapshot, "hits", "pointer-events"), "auto");
}
