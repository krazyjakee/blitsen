//! The cursor a point in the viewport resolves to.
//!
//! Asserted from the point rather than from an element, because that is what
//! the window has: the pointer is somewhere, and the shell has to be told what
//! to show there. An element-shaped assertion would pass while the window kept
//! the wrong cursor, which is exactly the state issue #128 reported.

use super::*;
use cursor_icon::CursorIcon;

fn cursor_at(dom: &mut BlitzDom, x: f32, y: f32) -> Option<CursorIcon> {
    dom.flush_layout().expect("layout");
    dom.cursor_at(x, y).expect("cursor")
}

/// React Flow's connection handle, reduced to the shape that broke it.
///
/// Every part matters. The nodes sit inside a `pointer-events: none` container
/// that is transformed as a whole; each handle is absolutely positioned and
/// translated half outside the node it hangs off; and the node itself declares
/// `cursor: default`, so a hit that lands on the node instead of the handle
/// looks exactly like a pointer that never moved.
#[test]
fn an_author_cursor_on_a_handle_hung_outside_its_node_reaches_the_shell() {
    let mut dom = viewport_document(
        r#"<style>
             .pane { position: absolute; inset: 0; z-index: 1 }
             .viewport { position: absolute; inset: 0; z-index: 2; pointer-events: none;
                         transform-origin: 0 0; transform: translate(20px, 20px) }
             .nodes { pointer-events: none; transform-origin: 0 0 }
             .node { position: absolute; left: 40px; top: 40px; width: 120px; height: 60px;
                     pointer-events: all; cursor: default; background: #333 }
             .handle { position: absolute; top: 50%; left: 0; width: 10px; height: 10px;
                       transform: translate(-50%, -50%); pointer-events: all;
                       cursor: crosshair; background: red }
           </style>
           <div class="pane"></div>
           <div class="viewport"><div class="nodes">
             <div class="node" id="node"><div class="handle" id="handle"></div></div>
           </div></div>"#,
        1.0,
    );
    // The node occupies (60, 60)-(180, 120); the handle is centred on its left
    // edge, so it covers (55, 85)-(65, 95) and hangs half outside the node.
    assert_eq!(
        cursor_at(&mut dom, 120.0, 90.0),
        Some(CursorIcon::Default),
        "the node's own cursor"
    );
    assert_eq!(
        cursor_at(&mut dom, 60.0, 90.0),
        Some(CursorIcon::Crosshair),
        "a handle inside the node's box did not reach the shell"
    );
    assert_eq!(
        cursor_at(&mut dom, 57.0, 90.0),
        Some(CursorIcon::Crosshair),
        "the half of the handle hung outside its node did not reach the shell"
    );
    assert_eq!(
        cursor_at(&mut dom, 120.0, 90.0),
        Some(CursorIcon::Default),
        "leaving the handle did not restore the cursor under it"
    );
}

/// The class that makes a handle connectable is added while the pointer is
/// already sitting on it, and the cursor has to follow without a second move.
#[test]
fn a_class_added_under_a_still_pointer_changes_the_cursor() {
    let mut dom = viewport_document(
        r#"<style>
             .handle { position: absolute; left: 50px; top: 50px; width: 20px; height: 20px }
             .handle.connectable { cursor: crosshair }
           </style>
           <div class="handle" id="handle"></div>"#,
        1.0,
    );
    assert_eq!(cursor_at(&mut dom, 60.0, 60.0), Some(CursorIcon::Default));
    let handle = dom
        .get_element_by_id("handle")
        .expect("query")
        .expect("element exists");
    dom.set_attribute(handle, &DomName::attribute("class"), "handle connectable")
        .expect("class");
    assert_eq!(
        cursor_at(&mut dom, 60.0, 60.0),
        Some(CursorIcon::Crosshair),
        "the cursor was still the one resolved before the class was added"
    );
}

/// `cursor: none` is the one value that is not a cursor: the shell hides the
/// pointer rather than showing an arrow, so it cannot collapse into the default.
#[test]
fn cursor_none_asks_for_no_pointer_rather_than_the_arrow() {
    let mut dom = viewport_document(
        r#"<div id="hidden" style="position: absolute; left: 0; top: 0; width: 100px;
             height: 100px; cursor: none"></div>"#,
        1.0,
    );
    assert_eq!(cursor_at(&mut dom, 50.0, 50.0), None);
    assert_eq!(
        cursor_at(&mut dom, 150.0, 150.0),
        Some(CursorIcon::Default),
        "beside it the arrow is back"
    );
}

/// Paint order decides the cursor for the same reason it decides the click: the
/// element in front is the one the pointer is on.
#[test]
fn the_element_in_front_decides_the_cursor() {
    let mut dom = viewport_document(
        r#"<style>
             div { position: absolute; left: 0; top: 0; width: 100px; height: 100px }
             #under { cursor: wait; z-index: 2 }
             #over { cursor: grab; z-index: 1 }
           </style>
           <div id="under"></div><div id="over"></div>"#,
        1.0,
    );
    assert_eq!(
        cursor_at(&mut dom, 50.0, 50.0),
        Some(CursorIcon::Wait),
        "the higher z-index is in front however the source is ordered"
    );
}

/// An element that is transparent to hits is transparent to the cursor too.
#[test]
fn a_pointer_events_none_overlay_shows_the_cursor_underneath_it() {
    let mut dom = viewport_document(
        r#"<style>
             div { position: absolute; left: 0; top: 0; width: 100px; height: 100px }
             #target { cursor: crosshair }
             #overlay { cursor: wait; pointer-events: none; z-index: 5 }
           </style>
           <div id="target"></div><div id="overlay"></div>"#,
        1.0,
    );
    assert_eq!(cursor_at(&mut dom, 50.0, 50.0), Some(CursorIcon::Crosshair));
}

/// `auto` is not one answer: it is the caret over text, the hand over a link,
/// the caret inside a control, and the arrow over everything else.
#[test]
fn auto_resolves_from_what_the_pointer_is_over() {
    let mut dom = viewport_document(
        r#"<p id="prose" style="width: 300px; margin: 0">ordinary prose</p>
           <p style="margin: 0"><a id="link" href="/elsewhere">a link</a></p>
           <input id="field" value="typed">
           <div id="empty" style="width: 300px; height: 40px"></div>"#,
        1.0,
    );
    let snapshot = dom.flush_layout().expect("layout");
    for (id, expected, message) in [
        (
            "prose",
            Some(CursorIcon::Text),
            "text under the pointer is the caret",
        ),
        ("link", Some(CursorIcon::Pointer), "a link is the hand"),
        (
            "field",
            Some(CursorIcon::Text),
            "a control that holds text is the caret throughout",
        ),
        (
            "empty",
            Some(CursorIcon::Default),
            "a box with no text in it is the arrow",
        ),
    ] {
        let node = dom
            .get_element_by_id(id)
            .expect("query")
            .expect("element exists");
        let rect = dom.layout_metrics(node, snapshot).expect("metrics").rect;
        assert_eq!(
            dom.cursor_at(rect.x + 2.0, rect.y + rect.height / 2.0)
                .expect("cursor"),
            expected,
            "{message}"
        );
    }
}

/// The empty space to the right of a short line is inside the paragraph's box
/// and is not text, so the caret must not be shown across it.
#[test]
fn the_space_past_the_end_of_a_line_is_not_text() {
    let mut dom = viewport_document(
        r#"<p id="prose" style="width: 300px; margin: 0">short</p>"#,
        1.0,
    );
    assert_eq!(cursor_at(&mut dom, 2.0, 8.0), Some(CursorIcon::Text));
    assert_eq!(
        cursor_at(&mut dom, 290.0, 8.0),
        Some(CursorIcon::Default),
        "the caret was shown over empty line-box space"
    );
}
