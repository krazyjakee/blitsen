//! The user-agent baseline Blitz's default sheet leaves out.
//!
//! These assert the observable behaviour rather than the presence of a rule:
//! what the window would show as a cursor, what the cascade resolves, and where
//! layout puts the boxes.

use super::*;
use blitsen_dom::LayoutSnapshot;
use cursor_icon::CursorIcon;

/// Hovers an element and reports the cursor the shell would set.
///
/// The point is taken just inside the leading edge rather than at the centre:
/// the centre of a wide block is past the end of its text, and a hit on empty
/// line-box space is not a text hit.
fn cursor_over(dom: &mut BlitzDom, id: &str) -> Option<CursorIcon> {
    let snapshot = dom.flush_layout().expect("layout");
    let node = dom
        .get_element_by_id(id)
        .expect("query")
        .expect("element exists");
    let rect = dom.layout_metrics(node, snapshot).expect("metrics").rect;
    let document = dom.document_mut().as_mut();
    document.set_hover_to(rect.x + 2.0, rect.y + rect.height / 2.0);
    document.get_cursor()
}

fn resolved(dom: &BlitzDom, snapshot: LayoutSnapshot, id: &str, property: &str) -> String {
    let node = dom
        .get_element_by_id(id)
        .expect("query")
        .expect("element exists");
    dom.resolved_style(node, property, snapshot)
        .expect("resolved style")
        .expect("property is resolvable")
}

/// A button is a control, not a paragraph: the hit on its own label must not
/// fall through to the text caret the way an unstyled `cursor: auto` does.
#[test]
fn a_button_hovers_as_a_control_and_prose_still_hovers_as_text() {
    let mut dom = viewport_document(
        r#"<button id="control" style="padding: 20px">Increment</button>
           <p id="prose" style="width: 300px">ordinary prose</p>"#,
        1.0,
    );
    assert_eq!(
        cursor_over(&mut dom, "control"),
        Some(CursorIcon::Default),
        "a button's label must not report the text caret"
    );
    assert_eq!(
        cursor_over(&mut dom, "prose"),
        Some(CursorIcon::Text),
        "prose is still selectable text"
    );
}

/// The label of a control is not selectable prose either, and an author who
/// wants the hand affordance stays in charge of it.
#[test]
fn control_labels_are_unselectable_and_the_author_still_decides_the_cursor() {
    let mut dom = viewport_document(
        r#"<style>#hand { cursor: pointer }</style>
           <button id="hand">Increment</button><label id="named">Name</label>"#,
        1.0,
    );
    let snapshot = dom.flush_layout().unwrap();
    assert_eq!(resolved(&dom, snapshot, "hand", "user-select"), "none");
    assert_eq!(resolved(&dom, snapshot, "named", "cursor"), "default");
    assert_eq!(
        cursor_over(&mut dom, "hand"),
        Some(CursorIcon::Pointer),
        "an author's cursor must still win over the baseline"
    );
}

/// A control that cannot be used has to look unusable.
#[test]
fn a_disabled_control_does_not_resolve_to_the_same_style_as_a_live_one() {
    let dom = viewport_document(
        r#"<input id="live" value="live"><input id="dead" disabled value="dead">"#,
        1.0,
    );
    let mut dom = dom;
    let snapshot = dom.flush_layout().unwrap();
    let live = resolved(&dom, snapshot, "live", "color");
    let dead = resolved(&dom, snapshot, "dead", "color");
    assert_ne!(
        live, dead,
        "a disabled control resolved the same colour as a live one: {live}"
    );
    assert_eq!(resolved(&dom, snapshot, "dead", "cursor"), "default");
}

/// Without `display: block` the legend and the contents share a line, which is
/// what the missing `forms.css` produced.
#[test]
fn a_fieldset_is_a_bordered_block_and_its_legend_takes_its_own_line() {
    let mut dom = viewport_document(
        r#"<fieldset id="set"><legend id="caption">Legend</legend><span id="body">contents</span></fieldset>"#,
        1.0,
    );
    let snapshot = dom.flush_layout().unwrap();
    assert_eq!(resolved(&dom, snapshot, "set", "display"), "block");
    assert_eq!(resolved(&dom, snapshot, "set", "border-top-width"), "2px");
    let set = dom.get_element_by_id("set").unwrap().unwrap();
    let caption = dom.get_element_by_id("caption").unwrap().unwrap();
    let body = dom.get_element_by_id("body").unwrap().unwrap();
    let caption_rect = dom.layout_metrics(caption, snapshot).unwrap().rect;
    let body_rect = dom.layout_metrics(body, snapshot).unwrap().rect;
    assert!(
        body_rect.y > caption_rect.y,
        "the legend and the contents shared a line: {caption_rect:?} {body_rect:?}"
    );
    assert!(
        caption_rect.width > body_rect.width * 2.0,
        "the legend did not take a line of its own: {caption_rect:?} {body_rect:?}"
    );
    let set_rect = dom.layout_metrics(set, snapshot).unwrap().rect;
    assert!(
        caption_rect.x > set_rect.x,
        "the fieldset drew no padding: {set_rect:?} {caption_rect:?}"
    );
}

/// An `<a>` is a link when it has an `href`, and a bare anchor is prose.
///
/// Both orders are checked deliberately. Blitz matches link-ness ad hoc and
/// never records it in `ElementState`, so stylo's style-sharing cache will hand
/// one anchor's style to a sibling anchor of the opposite kind — with a
/// `:not(:any-link)` rule here, whichever anchor came first won.
#[test]
fn only_an_anchor_with_an_href_is_painted_as_a_link() {
    for body in [
        r##"<a id="link" href="#target">link</a><a id="plain">plain</a>"##,
        r##"<a id="plain">plain</a><a id="link" href="#target">link</a>"##,
    ] {
        let mut dom = viewport_document(body, 1.0);
        let snapshot = dom.flush_layout().unwrap();
        assert_eq!(
            resolved(&dom, snapshot, "link", "text-decoration-line"),
            "underline",
            "a real link lost its underline in {body}"
        );
        assert_ne!(
            resolved(&dom, snapshot, "link", "color"),
            resolved(&dom, snapshot, "plain", "color"),
            "an anchor without an href resolved a link's colour in {body}"
        );
        assert_eq!(
            resolved(&dom, snapshot, "plain", "text-decoration-line"),
            "none",
            "an anchor without an href was underlined in {body}"
        );
    }
}

/// A textarea reports what was typed into it, leading whitespace included.
#[test]
fn a_textarea_preserves_the_whitespace_of_its_contents() {
    let mut dom = viewport_document(r#"<textarea id="notes">  indented</textarea>"#, 1.0);
    let snapshot = dom.flush_layout().unwrap();
    assert_eq!(resolved(&dom, snapshot, "notes", "white-space"), "pre-wrap");
}
