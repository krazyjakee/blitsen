//! The surface behind one `<canvas>` element, and the 2D context over it.
//!
//! Blitz owns the element's box — `<canvas>` is already a replaced element
//! upstream, sized from its `width`/`height` content attributes and defaulting
//! to 300×150 — and this module owns what is drawn inside it.
//!
//! Contents are a recorded [`Scene`], not pixels. `Widget::paint` returns an
//! anyrender command list that whichever backend is live replays: vello on the
//! GPU in the window, vello_cpu headless and in the tests. So a canvas costs no
//! rasterisation and no upload on the paint path, and it composites in the same
//! frame as the DOM at the element's own paint position — z-order, ancestor
//! `overflow` and `border-radius` come from that position rather than from a
//! second pass. This is what `<blitsen-view>` pays a full-frame RGBA upload for
//! (see [`crate::viewport`]).
//!
//! Rasterisation is still needed, but only where the specification demands a
//! readback — `getImageData`, `toDataURL` — and not once per frame. See
//! [`readback`].
//!
//! The backing store is in canvas pixels and is independent of the box: the
//! recorded scene is scaled into whatever CSS makes the element, exactly as a
//! browser scales a canvas whose attribute size and style size disagree.
//!
//! **Nothing here holds 2D context state.** The transform stack, the paint
//! styles, the current path and `save`/`restore` are all JavaScript's; what
//! crosses the boundary is a stream of self-contained drawing commands. See
//! [`wire`] for the shape of that stream and why it has that shape.

mod readback;
mod record;
mod text;
mod wire;

use std::cell::RefCell;
use std::rc::Rc;

use anyrender::{PaintScene as _, RenderContext, Scene};
use blitsen_dom::{
    CanvasCommands, CanvasEncoding, CanvasSurface, CanvasTextMetrics, CanvasTextStyle, DomBackend,
    DomError, DomName,
};
use blitz::dom::node::{ComputedStyles, ImageData as BlitzImageData};
use blitz::dom::{NodeId, Widget};
use kurbo::{Affine, Rect, Shape as _};
use peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};

use crate::BlitzDom;
use crate::surface::{Surface, SurfaceWidget, attach_widgets};

use readback::CanvasImageFormat;
pub(crate) use text::TextEngine;
use text::{ShapedText, TextRequest};
use wire::Reader;

/// The tag whose elements carry a canvas.
pub(crate) const CANVAS_TAG: &str = "canvas";

/// The backing store a canvas has when its attributes do not say otherwise.
///
/// The same pair as the default object size Blitz lays the element out at, so
/// an unconfigured canvas draws at one backing-store pixel per CSS pixel.
const DEFAULT_SIZE: (u32, u32) = (300, 150);

/// What a malformed or impossible canvas operation is refused with.
///
/// Every one of these is a bug in the bootstrap rather than in an application:
/// the command stream is written by code this crate ships beside, so a
/// truncated or unbalanced one means the two halves disagree. They are still
/// errors rather than assertions, because a refused stream leaves the canvas
/// with the contents it had and an exception at the call, and a panic across
/// the native boundary would take the process down.
#[derive(Debug)]
pub(crate) enum CanvasError {
    /// The stream ended in the middle of a command.
    Truncated,
    /// A tag, count or structure the reader does not recognise.
    Malformed,
    /// The stream closed a layer it did not open, or left one open.
    Unbalanced,
    /// An image encoder refused the pixels it was given.
    Encode(String),
}

impl From<CanvasError> for DomError {
    fn from(error: CanvasError) -> Self {
        Self::Backend(match error {
            CanvasError::Truncated => "canvas command stream ended mid-command".into(),
            CanvasError::Malformed => "canvas command stream is malformed".into(),
            CanvasError::Unbalanced => "canvas command stream leaves layers unbalanced".into(),
            CanvasError::Encode(message) => format!("canvas image encoding failed: {message}"),
        })
    }
}

/// Parses a `width`/`height` content attribute into a backing-store dimension.
///
/// HTML asks for a non-negative integer and falls back to the default for
/// anything else, which includes a negative number, a float and a unit.
fn dimension(value: Option<&str>, default: u32) -> u32 {
    value
        .and_then(|value| value.trim().parse::<u32>().ok())
        .unwrap_or(default)
}

/// The backing store and recorded contents shared with the DOM bridge.
#[derive(Debug)]
pub(crate) struct CanvasState {
    width: u32,
    height: u32,
    revision: u64,
    attached: bool,
    destructive: bool,
    scene: Scene,
}

impl Default for CanvasState {
    fn default() -> Self {
        Self {
            width: DEFAULT_SIZE.0,
            height: DEFAULT_SIZE.1,
            revision: 0,
            attached: false,
            destructive: false,
            scene: Scene::new(),
        }
    }
}

impl Surface for CanvasState {
    fn revision(&self) -> u64 {
        self.revision
    }

    fn is_attached(&self) -> bool {
        self.attached
    }

    fn mark_attached(&mut self) {
        self.attached = true;
    }
}

impl CanvasState {
    /// Adopts a backing-store size, reporting whether it changed.
    ///
    /// Contents recorded for the previous size are dropped. That is not an
    /// optimisation — HTML says assigning either dimension clears the canvas to
    /// transparent black.
    pub(crate) fn resize(&mut self, width: u32, height: u32) -> bool {
        if self.width == width && self.height == height {
            return false;
        }
        self.width = width;
        self.height = height;
        self.clear();
        true
    }

    /// Discards the recorded contents, leaving the canvas transparent black.
    pub(crate) fn clear(&mut self) {
        self.scene.reset();
        self.destructive = false;
        self.revision += 1;
    }

    /// Whether the recorded contents can erase what is painted under them.
    ///
    /// A `globalCompositeOperation` such as `copy` or `source-in` clears the
    /// canvas everywhere its source is absent, and a recorded scene is replayed
    /// into the document's own scene rather than onto a bitmap of its own — so
    /// left alone, one of those would erase the page behind the element. It is
    /// tracked rather than assumed because the answer decides whether the
    /// element costs a compositing layer per frame, and almost no canvas uses
    /// one of the six.
    pub(crate) fn is_destructive(&self) -> bool {
        self.destructive
    }

    /// Records test drawing commands into the canvas's own coordinate space.
    #[cfg(test)]
    pub(crate) fn record(&mut self, ops: impl FnOnce(&mut Scene)) {
        ops(&mut self.scene);
        self.revision += 1;
    }

    /// Reports the backing store size in canvas pixels.
    pub(crate) fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

/// Paints one `<canvas>` element's recorded contents into the document's scene.
pub(crate) struct CanvasWidget(SurfaceWidget<CanvasState>);

impl CanvasWidget {
    pub(crate) fn new(state: Rc<RefCell<CanvasState>>) -> Self {
        Self(SurfaceWidget::new(state))
    }
}

impl Widget for CanvasWidget {
    fn requires_redraw(&self) -> bool {
        self.0.needs_repaint()
    }

    fn paint(
        &mut self,
        _context: &mut dyn RenderContext,
        _styles: &ComputedStyles,
        width: u32,
        height: u32,
        _scale: f64,
    ) -> Scene {
        let mut scene = Scene::new();
        let state = self.0.begin_paint();

        let (canvas_width, canvas_height) = state.size();
        // A canvas with no backing store has nowhere to draw and no ratio to
        // scale by; a browser renders it as nothing rather than as an error.
        if canvas_width == 0 || canvas_height == 0 {
            return scene;
        }
        // Contents that can erase are composited as a group, so what they erase
        // is the canvas and not the document under it. Everything else is
        // appended flat, because a layer per canvas per frame is a real cost and
        // the six operations that need one are rare.
        let isolate = state.is_destructive();
        if isolate {
            scene.push_clip_layer(
                Affine::IDENTITY,
                &Rect::new(0.0, 0.0, f64::from(width), f64::from(height)),
            );
        }
        // The recorded scene is in canvas pixels. Everything that makes the box
        // a different size — CSS, the display's density — arrives here as the
        // painted width and height, so one transform covers both.
        scene.append_scene(
            state.scene.clone(),
            Affine::scale_non_uniform(
                f64::from(width) / f64::from(canvas_width),
                f64::from(height) / f64::from(canvas_height),
            ),
        );
        if isolate {
            scene.pop_layer();
        }
        scene
    }
}

/// Where `textAlign` and `textBaseline` put a run relative to its anchor.
pub(crate) struct TextAnchor {
    /// `textAlign`, as the tag the bootstrap writes.
    pub(crate) align: u8,
    /// `textBaseline`, as the tag the bootstrap writes.
    pub(crate) baseline: u8,
    /// Whether the context's `direction` resolved to right-to-left.
    pub(crate) rtl: bool,
}

/// The displacement one anchor puts between the given point and the baseline.
///
/// `start` and `end` are the two that depend on direction, which is the whole
/// reason `direction` reaches this side at all: `left` and `right` mean the
/// same thing either way, and a run with no direction of its own would place
/// `start` correctly only half the time.
pub(crate) fn anchor_offsets(anchor: &TextAnchor, shaped: &ShapedText) -> (f64, f64) {
    let leading = if anchor.rtl { -shaped.width } else { 0.0 };
    let trailing = if anchor.rtl { 0.0 } else { -shaped.width };
    let dx = match anchor.align {
        1 => trailing,
        2 => 0.0,
        3 => -shaped.width,
        4 => -shaped.width / 2.0,
        _ => leading,
    };
    // Positive is downwards, and the anchor is where the *named* baseline sits,
    // so this is how far the alphabetic baseline is from it.
    let dy = match anchor.baseline {
        1 => shaped.font_ascent,
        // The hanging baseline is where Devanagari hangs from. Nothing in an
        // OpenType face is required to record it, so this is the fraction of
        // the ascent every browser uses in its absence.
        2 => shaped.font_ascent * 0.8,
        3 => (shaped.font_ascent - shaped.font_descent) / 2.0,
        4 | 5 => -shaped.font_descent,
        _ => 0.0,
    };
    (dx, dy)
}

impl BlitzDom {
    /// Gives every connected `<canvas>` a backing store, and forgets dead ones.
    ///
    /// Attaching is a tree mutation and so runs before layout resolves, for the
    /// reason [`BlitzDom::attach_native_viewports`] does. The initial
    /// `width`/`height` are read here rather than waiting for a later
    /// attribute write, because a parsed document's attributes are already in
    /// place by the time anything can observe them.
    pub(crate) fn attach_canvases(&mut self) -> Result<(), DomError> {
        attach_widgets(
            self,
            CANVAS_TAG,
            |dom| &mut dom.canvases,
            |dom, node| dom.canvas_state(node),
            |state| Box::new(CanvasWidget::new(state)),
        )
    }

    /// Returns a canvas element's backing store, creating it if it has none.
    ///
    /// A canvas that is not in the document still draws: `createElement`,
    /// draw, `toDataURL` is how an application makes an image without ever
    /// showing one, and it is also how a framework prepares a canvas before
    /// mounting it. So the store is not tied to being connected — only the
    /// widget that paints it is, and [`Self::attach_canvases`] installs that
    /// the first time the element is found in the document.
    fn canvas_state(&mut self, node: NodeId) -> Result<Rc<RefCell<CanvasState>>, DomError> {
        if let Some(state) = self.canvases.get(&node) {
            return Ok(Rc::clone(state));
        }
        if !self.is_tag(node, CANVAS_TAG) {
            return Err(DomError::InvalidNodeType);
        }
        let width = self.attribute(node, &DomName::attribute("width"))?;
        let height = self.attribute(node, &DomName::attribute("height"))?;
        let state = Rc::new(RefCell::new(CanvasState {
            width: dimension(width.as_deref(), DEFAULT_SIZE.0),
            height: dimension(height.as_deref(), DEFAULT_SIZE.1),
            ..Default::default()
        }));
        self.canvases.insert(node, Rc::clone(&state));
        Ok(state)
    }

    /// Re-reads a canvas's backing-store size after one of its attributes moved.
    ///
    /// Through the attribute write rather than through `Widget::attribute_changed`
    /// because a canvas that is not in the document has no widget, and
    /// `canvas.width = 800` on one has to clear it and resize it all the same.
    pub(crate) fn resize_canvas_backing_store(&mut self, node: NodeId, name: &DomName) {
        if !matches!(name.local.as_str(), "width" | "height") {
            return;
        }
        let Some(state) = self.canvases.get(&node).map(Rc::clone) else {
            return;
        };
        let width = self
            .attribute(node, &DomName::attribute("width"))
            .ok()
            .flatten();
        let height = self
            .attribute(node, &DomName::attribute("height"))
            .ok()
            .flatten();
        state.borrow_mut().resize(
            dimension(width.as_deref(), DEFAULT_SIZE.0),
            dimension(height.as_deref(), DEFAULT_SIZE.1),
        );
    }

    /// Reports one canvas's backing store size in canvas pixels.
    pub(crate) fn canvas_backing_store(&mut self, node: NodeId) -> Result<CanvasSurface, DomError> {
        let state = self.canvas_state(node)?;
        let (width, height) = state.borrow().size();
        Ok(CanvasSurface { width, height })
    }

    /// Records one submission of drawing commands into a canvas.
    pub(crate) fn record_canvas(
        &mut self,
        node: NodeId,
        commands: CanvasCommands<'_>,
    ) -> Result<(), DomError> {
        let state = self.canvas_state(node)?;
        let mut reader = Reader::new(commands.numbers, commands.strings);
        let images = self.resolve_canvas_sources(&mut reader, commands.pixels)?;
        let mut engine = self.take_text_engine();
        let mut borrowed = state.borrow_mut();
        let CanvasState {
            scene, destructive, ..
        } = &mut *borrowed;
        let result = record::replay(scene, &mut reader, &images, &mut engine, destructive);
        borrowed.revision += 1;
        drop(borrowed);
        self.text_engine = Some(engine);
        result.map_err(DomError::from)
    }

    /// Reads back a rectangle of a canvas as straight-alpha RGBA8 rows.
    pub(crate) fn read_canvas_pixels(
        &mut self,
        node: NodeId,
        x: f64,
        y: f64,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, DomError> {
        let state = self.canvas_state(node)?;
        let pixels = readback::rasterize(&state.borrow().scene, x, y, width, height);
        Ok(pixels)
    }

    /// Encodes a whole canvas as a complete image file.
    pub(crate) fn encode_canvas_image(
        &mut self,
        node: NodeId,
        mime_type: &str,
        quality: f64,
    ) -> Result<CanvasEncoding, DomError> {
        let state = self.canvas_state(node)?;
        let (width, height) = state.borrow().size();
        let format = CanvasImageFormat::from_mime_type(mime_type);
        if width == 0 || height == 0 {
            return Ok(CanvasEncoding {
                mime_type: format.mime_type(),
                bytes: Vec::new(),
            });
        }
        let pixels = readback::rasterize(&state.borrow().scene, 0.0, 0.0, width, height);
        Ok(CanvasEncoding {
            mime_type: format.mime_type(),
            bytes: readback::encode(&pixels, width, height, format, quality)?,
        })
    }

    /// Encodes a whole canvas as a `data:` URL.
    pub(crate) fn canvas_image_url(
        &mut self,
        node: NodeId,
        mime_type: &str,
        quality: f64,
    ) -> Result<String, DomError> {
        let encoding = self.encode_canvas_image(node, mime_type, quality)?;
        // What a browser answers for a canvas with no pixels to encode: not an
        // image of nothing, but a URL that is explicitly not an image.
        if encoding.bytes.is_empty() {
            return Ok("data:,".into());
        }
        Ok(format!(
            "data:{};base64,{}",
            encoding.mime_type,
            readback::base64(&encoding.bytes)
        ))
    }

    /// Measures a run of text in the font a 2D context is set to.
    pub(crate) fn canvas_text_metrics(
        &mut self,
        style: CanvasTextStyle<'_>,
        text: &str,
    ) -> CanvasTextMetrics {
        let mut engine = self.take_text_engine();
        let shaped = engine.shape(&TextRequest {
            families: style.families,
            size: style.size as f32,
            weight: style.weight as f32,
            style: style.style,
            stretch: style.stretch as f32,
            text,
        });
        self.text_engine = Some(engine);
        let anchor = TextAnchor {
            align: style.align,
            baseline: style.baseline,
            rtl: style.rtl,
        };
        let (dx, dy) = anchor_offsets(&anchor, &shaped);
        CanvasTextMetrics {
            width: shaped.width,
            actual_left: shaped.actual_left - dx,
            actual_right: shaped.actual_right + dx,
            actual_ascent: shaped.actual_ascent - dy,
            actual_descent: shaped.actual_descent + dy,
            font_ascent: shaped.font_ascent - dy,
            font_descent: shaped.font_descent + dy,
        }
    }

    /// Answers `isPointInPath` and `isPointInStroke`.
    pub(crate) fn path_contains_point(
        &mut self,
        stroked: bool,
        geometry: &[f64],
    ) -> Result<bool, DomError> {
        let strings: [String; 0] = [];
        let mut reader = Reader::new(geometry, &strings);
        let rule = wire::fill_rule(reader.tag()?)?;
        let stroke = stroked.then(|| reader.stroke()).transpose()?;
        let transform = reader.transform()?;
        let path = reader.path()?;
        let x = reader.number()?;
        let y = reader.number()?;
        // The point is in canvas space and the path is in the user space the
        // transform describes, so the path is what moves.
        let path = transform * path;
        let path = match stroke {
            // A stroke is hit-tested against the region it paints, which is the
            // outline expanded by the pen — the path itself has no area.
            Some(stroke) => kurbo::stroke(path.iter(), &stroke, &Default::default(), 0.1),
            None => path,
        };
        let winding = path.winding(kurbo::Point::new(x, y));
        Ok(match rule {
            peniko::Fill::EvenOdd => winding % 2 != 0,
            peniko::Fill::NonZero => winding != 0,
        })
    }

    /// Borrows the shaping contexts, building them the first time text is drawn.
    ///
    /// Taken out and put back rather than borrowed, because shaping needs the
    /// engine mutably while the canvas being drawn into is already borrowed
    /// from the same document.
    fn take_text_engine(&mut self) -> TextEngine {
        self.text_engine
            .take()
            .unwrap_or_else(|| TextEngine::new(self.canvas_fonts.clone()))
    }

    /// Resolves the image sources named in a command stream's preamble.
    ///
    /// Every source is resolved before a single command is replayed, and that
    /// ordering is load-bearing: an element source is read out of the document
    /// and a canvas source is rasterised out of another canvas's contents, both
    /// of which need the document while the canvas being drawn into needs it
    /// too. Resolving first means the borrows never overlap — including the one
    /// case where they would alias, a canvas drawn into itself.
    fn resolve_canvas_sources(
        &mut self,
        reader: &mut Reader<'_>,
        pixels: &[u8],
    ) -> Result<Vec<ImageData>, DomError> {
        let count = reader.count()?;
        let mut images = Vec::with_capacity(count);
        for _ in 0..count {
            images.push(match reader.tag()? {
                // Pixels the application supplied, as one run inside the
                // stream's single byte buffer.
                0 => {
                    let width = reader.count()? as u32;
                    let height = reader.count()? as u32;
                    let offset = reader.count()?;
                    let length = width as usize * height as usize * 4;
                    let bytes = pixels
                        .get(offset..offset.saturating_add(length))
                        .ok_or(CanvasError::Truncated)?;
                    ImageData {
                        data: Blob::from(bytes.to_vec()),
                        format: ImageFormat::Rgba8,
                        alpha_type: ImageAlphaType::Alpha,
                        width,
                        height,
                    }
                }
                // An element in this document: an `<img>` that has decoded, or
                // another canvas. Its size is the element's own and is not
                // carried here, because a second copy of it could disagree.
                1 => {
                    let node = NodeId::from_u64(reader.number()? as u64);
                    self.canvas_source_element(node)?
                }
                _ => return Err(CanvasError::Malformed.into()),
            });
        }
        Ok(images)
    }

    /// Resolves one element source into pixels a canvas can sample.
    ///
    /// An element that has nothing to give — an image still loading, one that
    /// failed, a canvas with no backing store — resolves to a one-pixel
    /// transparent image rather than to an error, because `drawImage` with an
    /// unusable source is defined to draw nothing rather than to throw.
    fn canvas_source_element(&mut self, node: NodeId) -> Result<ImageData, DomError> {
        if self.is_tag(node, CANVAS_TAG) {
            let state = self.canvas_state(node)?;
            let (width, height) = state.borrow().size();
            if width == 0 || height == 0 {
                return Ok(empty_image());
            }
            return Ok(readback::to_image(&state.borrow().scene, width, height));
        }
        let raster = self
            .node(node)?
            .element_data()
            .and_then(|element| element.image_data())
            .and_then(|image| match image {
                BlitzImageData::Raster(raster) => Some(raster.clone()),
                _ => None,
            });
        Ok(match raster {
            Some(raster) => ImageData {
                data: raster.data.clone(),
                format: ImageFormat::Rgba8,
                alpha_type: ImageAlphaType::Alpha,
                width: raster.width,
                height: raster.height,
            },
            None => empty_image(),
        })
    }
}

/// One transparent pixel, which draws as nothing at any size.
fn empty_image() -> ImageData {
    ImageData {
        data: Blob::from(vec![0_u8; 4]),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width: 1,
        height: 1,
    }
}
