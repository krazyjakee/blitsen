//! SVG: an inline `<svg>` subtree, and SVG as a subresource (issue #238).
//!
//! Both halves are Blitz's, and both were dark until the pin moved: `blitz-dom`'s
//! `svg` feature parses the element's own markup with usvg, and `blitz-paint`'s
//! turns the parsed tree into paint commands through `anyrender_svg`. What these
//! tests gate is the boundary — which paths are live, what reaches them from the
//! cascade, and what a mutation does — because none of it is visible from the
//! DOM: `createElementNS` and `querySelector` behaved identically while nothing
//! painted at all, which is what made this worth a test rather than a claim.

use super::*;

/// Counts pixels of any colour inside a rectangle.
fn inked_within(pixels: &[u8], width: u32, rect: blitsen_dom::Rect) -> u32 {
    let mut inked = 0;
    for y in rect.y as u32..(rect.y + rect.height) as u32 {
        for x in rect.x as u32..(rect.x + rect.width) as u32 {
            if pixel(pixels, width, x, y)[3] > 0 {
                inked += 1;
            }
        }
    }
    inked
}

#[test]
fn an_inline_svg_paints_inside_the_box_its_attributes_ask_for() {
    let mut dom = fixture_document(
        r##"<svg id="icon" width="100" height="60" viewBox="0 0 10 6">
              <rect width="10" height="6" fill="#ef4444"></rect>
            </svg>"##,
        None,
    );
    let snapshot = dom.flush_layout().unwrap();
    let icon = dom.get_element_by_id("icon").unwrap().unwrap();
    let rect = dom.layout_metrics(icon, snapshot).unwrap().rect;
    assert_eq!((rect.width, rect.height), (100.0, 60.0));
    let pixels = render(&mut dom, 400, 200);
    assert_eq!(
        pixel(&pixels, 400, 50, 30),
        [239, 68, 68, 255],
        "the rect fills the element's box"
    );
    assert_eq!(
        inked_bounds(&pixels, 400),
        Some((0, 0, 100, 60)),
        "the viewBox is mapped onto the element's box, so a 10x6 rect covers 100x60"
    );
}

#[test]
fn css_sizes_the_element_and_the_drawing_scales_with_it() {
    let mut dom = fixture_document(
        r##"<svg id="icon" width="100" height="60" viewBox="0 0 10 6"
                 style="width: 50px; height: 30px">
              <rect width="10" height="6" fill="#ef4444"></rect>
            </svg>"##,
        None,
    );
    dom.flush_layout().unwrap();
    let pixels = render(&mut dom, 400, 200);
    assert_eq!(
        inked_bounds(&pixels, 400),
        Some((0, 0, 50, 30)),
        "author CSS wins over the attributes, and the drawing follows the box"
    );
}

#[test]
fn current_color_is_the_colour_the_cascade_resolved() {
    let mut dom = fixture_document(
        r##"<div style="color: #22c55e">
              <svg id="icon" width="40" height="40" viewBox="0 0 24 24" fill="none"
                   stroke="currentColor" stroke-width="8">
                <path d="M2 12 L22 12"></path>
              </svg>
            </div>"##,
        None,
    );
    dom.flush_layout().unwrap();
    let pixels = render(&mut dom, 400, 200);
    assert_eq!(
        pixel(&pixels, 400, 20, 20),
        [34, 197, 94, 255],
        "an icon set's stroke=\"currentColor\" takes the inherited colour, not black"
    );
}

#[test]
fn a_mutation_inside_the_subtree_repaints_it() {
    let mut dom = fixture_document(
        r##"<svg id="icon" width="100" height="60" viewBox="0 0 10 6">
              <rect id="fill" width="10" height="6" fill="#ef4444"></rect>
            </svg>"##,
        None,
    );
    dom.flush_layout().unwrap();
    assert_eq!(
        pixel(&render(&mut dom, 400, 200), 400, 50, 30),
        [239, 68, 68, 255]
    );

    // The whole of what a charting library does: set an attribute on a child
    // and expect the frame to follow. The subtree is re-parsed rather than
    // patched, because what is painted is a usvg tree built from the element's
    // own markup — so the cost is per mutation, and a chart that redraws every
    // frame re-parses every frame.
    let rect = dom.get_element_by_id("fill").unwrap().unwrap();
    dom.set_attribute(rect, &DomName::attribute("fill"), "#2563eb")
        .unwrap();
    dom.flush_layout().unwrap();
    assert_eq!(
        pixel(&render(&mut dom, 400, 200), 400, 50, 30),
        [37, 99, 235, 255],
        "an attribute set on an SVG child is painted, not just recorded in the DOM"
    );
}

#[test]
fn svg_subresources_paint_through_img_and_background_image() {
    let mut dom = fixture_document(
        r#"<img id="logo" src="icon.svg">
           <div id="tile" style="width: 40px; height: 20px; background-image: url(icon.svg)"></div>"#,
        None,
    );
    let snapshot = dom.flush_layout().unwrap();
    let logo = dom.get_element_by_id("logo").unwrap().unwrap();
    let rect = dom.layout_metrics(logo, snapshot).unwrap().rect;
    assert_eq!(
        (rect.width, rect.height),
        (40.0, 20.0),
        "an SVG image has the intrinsic size its own width and height declare"
    );
    let pixels = render(&mut dom, 400, 200);
    assert_eq!(
        pixel(&pixels, 400, 20, 10),
        [37, 99, 235, 255],
        "<img src=\"*.svg\"> paints rather than being answered with an empty body"
    );
    let tile = dom.get_element_by_id("tile").unwrap().unwrap();
    let tile_rect = dom.layout_metrics(tile, snapshot).unwrap().rect;
    assert_eq!(
        pixel(&pixels, 400, 20, tile_rect.y as u32 + 10),
        [37, 99, 235, 255],
        "a CSS background-image naming an SVG paints too"
    );
}

#[test]
fn text_inside_an_svg_is_painted_as_glyphs() {
    let mut dom = fixture_document(
        r##"<svg id="label" width="100" height="40" viewBox="0 0 100 40">
              <text x="0" y="30" font-size="30" fill="#000000">Hi</text>
            </svg>"##,
        None,
    );
    let snapshot = dom.flush_layout().unwrap();
    let label = dom.get_element_by_id("label").unwrap().unwrap();
    let rect = dom.layout_metrics(label, snapshot).unwrap().rect;
    let pixels = render(&mut dom, 400, 200);
    // usvg flattens `<text>` to outlines through the same font database Blitz
    // shapes HTML text with, so a chart's axis labels paint. The count is a
    // floor rather than an exact figure: which glyphs the host has is not this
    // test's business, only that something was drawn where the label is.
    assert!(
        inked_within(&pixels, 400, rect) > 200,
        "<text> paints glyphs rather than nothing: {:?}",
        inked_bounds(&pixels, 400)
    );
}

#[test]
fn gradients_and_stroke_options_are_painted_rather_than_dropped() {
    let mut dom = fixture_document(
        r##"<svg id="chart" width="100" height="40" viewBox="0 0 100 40">
              <defs>
                <linearGradient id="fade" x1="0" y1="0" x2="1" y2="0">
                  <stop offset="0" stop-color="#ff0000"></stop>
                  <stop offset="1" stop-color="#0000ff"></stop>
                </linearGradient>
              </defs>
              <rect width="100" height="20" fill="url(#fade)"></rect>
              <path d="M0 30 L100 30" stroke="#16a34a" stroke-width="8"
                    stroke-dasharray="10 10"></path>
            </svg>"##,
        None,
    );
    dom.flush_layout().unwrap();
    let pixels = render(&mut dom, 400, 200);
    let left = pixel(&pixels, 400, 2, 10);
    let right = pixel(&pixels, 400, 97, 10);
    assert!(
        left[0] > 200 && left[2] < 60,
        "the gradient starts red: {left:?}"
    );
    assert!(
        right[2] > 200 && right[0] < 60,
        "and ends blue, which is a gradient rather than a flat fill: {right:?}"
    );
    assert_eq!(
        pixel(&pixels, 400, 2, 30),
        [22, 163, 74, 255],
        "the dashed stroke paints where a dash is"
    );
    assert_eq!(
        pixel(&pixels, 400, 14, 30),
        [0, 0, 0, 0],
        "and leaves the gap between dashes empty"
    );
}

/// A `<pattern>` fill is the one shape here that is both unsupported *and*
/// destructive, and it is gated so that the day it changes is a failing test.
///
/// `anyrender_svg` cannot convert a pattern paint, so it hands the node to its
/// error handler — which fills the node's bounding box with half-transparent
/// red under the *identity* transform rather than under the transform the node
/// is painted at. The mark therefore lands in the top-left corner of the frame,
/// over whatever was already there, instead of over the element that could not
/// be painted. Gap G16 in BLITZ-GAPS.md.
#[test]
fn an_unsupported_pattern_fill_marks_the_frame_corner_rather_than_the_element() {
    let mut dom = fixture_document(
        r##"<div id="tile" style="width: 40px; height: 20px; background-image: url(icon.svg)"></div>
            <div style="padding-left: 100px">
            <svg id="patterned" width="40" height="40">
              <defs>
                <pattern id="p" width="4" height="4" patternUnits="userSpaceOnUse">
                  <rect width="2" height="2" fill="#16a34a"></rect>
                </pattern>
              </defs>
              <rect width="40" height="40" fill="url(#p)"></rect>
            </svg>
            </div>"##,
        None,
    );
    let snapshot = dom.flush_layout().unwrap();
    let patterned = dom.get_element_by_id("patterned").unwrap().unwrap();
    let rect = dom.layout_metrics(patterned, snapshot).unwrap().rect;
    assert!(
        rect.x > 40.0,
        "the patterned element is not at the origin: {rect:?}"
    );
    let pixels = render(&mut dom, 400, 200);
    assert_eq!(
        inked_within(&pixels, 400, rect),
        0,
        "nothing paints where the pattern is"
    );
    assert_eq!(
        pixel(&pixels, 400, 20, 10),
        [147, 50, 117, 255],
        "the mark is red at half alpha over the blue tile, in the frame's corner"
    );
}
