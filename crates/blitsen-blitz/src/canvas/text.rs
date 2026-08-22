//! Shaping for `fillText`, `strokeText` and `measureText`.
//!
//! The three are one operation with three endings, so they are one function
//! here: shape the string, then either record the glyphs or report the box.
//! Splitting them would let a measurement disagree with what was drawn, which
//! is the one thing an application uses `measureText` to rule out.
//!
//! Shaping is Parley's, over the same [`FontContext`] the document's own text
//! is laid out with — see [`crate::fonts`] for why that matters and what it
//! costs. What is *not* shared is the layout: a canvas text run is a single
//! line with no wrapping, no inline boxes and no styled ranges, so it is built
//! from scratch here rather than dug out of the document's inline layout.
//!
//! Ink extents come from the font's own glyph bounding boxes rather than from
//! rasterising, because `actualBoundingBoxAscent` is what an application
//! vertically centres text with and a wrong one is a visibly wrong layout.

use anyrender::Glyph;
use kurbo::Affine;
use parley::style::{FontFamily, FontStyle, FontWeight, FontWidth, StyleProperty};
use parley::{FontContext, LayoutContext, PositionedLayoutItem};
use peniko::FontData;
use skrifa::instance::{LocationRef, NormalizedCoord, Size};
use skrifa::metrics::GlyphMetrics;
use skrifa::{FontRef, GlyphId};

/// The font and string one text operation draws.
pub(crate) struct TextRequest<'a> {
    /// The `font-family` list, in CSS syntax.
    pub(crate) families: &'a str,
    /// Used font size in canvas pixels.
    pub(crate) size: f32,
    /// Numeric `font-weight`.
    pub(crate) weight: f32,
    /// `font-style`, as the tag the bootstrap writes.
    pub(crate) style: u8,
    /// `font-stretch` as a percentage, where 100 is normal.
    pub(crate) stretch: f32,
    /// The string, with its newlines and tabs already collapsed to spaces.
    pub(crate) text: &'a str,
}

/// One shaped run of a canvas text operation, owning what it needs to be drawn.
///
/// Owned rather than borrowed from the layout because the layout is built
/// inside a `&mut self` borrow of the shared shaping contexts, and the scene it
/// is recorded into is reached through a different borrow.
pub(crate) struct ShapedRun {
    /// The face the run resolved to.
    pub(crate) font: FontData,
    /// Used size of that face.
    pub(crate) size: f32,
    /// Variation-axis coordinates the face was instanced at.
    pub(crate) coords: Vec<i16>,
    /// The synthetic oblique a face without a real italic is drawn with.
    pub(crate) skew: Option<Affine>,
    /// Glyph identifiers and their positions, relative to the run's origin.
    pub(crate) glyphs: Vec<Glyph>,
}

/// A shaped canvas text run and the metrics `measureText` answers with.
///
/// Every distance is relative to the text's own anchor — the point
/// `fillText` was given, after `textAlign` and `textBaseline` have moved it —
/// and positive upwards, which is what a `TextMetrics` reports.
pub(crate) struct ShapedText {
    /// Runs in drawing order, positioned relative to the anchor.
    pub(crate) runs: Vec<ShapedRun>,
    /// Total advance width.
    pub(crate) width: f64,
    /// Ink extent to the left of the anchor.
    pub(crate) actual_left: f64,
    /// Ink extent to the right of the anchor.
    pub(crate) actual_right: f64,
    /// Ink extent above the alphabetic baseline.
    pub(crate) actual_ascent: f64,
    /// Ink extent below the alphabetic baseline.
    pub(crate) actual_descent: f64,
    /// Typographic ascent of the first face the run resolved to.
    pub(crate) font_ascent: f64,
    /// Typographic descent of that face.
    pub(crate) font_descent: f64,
}

/// The shaping contexts one document's canvases share.
///
/// Parley's contexts are caches rather than state: reusing them across draws is
/// what keeps a per-frame `fillText` from re-resolving its font stack and
/// re-shaping from cold every time.
pub(crate) struct TextEngine {
    fonts: FontContext,
    layouts: LayoutContext<()>,
}

impl TextEngine {
    pub(crate) fn new(fonts: FontContext) -> Self {
        Self {
            fonts,
            layouts: LayoutContext::new(),
        }
    }

    /// Shapes one text operation into runs positioned around its anchor.
    ///
    /// The anchor is the origin: a glyph's `x` is its distance from the start
    /// of the run and its `y` is its distance from the alphabetic baseline.
    /// Where `textAlign` and `textBaseline` then put that origin is the
    /// caller's transform, so the metrics reported and the glyphs recorded
    /// come out of one shaping rather than two.
    pub(crate) fn shape(&mut self, request: &TextRequest<'_>) -> ShapedText {
        let style = match request.style {
            1 => FontStyle::Italic,
            2 => FontStyle::Oblique(None),
            _ => FontStyle::Normal,
        };
        let mut builder = self
            .layouts
            .ranged_builder(&mut self.fonts, request.text, 1.0, true);
        builder.push_default(StyleProperty::FontFamily(FontFamily::Source(
            request.families.into(),
        )));
        builder.push_default(StyleProperty::FontSize(request.size));
        builder.push_default(StyleProperty::FontWeight(FontWeight::new(request.weight)));
        builder.push_default(StyleProperty::FontStyle(style));
        builder.push_default(StyleProperty::FontWidth(FontWidth::from_percentage(
            request.stretch,
        )));
        let mut layout = builder.build(request.text);
        // No maximum advance: a canvas text run is one line however long it is,
        // and `maxWidth` squeezes the drawn result rather than wrapping it.
        layout.break_all_lines(None);

        let mut shaped = ShapedText {
            runs: Vec::new(),
            width: f64::from(layout.width()),
            actual_left: 0.0,
            actual_right: 0.0,
            actual_ascent: 0.0,
            actual_descent: 0.0,
            font_ascent: 0.0,
            font_descent: 0.0,
        };
        let mut ink: Option<[f64; 4]> = None;
        let mut metrics_taken = false;
        for line in layout.lines() {
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(run) = item else {
                    continue;
                };
                let parley_run = run.run();
                if !metrics_taken {
                    let metrics = parley_run.metrics();
                    shaped.font_ascent = f64::from(metrics.ascent);
                    shaped.font_descent = f64::from(metrics.descent);
                    metrics_taken = true;
                }
                let baseline = f64::from(run.baseline());
                let font = parley_run.font().clone();
                let size = parley_run.font_size();
                let glyphs: Vec<_> = run
                    .positioned_glyphs()
                    .map(|glyph| Glyph {
                        id: glyph.id,
                        x: glyph.x,
                        // Parley positions a glyph against the layout's own
                        // origin; the anchor is the baseline, so the baseline
                        // is what comes back out.
                        y: glyph.y - baseline as f32,
                    })
                    .collect();
                accumulate_ink(
                    &font,
                    size,
                    parley_run.normalized_coords(),
                    &glyphs,
                    &mut ink,
                );
                shaped.runs.push(ShapedRun {
                    font,
                    size,
                    coords: parley_run.normalized_coords().to_vec(),
                    skew: parley_run
                        .synthesis()
                        .skew()
                        .map(|angle| Affine::skew(f64::from(angle).to_radians().tan(), 0.0)),
                    glyphs,
                });
            }
        }
        if let Some([left, top, right, bottom]) = ink {
            shaped.actual_left = -left;
            shaped.actual_right = right;
            shaped.actual_ascent = -top;
            shaped.actual_descent = bottom;
        }
        shaped
    }
}

/// Grows the ink box by one run's glyph outlines.
///
/// A glyph whose bounds the face cannot report contributes nothing rather than
/// a zero-sized box at the origin, which would drag the extents back to the
/// anchor and quietly make a centred line of text sit wrong.
fn accumulate_ink(
    font: &FontData,
    size: f32,
    coords: &[i16],
    glyphs: &[Glyph],
    ink: &mut Option<[f64; 4]>,
) {
    let Ok(face) = FontRef::from_index(font.data.as_ref(), font.index) else {
        return;
    };
    let location: Vec<NormalizedCoord> = coords
        .iter()
        .map(|coord| NormalizedCoord::from_bits(*coord))
        .collect();
    let metrics = GlyphMetrics::new(&face, Size::new(size), LocationRef::new(&location));
    for glyph in glyphs {
        let Some(bounds) = metrics.bounds(GlyphId::from(glyph.id)) else {
            continue;
        };
        if bounds.x_min == bounds.x_max || bounds.y_min == bounds.y_max {
            continue;
        }
        // Font space is y-up and canvas space is y-down, so the glyph's own
        // maximum is the smaller number here.
        let box_of = [
            f64::from(glyph.x + bounds.x_min),
            f64::from(glyph.y - bounds.y_max),
            f64::from(glyph.x + bounds.x_max),
            f64::from(glyph.y - bounds.y_min),
        ];
        *ink = Some(match *ink {
            None => box_of,
            Some([left, top, right, bottom]) => [
                left.min(box_of[0]),
                top.min(box_of[1]),
                right.max(box_of[2]),
                bottom.max(box_of[3]),
            ],
        });
    }
}
