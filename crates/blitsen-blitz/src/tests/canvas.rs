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
use blitsen_dom::{CanvasCommands, CanvasTextStyle};
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

/// Builds a command stream the way the bootstrap does, one command at a time.
///
/// The bootstrap is the real writer of this format; this is the second one, and
/// it exists so the reader can be tested against a stream written from the
/// specification rather than from the same code that reads it.
#[derive(Default)]
struct Commands {
    numbers: Vec<f64>,
    strings: Vec<String>,
    pixels: Vec<u8>,
}

/// The identity transform, as six numbers in the order the wire carries them.
const IDENTITY: [f64; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

impl Commands {
    /// Opens a stream that names no image sources.
    fn new() -> Self {
        Self {
            numbers: vec![0.0],
            ..Default::default()
        }
    }

    /// Opens a stream carrying one image, whose index is zero.
    fn with_image(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        Self {
            numbers: vec![1.0, 0.0, f64::from(width), f64::from(height), 0.0],
            strings: Vec::new(),
            pixels,
        }
    }

    fn push(&mut self, values: impl IntoIterator<Item = f64>) -> &mut Self {
        self.numbers.extend(values);
        self
    }

    /// A solid paint, as four components in the 0–1 range.
    fn solid(&mut self, rgba: [f64; 4]) -> &mut Self {
        self.push([0.0]).push(rgba)
    }

    /// A rectangle, as the four-command path the bootstrap records one as.
    fn rect(&mut self, x: f64, y: f64, width: f64, height: f64) -> &mut Self {
        self.push([
            13.0,
            0.0,
            x,
            y,
            1.0,
            x + width,
            y,
            1.0,
            x + width,
            y + height,
            1.0,
            x,
            y + height,
            4.0,
        ])
    }

    fn string(&mut self, value: &str) -> &mut Self {
        self.strings.push(value.to_owned());
        self.push([(self.strings.len() - 1) as f64])
    }

    fn commands(&self) -> CanvasCommands<'_> {
        CanvasCommands {
            numbers: &self.numbers,
            strings: &self.strings,
            pixels: &self.pixels,
        }
    }
}

/// A document with one canvas, laid out and ready to be drawn into.
fn canvas_document(attributes: &str) -> (BlitzDom, blitz::dom::NodeId) {
    let mut dom = viewport_document(
        &format!(r#"<canvas id="canvas" {attributes}></canvas>"#),
        1.0,
    );
    dom.flush_layout().unwrap();
    let canvas = dom.get_element_by_id("canvas").unwrap().unwrap();
    (dom, canvas)
}

#[test]
fn a_filled_path_reaches_the_composited_frame_and_the_readback() {
    let (mut dom, canvas) = canvas_document(r#"width="100" height="50""#);
    let mut commands = Commands::new();
    commands.push([1.0]).solid([0.0, 1.0, 0.0, 1.0]).push([0.0]);
    commands.push(IDENTITY).rect(10.0, 10.0, 40.0, 20.0);
    dom.submit_canvas(canvas, commands.commands()).unwrap();

    let pixels = render(&mut dom, 100, 50);
    assert_eq!(
        pixel(&pixels, 100, 20, 20),
        [0, 255, 0, 255],
        "the fill composites into the document's own frame"
    );
    assert_eq!(pixel(&pixels, 100, 5, 5), [0, 0, 0, 0]);

    let read = dom.canvas_pixels(canvas, 0.0, 0.0, 100, 50).unwrap();
    assert_eq!(
        pixel(&read, 100, 20, 20),
        [0, 255, 0, 255],
        "and the readback rasterises the same recorded scene"
    );
    assert_eq!(pixel(&read, 100, 60, 40), [0, 0, 0, 0]);
}

#[test]
fn a_stroke_paints_the_pen_rather_than_the_path() {
    let (mut dom, canvas) = canvas_document(r#"width="60" height="60""#);
    let mut commands = Commands::new();
    // Opcode 2 is a stroke: paint, then width, cap, join, miter limit, dash
    // offset and dash count, then the transform and the path.
    commands.push([2.0]).solid([0.0, 0.0, 1.0, 1.0]);
    commands.push([10.0, 0.0, 0.0, 10.0, 0.0, 0.0]);
    commands.push(IDENTITY).rect(15.0, 15.0, 30.0, 30.0);
    dom.submit_canvas(canvas, commands.commands()).unwrap();

    let read = dom.canvas_pixels(canvas, 0.0, 0.0, 60, 60).unwrap();
    assert_eq!(
        pixel(&read, 60, 15, 30),
        [0, 0, 255, 255],
        "the pen covers the path it was drawn along"
    );
    assert_eq!(
        pixel(&read, 60, 30, 30),
        [0, 0, 0, 0],
        "and a stroked rectangle is not a filled one"
    );
}

#[test]
fn a_clip_layer_bounds_what_is_drawn_after_it() {
    let (mut dom, canvas) = canvas_document(r#"width="80" height="80""#);
    let mut commands = Commands::new();
    // Push a clip over the top-left quarter, fill everything, pop.
    commands
        .push([3.0])
        .push(IDENTITY)
        .rect(0.0, 0.0, 40.0, 40.0);
    commands.push([1.0]).solid([1.0, 0.0, 0.0, 1.0]).push([0.0]);
    commands.push(IDENTITY).rect(0.0, 0.0, 80.0, 80.0);
    commands.push([5.0]);
    dom.submit_canvas(canvas, commands.commands()).unwrap();

    let read = dom.canvas_pixels(canvas, 0.0, 0.0, 80, 80).unwrap();
    assert_eq!(pixel(&read, 80, 20, 20), [255, 0, 0, 255]);
    assert_eq!(
        pixel(&read, 80, 60, 60),
        [0, 0, 0, 0],
        "the fill stops at the clip rather than at the canvas"
    );
}

#[test]
fn a_stream_that_leaves_a_layer_open_is_refused_whole() {
    let (mut dom, canvas) = canvas_document(r#"width="40" height="40""#);
    let mut commands = Commands::new();
    commands
        .push([3.0])
        .push(IDENTITY)
        .rect(0.0, 0.0, 10.0, 10.0);
    let error = dom.submit_canvas(canvas, commands.commands()).unwrap_err();
    assert!(
        format!("{error:?}").contains("unbalanced"),
        "an unbalanced stream names what is wrong with it: {error:?}"
    );
}

#[test]
fn put_image_data_replaces_pixels_rather_than_drawing_over_them() {
    let (mut dom, canvas) = canvas_document(r#"width="40" height="40""#);
    let mut opaque = Commands::new();
    opaque.push([1.0]).solid([1.0, 0.0, 0.0, 1.0]).push([0.0]);
    opaque.push(IDENTITY).rect(0.0, 0.0, 40.0, 40.0);
    dom.submit_canvas(canvas, opaque.commands()).unwrap();

    // Two by two, half-transparent green. Written through, so what comes back
    // is the alpha that was put in rather than green over red.
    let mut commands = Commands::with_image(2, 2, [0, 255, 0, 128].repeat(4));
    commands.push([8.0, 0.0, 4.0, 4.0, 4.0, 4.0, 2.0, 2.0]);
    dom.submit_canvas(canvas, commands.commands()).unwrap();

    let read = dom.canvas_pixels(canvas, 0.0, 0.0, 40, 40).unwrap();
    assert_eq!(pixel(&read, 40, 5, 5), [0, 255, 0, 128]);
    assert_eq!(
        pixel(&read, 40, 20, 20),
        [255, 0, 0, 255],
        "and only inside the rectangle it was given"
    );
}

#[test]
fn text_is_shaped_from_the_documents_own_fonts_and_measures_what_it_draws() {
    let (mut dom, canvas) = canvas_document(r#"width="200" height="60""#);
    let style = CanvasTextStyle {
        families: "sans-serif",
        size: 32.0,
        weight: 400.0,
        style: 0,
        stretch: 100.0,
        align: 0,
        baseline: 0,
        rtl: false,
    };
    let metrics = dom.measure_canvas_text(style, "Hi").unwrap();
    assert!(
        metrics.width > 0.0,
        "a measured run has an advance: {metrics:?}"
    );
    assert!(
        metrics.actual_ascent > 0.0 && metrics.font_ascent > 0.0,
        "and ink above the baseline: {metrics:?}"
    );

    let mut commands = Commands::new();
    // Opcode 6 is text: paint, fill-or-stroke, transform, then the font, the
    // anchor and the string.
    commands.push([6.0]).solid([0.0, 0.0, 0.0, 1.0]).push([0.0]);
    commands.push(IDENTITY);
    commands.string("sans-serif");
    commands.push([32.0, 400.0, 0.0, 100.0]);
    commands.push([0.0, 0.0, 0.0]);
    commands.push([10.0, 40.0, 0.0]);
    commands.string("Hi");
    dom.submit_canvas(canvas, commands.commands()).unwrap();

    let read = dom.canvas_pixels(canvas, 0.0, 0.0, 200, 60).unwrap();
    let bounds = inked_bounds(&read, 200).expect("text painted glyphs");
    assert!(
        bounds.0 >= 9 && bounds.1 < 40,
        "the run starts at the anchor and sits above the baseline: {bounds:?}"
    );
    assert!(
        f64::from(bounds.2) <= metrics.width + 2.0,
        "and is no wider than it measured: {bounds:?} against {}",
        metrics.width
    );
}

#[test]
fn a_point_is_inside_a_path_by_the_rule_it_is_asked_about() {
    let (mut dom, _) = canvas_document("");
    // A square with a smaller square inside it, both wound the same way: the
    // hole is a hole only under the even-odd rule.
    let mut geometry = Commands::default();
    geometry.push([0.0]).push(IDENTITY);
    geometry.push([26.0]);
    geometry.push([
        0.0, 0.0, 0.0, 1.0, 100.0, 0.0, 1.0, 100.0, 100.0, 1.0, 0.0, 100.0, 4.0,
    ]);
    geometry.push([
        0.0, 25.0, 25.0, 1.0, 75.0, 25.0, 1.0, 75.0, 75.0, 1.0, 25.0, 75.0, 4.0,
    ]);
    geometry.push([50.0, 50.0]);
    assert!(
        dom.canvas_contains(false, &geometry.numbers).unwrap(),
        "the inner square is filled under the non-zero rule"
    );

    geometry.numbers[0] = 1.0;
    assert!(
        !dom.canvas_contains(false, &geometry.numbers).unwrap(),
        "and is a hole under the even-odd rule"
    );
}

#[test]
fn a_canvas_draws_before_it_is_ever_in_the_document() {
    let mut dom = viewport_document("<div id=\"host\"></div>", 1.0);
    dom.flush_layout().unwrap();
    let canvas = dom.create_element(&DomName::html("canvas")).unwrap();
    dom.set_attribute(canvas, &DomName::attribute("width"), "20")
        .unwrap();
    dom.set_attribute(canvas, &DomName::attribute("height"), "20")
        .unwrap();

    let mut commands = Commands::new();
    commands.push([1.0]).solid([1.0, 1.0, 0.0, 1.0]).push([0.0]);
    commands.push(IDENTITY).rect(0.0, 0.0, 20.0, 20.0);
    dom.submit_canvas(canvas, commands.commands()).unwrap();
    assert_eq!(
        pixel(
            &dom.canvas_pixels(canvas, 0.0, 0.0, 20, 20).unwrap(),
            20,
            5,
            5
        ),
        [255, 255, 0, 255],
        "a canvas made with createElement draws without being connected"
    );

    // And once it is connected, the contents it already had are painted.
    let host = dom.get_element_by_id("host").unwrap().unwrap();
    dom.append_child(host, canvas).unwrap();
    dom.flush_layout().unwrap();
    let frame = render(&mut dom, 40, 40);
    assert_eq!(pixel(&frame, 40, 5, 5), [255, 255, 0, 255]);
}

#[test]
fn a_canvas_encodes_itself_as_a_data_url() {
    let (mut dom, canvas) = canvas_document(r#"width="8" height="8""#);
    let mut commands = Commands::new();
    commands.push([1.0]).solid([1.0, 0.0, 1.0, 1.0]).push([0.0]);
    commands.push(IDENTITY).rect(0.0, 0.0, 8.0, 8.0);
    dom.submit_canvas(canvas, commands.commands()).unwrap();

    let url = dom.canvas_data_url(canvas, "image/png", 1.0).unwrap();
    assert!(url.starts_with("data:image/png;base64,"), "{url}");
    let jpeg = dom.canvas_data_url(canvas, "image/jpeg", 0.8).unwrap();
    assert!(jpeg.starts_with("data:image/jpeg;base64,"), "{jpeg}");
    assert!(
        dom.encode_canvas(canvas, "image/webp", 1.0)
            .unwrap()
            .mime_type
            == "image/png",
        "a type this canvas cannot encode falls back to PNG, as the specification says"
    );
}

#[test]
fn a_destructive_composite_erases_the_canvas_and_not_the_page_behind_it() {
    let mut dom = viewport_document(
        r#"<div style="background: rgb(0,0,255); width: 100px; height: 100px">
             <canvas id="canvas" width="60" height="60"></canvas>
           </div>"#,
        1.0,
    );
    dom.flush_layout().unwrap();
    let canvas = dom.get_element_by_id("canvas").unwrap().unwrap();

    let mut commands = Commands::new();
    commands.push([1.0]).solid([1.0, 0.0, 0.0, 1.0]).push([0.0]);
    commands.push(IDENTITY).rect(0.0, 0.0, 60.0, 60.0);
    // `copy` over a small square: everywhere else on the canvas is cleared,
    // which is what a browser does, and the page behind it is not.
    commands.push([4.0, 9.0, 1.0]).push(IDENTITY);
    commands.rect(0.0, 0.0, 20.0, 20.0);
    commands.push([1.0]).solid([0.0, 1.0, 0.0, 1.0]).push([0.0]);
    commands.push(IDENTITY).rect(0.0, 0.0, 20.0, 20.0);
    commands.push([5.0]);
    dom.submit_canvas(canvas, commands.commands()).unwrap();

    let frame = render(&mut dom, 100, 100);
    assert_eq!(pixel(&frame, 100, 10, 10), [0, 255, 0, 255]);
    assert_eq!(
        pixel(&frame, 100, 40, 40),
        [0, 0, 255, 255],
        "the canvas erased itself where the copy had no source"
    );
    assert_eq!(
        pixel(&frame, 100, 80, 80),
        [0, 0, 255, 255],
        "and the document behind it is untouched"
    );
}

#[test]
fn a_clear_erases_a_rectangle_and_leaves_the_rest_of_the_canvas() {
    let (mut dom, canvas) = canvas_document(r#"width="40" height="40""#);
    let mut commands = Commands::new();
    commands.push([1.0]).solid([1.0, 0.0, 0.0, 1.0]).push([0.0]);
    commands.push(IDENTITY).rect(0.0, 0.0, 40.0, 40.0);
    // `clearRect` is a `destination-out` layer with an opaque shape in it.
    commands.push([4.0, 6.0, 1.0]).push(IDENTITY);
    commands.rect(10.0, 10.0, 10.0, 10.0);
    commands.push([1.0]).solid([0.0, 0.0, 0.0, 1.0]).push([0.0]);
    commands.push(IDENTITY).rect(10.0, 10.0, 10.0, 10.0);
    commands.push([5.0]);
    dom.submit_canvas(canvas, commands.commands()).unwrap();

    let read = dom.canvas_pixels(canvas, 0.0, 0.0, 40, 40).unwrap();
    assert_eq!(pixel(&read, 40, 15, 15), [0, 0, 0, 0]);
    assert_eq!(
        pixel(&read, 40, 30, 30),
        [255, 0, 0, 255],
        "clearing a rectangle clears that rectangle and nothing else"
    );
}

#[test]
fn a_canvas_is_its_own_image_source_without_aliasing_its_contents() {
    let (mut dom, canvas) = canvas_document(r#"width="40" height="40""#);
    let mut commands = Commands::new();
    commands.push([1.0]).solid([1.0, 0.0, 0.0, 1.0]).push([0.0]);
    commands.push(IDENTITY).rect(0.0, 0.0, 10.0, 10.0);
    dom.submit_canvas(canvas, commands.commands()).unwrap();

    // The source and the destination are one element, which is the case where
    // resolving sources lazily would try to borrow the same contents twice.
    // Opcode 7 is an image: source, quality, transform, source rect, dest rect.
    let mut copy = Commands::default();
    copy.push([1.0, 1.0, canvas.as_u64() as f64]);
    copy.push([7.0, 0.0, 0.0]).push(IDENTITY);
    copy.push([0.0, 0.0, 10.0, 10.0, 20.0, 20.0, 10.0, 10.0]);
    dom.submit_canvas(canvas, copy.commands()).unwrap();

    let read = dom.canvas_pixels(canvas, 0.0, 0.0, 40, 40).unwrap();
    assert_eq!(pixel(&read, 40, 5, 5), [255, 0, 0, 255]);
    assert_eq!(
        pixel(&read, 40, 25, 25),
        [255, 0, 0, 255],
        "a canvas drawn into itself copies what it held when the draw was recorded"
    );
}
