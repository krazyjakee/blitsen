//! Range geometry: the rectangles a run of characters occupies, and the
//! character a point lands on.
//!
//! Measured against the block fixture font throughout. Every one of its glyphs
//! is a solid em box, so at `font: 50px` a character is exactly 50 wide and a
//! run's arithmetic is exact: an assertion here says the run was measured
//! rather than that it was plausible. The same font is what lets the caret
//! read be checked by clicking at a coordinate the rectangles named.

use super::*;

/// A document laid out in the fixture font, at 50px with no margins anywhere.
fn text_document(body: &str) -> BlitzDom {
    scaled_text_document(body, 1.0)
}

fn scaled_text_document(body: &str, scale: f32) -> BlitzDom {
    let html = format!(
        r#"<style>
             html, body {{ margin: 0 }}
             @font-face {{ font-family: "Block"; src: url("block-ascii.ttf") format("truetype") }}
             * {{ margin: 0; padding: 0; font: 50px "Block"; }}
           </style>
           {body}"#,
    );
    BlitzDom::from_html(
        &html,
        DocumentConfig {
            base_url: Some(fixtures_url()),
            viewport: Some(Viewport::new(400, 200, scale, ColorScheme::Light)),
            ..Default::default()
        },
    )
}

/// The `index`th text node inside the element a selector names.
fn text_node(dom: &BlitzDom, selector: &str, index: usize) -> blitz::dom::NodeId {
    let element = dom
        .query_selector(dom.document(), selector)
        .unwrap()
        .expect("the selector matched no element");
    dom.children(element)
        .unwrap()
        .into_iter()
        .filter(|node| dom.node_kind(*node) == Ok(NodeKind::Text))
        .nth(index)
        .expect("the element has no text node there")
}

/// `(x, width)` of each rectangle, which is what the fixture font pins exactly.
fn spans(rects: &[blitsen_dom::Rect]) -> Vec<(f32, f32)> {
    rects.iter().map(|rect| (rect.x, rect.width)).collect()
}

#[test]
fn a_run_of_characters_measures_the_glyphs_it_covers() {
    let mut dom = text_document(r#"<p id="line">ABCDEF</p>"#);
    let text = text_node(&dom, "#line", 0);
    let snapshot = dom.flush_layout().unwrap();

    let rects = dom.text_rects(text, 1, 3, snapshot).unwrap();
    assert_eq!(
        spans(&rects),
        vec![(50.0, 100.0)],
        "two 50px em blocks, starting one block in"
    );
    assert!(
        rects[0].height > 0.0 && rects[0].y >= 0.0,
        "the rectangle covers the line box: {:?}",
        rects[0]
    );
    assert_eq!(
        spans(&dom.text_rects(text, 0, 6, snapshot).unwrap()),
        vec![(0.0, 300.0)],
        "the whole node is the whole run"
    );
    assert!(
        dom.text_rects(text, 2, 2, snapshot).unwrap().is_empty(),
        "a collapsed range covers nothing"
    );
    assert_eq!(
        spans(&dom.text_rects(text, 4, 99, snapshot).unwrap()),
        vec![(200.0, 100.0)],
        "an offset past the end is the end"
    );
}

/// A range is measured per line box, because that is what a browser returns and
/// what any caller measuring wrapped text is asking for.
#[test]
fn a_wrapped_run_measures_one_rectangle_per_line() {
    let mut dom = text_document(r#"<p id="line" style="width: 200px">AAAA BBBB</p>"#);
    let text = text_node(&dom, "#line", 0);
    let snapshot = dom.flush_layout().unwrap();

    let rects = dom.text_rects(text, 0, 9, snapshot).unwrap();
    assert_eq!(
        spans(&rects),
        vec![(0.0, 250.0), (0.0, 200.0)],
        "four blocks a line, and the space the break hangs is inside the run"
    );
    assert!(
        rects[1].y >= rects[0].y + rects[0].height,
        "the second line is below the first: {rects:?}"
    );
    assert_eq!(
        spans(&dom.text_rects(text, 6, 8, snapshot).unwrap()),
        vec![(50.0, 100.0)],
        "a run inside the second line measures on that line alone"
    );
}

/// The laid-out text is not the DOM's text, and the offsets a caller counts are
/// the DOM's. A collapsed run of whitespace is one space in the layout and four
/// characters in the node, so measuring what follows it is the whole test.
#[test]
fn collapsed_whitespace_does_not_shift_the_offsets() {
    let mut dom = text_document(
        r#"<p id="line">  AB
             CD  </p>"#,
    );
    let text = text_node(&dom, "#line", 0);
    let snapshot = dom.flush_layout().unwrap();
    let content = dom.text_content(text).unwrap();
    let position = content.find("CD").expect("the fixture contains it") as u32;

    assert_eq!(
        spans(
            &dom.text_rects(text, position, position + 2, snapshot)
                .unwrap()
        ),
        vec![(150.0, 100.0)],
        "two leading spaces dropped, the newline and its indentation one space: `AB CD`"
    );
    assert_eq!(
        spans(&dom.text_rects(text, 0, 2, snapshot).unwrap()),
        vec![],
        "the leading whitespace laid nothing out, so it covers nothing"
    );
}

/// Every text node in a block shares one layout, so a node's own offsets have
/// to be found within it rather than assumed to start at zero.
#[test]
fn a_node_is_measured_within_the_layout_it_shares() {
    let mut dom = text_document(r#"<p id="line">AB<span id="mid">CD</span>EF</p>"#);
    let snapshot = dom.flush_layout().unwrap();

    assert_eq!(
        spans(
            &dom.text_rects(text_node(&dom, "#line", 0), 0, 2, snapshot)
                .unwrap()
        ),
        vec![(0.0, 100.0)]
    );
    assert_eq!(
        spans(
            &dom.text_rects(text_node(&dom, "#mid", 0), 0, 2, snapshot)
                .unwrap()
        ),
        vec![(100.0, 100.0)],
        "the nested node's own offsets, measured where the span sits"
    );
    assert_eq!(
        spans(
            &dom.text_rects(text_node(&dom, "#line", 1), 1, 2, snapshot)
                .unwrap()
        ),
        vec![(250.0, 50.0)],
        "the node after the span, measured from its own second character"
    );
}

/// A case transform rewrites the laid-out text without changing the node's
/// data, so the offsets a caller counts are still the data's.
#[test]
fn a_case_transform_does_not_shift_the_offsets() {
    let mut dom = text_document(r#"<p id="line" style="text-transform: uppercase">ab cd</p>"#);
    let text = text_node(&dom, "#line", 0);
    let snapshot = dom.flush_layout().unwrap();

    assert_eq!(
        spans(&dom.text_rects(text, 3, 5, snapshot).unwrap()),
        vec![(150.0, 100.0)],
        "`CD` is where `cd` was written"
    );
}

/// Text that no node owns — a `<br>`'s newline, a list marker — is in the same
/// layout as the text that follows it, and would shift it if it were ignored.
#[test]
fn text_no_node_owns_is_accounted_for() {
    let mut dom = text_document(r#"<p id="line">AB<br>CD</p>"#);
    let snapshot = dom.flush_layout().unwrap();
    let rects = dom
        .text_rects(text_node(&dom, "#line", 1), 0, 2, snapshot)
        .unwrap();
    assert_eq!(
        spans(&rects),
        vec![(0.0, 100.0)],
        "the text after the break starts the second line"
    );

    let mut dom =
        text_document(r#"<ul style="list-style-position: inside"><li id="item">AB</li></ul>"#);
    let snapshot = dom.flush_layout().unwrap();
    let rects = dom
        .text_rects(text_node(&dom, "#item", 0), 0, 2, snapshot)
        .unwrap();
    assert!(
        rects[0].x > 0.0,
        "the marker is laid out in front of the text: {rects:?}"
    );
    assert_eq!(rects[0].width, 100.0, "and the text is still two blocks");
}

/// Geometry is viewport-relative, which is what a client rectangle means: a
/// scrolled document reports where the run is now, not where it was written.
#[test]
fn rectangles_follow_the_scroll_and_the_box_they_sit_in() {
    let mut dom = text_document(
        r#"<div id="pad" style="padding: 30px 20px"><p id="line">AB</p></div>
           <div style="height: 900px"></div>"#,
    );
    let text = text_node(&dom, "#line", 0);
    let snapshot = dom.flush_layout().unwrap();
    assert_eq!(
        spans(&dom.text_rects(text, 0, 2, snapshot).unwrap()),
        vec![(20.0, 100.0)],
        "the padding of the box the line sits in"
    );
    let before = dom.text_rects(text, 0, 2, snapshot).unwrap()[0].y;

    let root = dom.document_element().unwrap();
    dom.set_scroll_offset(root, None, Some(40.0), snapshot)
        .unwrap();
    let snapshot = dom.flush_layout().unwrap();
    assert_eq!(
        dom.text_rects(text, 0, 2, snapshot).unwrap()[0].y,
        before - 40.0,
        "scrolled up by the scroll"
    );
}

/// Parley measures in device pixels and the DOM answers in CSS ones, so a
/// document on a 2x display is the case that says whether the scale was
/// divided back out — and it reads identically to the same document at 1x.
#[test]
fn geometry_is_reported_in_css_pixels_whatever_the_display_scale_is() {
    let mut dom = scaled_text_document(r#"<p id="line">ABCDEF</p>"#, 2.0);
    let text = text_node(&dom, "#line", 0);
    let snapshot = dom.flush_layout().unwrap();

    assert_eq!(
        spans(&dom.text_rects(text, 1, 3, snapshot).unwrap()),
        vec![(50.0, 100.0)],
        "the same two 50px blocks, one block in"
    );
    let rect = dom.text_rects(text, 1, 3, snapshot).unwrap()[0];
    assert_eq!(
        dom.caret_position(rect.x + 25.0, rect.y + rect.height / 2.0, snapshot)
            .unwrap()
            .map(|caret| (caret.node, caret.offset)),
        Some((text, 1)),
        "and the caret read takes CSS pixels too"
    );
}

/// A node nothing laid out has no geometry rather than an empty box at the
/// origin, which is the same answer the box reads give.
#[test]
fn text_that_was_never_laid_out_has_no_geometry() {
    let mut dom = text_document(r#"<p id="line" style="display: none">AB</p><p id="shown">CD</p>"#);
    let hidden = text_node(&dom, "#line", 0);
    let detached = dom.create_text("EF").unwrap();
    let snapshot = dom.flush_layout().unwrap();

    assert!(dom.text_rects(hidden, 0, 2, snapshot).unwrap().is_empty());
    assert!(dom.text_rects(detached, 0, 2, snapshot).unwrap().is_empty());
    assert_eq!(
        dom.text_rects(dom.document_element().unwrap(), 0, 1, snapshot),
        Err(DomError::InvalidNodeType),
        "an element is not a run of characters"
    );
}

/// The caret read is the rectangles asked the other way round, so it is checked
/// against them: a point inside the block a character painted is that character.
#[test]
fn a_point_resolves_to_the_character_under_it() {
    let mut dom = text_document(r#"<p id="line">AB<span id="mid">CD</span>EF</p>"#);
    let snapshot = dom.flush_layout().unwrap();
    let line = text_node(&dom, "#line", 0);
    let mid = text_node(&dom, "#mid", 0);
    let tail = text_node(&dom, "#line", 1);
    let middle = dom.text_rects(line, 0, 1, snapshot).unwrap()[0];
    let row = middle.y + middle.height / 2.0;

    let at = |dom: &BlitzDom, x: f32| {
        dom.caret_position(x, row, snapshot)
            .unwrap()
            .map(|caret| (caret.node, caret.offset))
    };
    assert_eq!(
        at(&dom, 10.0),
        Some((line, 0)),
        "the left of the first block"
    );
    assert_eq!(at(&dom, 90.0), Some((line, 2)), "the right of the second");
    assert_eq!(at(&dom, 110.0), Some((mid, 0)), "inside the span");
    assert_eq!(at(&dom, 260.0), Some((tail, 1)), "and after it");
    assert_eq!(
        at(&dom, 380.0),
        Some((tail, 2)),
        "past the end of the line is the end of its last node"
    );
    assert_eq!(
        dom.caret_position(10.0, 250.0, snapshot).unwrap(),
        None,
        "a point over no text at all has no answer"
    );
}

/// An inline element wrapped across lines occupies a rectangle per line, which
/// is the one thing a single border box cannot report.
#[test]
fn an_inline_element_reports_a_rectangle_for_each_line_it_wraps_onto() {
    let mut dom = text_document(r#"<p style="width: 200px">A <span id="run">BBB CCC</span> D</p>"#);
    let snapshot = dom.flush_layout().unwrap();
    let span = dom.query_selector(dom.document(), "#run").unwrap().unwrap();

    let rects = dom.client_rects(span, snapshot).unwrap();
    assert_eq!(rects.len(), 2, "one per line box: {rects:?}");
    assert_eq!(
        dom.bounding_rect(span, snapshot).unwrap().width,
        rects
            .iter()
            .map(|rect| rect.x + rect.width)
            .fold(f32::MIN, f32::max)
            - rects.iter().map(|rect| rect.x).fold(f32::MAX, f32::min),
        "the bounding rectangle is their union"
    );

    let block = dom.query_selector(dom.document(), "p").unwrap().unwrap();
    assert_eq!(
        dom.client_rects(block, snapshot).unwrap(),
        vec![dom.bounding_rect(block, snapshot).unwrap()],
        "a node with a box of its own has exactly one, and it is that box"
    );
}
