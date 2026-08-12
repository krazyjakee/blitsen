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
