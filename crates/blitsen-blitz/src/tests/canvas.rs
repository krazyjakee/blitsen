//! `<canvas>` as a laid-out replaced box carrying a recorded scene.
//!
//! The first question here is Blitz's rather than ours: a `<canvas>` is already
//! a replaced element upstream, sized from its `width`/`height` content
//! attributes, and what had to be established is that it survives being given a
//! custom widget at all — `SpecialElementData` holds one slot, and the widget
//! takes the one the canvas branch of replaced layout matches on. That is
//! [G14](../../../../docs/BLITZ-GAPS.md), patched locally.
//!
//! The rest is ours: the backing store the content attributes own, and the fact
//! that what a canvas records is in its own pixels and is scaled into whatever
//! box CSS gives the element.

use super::*;

use anyrender::PaintScene as _;
use blitz::dom::Widget;
use kurbo::{Affine, Rect};
use peniko::{Color, Fill};

use crate::surface::Surface as _;

/// A widget that draws nothing, so what is under test is the attachment.
struct Probe;

impl Widget for Probe {}

/// The backing store of an attached canvas, by element id.
fn backing_store(dom: &BlitzDom, id: &str) -> (u32, u32) {
    let node = dom.get_element_by_id(id).unwrap().unwrap();
    dom.canvases
        .get(&node)
        .expect("attached canvas")
        .borrow()
        .size()
}

/// The revision of an attached canvas's contents.
fn revision(dom: &BlitzDom, id: &str) -> u64 {
    let node = dom.get_element_by_id(id).unwrap().unwrap();
    dom.canvases
        .get(&node)
        .expect("attached canvas")
        .borrow()
        .revision()
}

#[test]
fn canvas_is_sized_by_its_content_attributes() {
    let mut dom = viewport_document(
        r#"<canvas id="default"></canvas>
           <canvas id="sized" width="200" height="100"></canvas>
           <canvas id="styled" width="200" height="100" style="width: 50px"></canvas>"#,
        1.0,
    );
    let snapshot = dom.flush_layout().unwrap();
    let box_of = |dom: &BlitzDom, id: &str| {
        let node = dom.get_element_by_id(id).unwrap().unwrap();
        let rect = dom.bounding_rect(node, snapshot).unwrap();
        (rect.width, rect.height)
    };

    assert_eq!(
        box_of(&dom, "default"),
        (300.0, 150.0),
        "a canvas with no attributes uses the default object size"
    );
    assert_eq!(box_of(&dom, "sized"), (200.0, 100.0));
    assert_eq!(
        box_of(&dom, "styled"),
        (50.0, 25.0),
        "CSS scales the box, and the intrinsic ratio carries the height"
    );
}

#[test]
fn canvas_survives_carrying_a_custom_widget() {
    let mut dom = viewport_document(
        r#"<canvas id="canvas" width="200" height="100"></canvas>"#,
        1.0,
    );
    let canvas = dom.get_element_by_id("canvas").unwrap().unwrap();
    dom.document_mut()
        .mutate()
        .set_custom_widget(canvas, Box::new(Probe));

    let snapshot = dom.flush_layout().unwrap();
    let rect = dom.bounding_rect(canvas, snapshot).unwrap();
    assert_eq!(
        (rect.width, rect.height),
        (200.0, 100.0),
        "attaching the widget does not cost the canvas its intrinsic size"
    );
}

#[test]
fn canvas_backing_stores_follow_their_content_attributes() {
    let mut dom = viewport_document(
        r#"<canvas id="default"></canvas>
           <canvas id="sized" width="200" height="100"></canvas>
           <canvas id="invalid" width="-4" height="12.5"></canvas>"#,
        1.0,
    );
    dom.flush_layout().unwrap();

    assert_eq!(backing_store(&dom, "default"), (300, 150));
    assert_eq!(backing_store(&dom, "sized"), (200, 100));
    assert_eq!(
        backing_store(&dom, "invalid"),
        (300, 150),
        "a dimension that is not a non-negative integer falls back to the default"
    );

    let sized = dom.get_element_by_id("sized").unwrap().unwrap();
    let before = revision(&dom, "sized");
    dom.set_attribute(sized, &DomName::attribute("width"), "400")
        .unwrap();
    assert_eq!(backing_store(&dom, "sized"), (400, 100));
    assert_eq!(
        revision(&dom, "sized"),
        before + 1,
        "a resize replaces the backing store"
    );

    dom.set_attribute(sized, &DomName::attribute("width"), "400")
        .unwrap();
    assert_eq!(
        revision(&dom, "sized"),
        before + 1,
        "an attribute written with the size it already had replaces nothing"
    );

    dom.remove_attribute(sized, &DomName::attribute("width"))
        .unwrap();
    assert_eq!(
        backing_store(&dom, "sized"),
        (300, 100),
        "removing the attribute restores the default rather than the last value"
    );
}

#[test]
fn canvas_state_is_forgotten_when_the_element_goes_away() {
    let mut dom = viewport_document(r#"<canvas id="canvas"></canvas>"#, 1.0);
    dom.flush_layout().unwrap();
    let canvas = dom.get_element_by_id("canvas").unwrap().unwrap();
    assert!(dom.canvases.contains_key(&canvas));

    dom.remove(canvas).unwrap();
    dom.flush_layout().unwrap();
    assert!(
        !dom.canvases.contains_key(&canvas),
        "a collected canvas does not keep its backing store alive"
    );
}

#[test]
fn canvas_contents_scale_from_the_backing_store_into_the_box() {
    // Backing store 100x50 in a 200x100 box: everything recorded is drawn at
    // twice the size, which is what a browser does when the attribute size and
    // the style size disagree.
    let mut dom = viewport_document(
        r#"<canvas id="canvas" width="100" height="50"
                   style="width: 200px; height: 100px"></canvas>"#,
        1.0,
    );
    dom.flush_layout().unwrap();
    let canvas = dom.get_element_by_id("canvas").unwrap().unwrap();

    // A quarter of the backing store, in the corner the box's origin is at.
    dom.canvases[&canvas].borrow_mut().record(|scene| {
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            Paint::Solid(Color::from_rgba8(0, 200, 40, 255)),
            None,
            &Rect::new(0.0, 0.0, 50.0, 25.0),
        );
    });

    let pixels = render(&mut dom, 300, 200);
    assert_eq!(
        pixel(&pixels, 300, 20, 20),
        [0, 200, 40, 255],
        "the recorded fill reaches the composited frame"
    );
    assert_eq!(
        pixel(&pixels, 300, 80, 40),
        [0, 200, 40, 255],
        "and covers the doubled quarter, not the recorded one"
    );
    assert_eq!(
        pixel(&pixels, 300, 120, 40),
        [0, 0, 0, 0],
        "the fill stops where the scaled quarter does"
    );
    assert_eq!(
        pixel(&pixels, 300, 250, 150),
        [0, 0, 0, 0],
        "and the canvas paints nothing outside its own box"
    );
}
