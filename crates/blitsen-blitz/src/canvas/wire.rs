//! The command stream a 2D context sends, and how it is read back.
//!
//! Every drawing operation crosses the engine boundary as a run of `f64`s in
//! one `Float64Array`, with the strings it needs held beside it. That shape is
//! the whole reason this module exists: a canvas frame is hundreds to thousands
//! of operations, and the DOM bridge's own channel — an operation name and its
//! arguments as strings, answered as JSON — costs a string conversion and a
//! parse per call. A `Float64Array` costs one copy per frame.
//!
//! Nothing here holds context state. The 2D context's state machine — the
//! transform stack, the paint styles, the current path, `save`/`restore` — is
//! JavaScript's, and every command carries the transform and paint it is drawn
//! with. So this side has no notion of a "current" anything, and a command
//! means the same thing wherever it appears in the stream.
//!
//! A submission is balanced: it opens no layer it does not close. The
//! JavaScript side unwinds its clip and compositing layers before it flushes
//! and re-opens them before the next command, which is what lets a recorded
//! scene be a sequence of independent submissions rather than one long one.

use kurbo::{Affine, BezPath, Point, Stroke};
use peniko::color::{DynamicColor, palette::css};
use peniko::{
    BlendMode, ColorStop, ColorStops, Compose, Extend, Gradient, GradientKind,
    LinearGradientPosition, Mix, RadialGradientPosition, SweepGradientPosition,
};

use super::CanvasError;

/// The operations a command stream can carry.
///
/// The numbers are the wire format and are matched in the bootstrap's own
/// table, so an opcode is never renumbered — only appended to.
pub(crate) mod op {
    /// Discards everything recorded so far.
    pub(crate) const RESET: u8 = 0;
    /// Fills a path.
    pub(crate) const FILL: u8 = 1;
    /// Strokes a path.
    pub(crate) const STROKE: u8 = 2;
    /// Opens a clip layer.
    pub(crate) const PUSH_CLIP: u8 = 3;
    /// Opens a compositing layer.
    pub(crate) const PUSH_LAYER: u8 = 4;
    /// Closes the innermost layer.
    pub(crate) const POP_LAYER: u8 = 5;
    /// Draws a run of text.
    pub(crate) const TEXT: u8 = 6;
    /// Draws part of an image.
    pub(crate) const IMAGE: u8 = 7;
    /// Replaces a rectangle of the backing store with supplied pixels.
    pub(crate) const PUT_IMAGE: u8 = 8;
}

/// A sequential reader over one command stream.
pub(crate) struct Reader<'a> {
    numbers: &'a [f64],
    strings: &'a [String],
    cursor: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(numbers: &'a [f64], strings: &'a [String]) -> Self {
        Self {
            numbers,
            strings,
            cursor: 0,
        }
    }

    /// Whether every command in the stream has been read.
    pub(crate) fn is_empty(&self) -> bool {
        self.cursor >= self.numbers.len()
    }

    /// Reads one number, refusing a stream that ends mid-command.
    pub(crate) fn number(&mut self) -> Result<f64, CanvasError> {
        let value = self
            .numbers
            .get(self.cursor)
            .copied()
            .ok_or(CanvasError::Truncated)?;
        self.cursor += 1;
        Ok(value)
    }

    /// Reads a count, which is a whole number that has to fit in memory.
    pub(crate) fn count(&mut self) -> Result<usize, CanvasError> {
        let value = self.number()?;
        if !value.is_finite() || value < 0.0 || value.fract() != 0.0 {
            return Err(CanvasError::Malformed);
        }
        Ok(value as usize)
    }

    /// Reads a small enumerated value.
    pub(crate) fn tag(&mut self) -> Result<u8, CanvasError> {
        u8::try_from(self.count()?).map_err(|_| CanvasError::Malformed)
    }

    /// Reads an index into the stream's string table.
    pub(crate) fn string(&mut self) -> Result<&'a str, CanvasError> {
        let index = self.count()?;
        self.strings
            .get(index)
            .map(String::as_str)
            .ok_or(CanvasError::Malformed)
    }

    /// Reads a 2D affine transform, in the order `DOMMatrix` writes it.
    pub(crate) fn transform(&mut self) -> Result<Affine, CanvasError> {
        let mut coefficients = [0.0; 6];
        for coefficient in &mut coefficients {
            *coefficient = self.number()?;
        }
        Ok(Affine::new(coefficients))
    }

    /// Reads a colour written as four components in the 0–1 range.
    pub(crate) fn color(&mut self) -> Result<peniko::Color, CanvasError> {
        let red = self.number()? as f32;
        let green = self.number()? as f32;
        let blue = self.number()? as f32;
        let alpha = self.number()? as f32;
        Ok(peniko::Color::new([red, green, blue, alpha]))
    }

    /// Reads a path, as the token stream `CanvasPath` records.
    pub(crate) fn path(&mut self) -> Result<BezPath, CanvasError> {
        let tokens = self.count()?;
        let end = self
            .cursor
            .checked_add(tokens)
            .filter(|end| *end <= self.numbers.len())
            .ok_or(CanvasError::Truncated)?;
        let mut path = BezPath::new();
        while self.cursor < end {
            let point = |reader: &mut Self| -> Result<Point, CanvasError> {
                Ok(Point::new(reader.number()?, reader.number()?))
            };
            match self.tag()? {
                0 => path.move_to(point(self)?),
                1 => path.line_to(point(self)?),
                2 => {
                    let control = point(self)?;
                    path.quad_to(control, point(self)?);
                }
                3 => {
                    let first = point(self)?;
                    let second = point(self)?;
                    path.curve_to(first, second, point(self)?);
                }
                4 => path.close_path(),
                _ => return Err(CanvasError::Malformed),
            }
        }
        if self.cursor != end {
            return Err(CanvasError::Malformed);
        }
        Ok(path)
    }

    /// Reads a paint, resolving an image paint against the resolved sources.
    ///
    /// A pattern's image is resolved before the stream is replayed, so this
    /// carries the index of one that was, rather than the element it came from.
    pub(crate) fn paint(&mut self, images: &[peniko::ImageData]) -> Result<Brush, CanvasError> {
        match self.tag()? {
            0 => Ok(Brush::Solid(self.color()?)),
            1 => Ok(Brush::Gradient(Box::new(self.gradient()?))),
            2 => {
                let index = self.count()?;
                let image = images.get(index).ok_or(CanvasError::Malformed)?.clone();
                let x_extend = extend(self.tag()?)?;
                let y_extend = extend(self.tag()?)?;
                let quality = quality(self.tag()?)?;
                let transform = self.transform()?;
                Ok(Brush::Image(Box::new(ImagePaint {
                    image,
                    sampler: peniko::ImageSampler {
                        x_extend,
                        y_extend,
                        quality,
                        // Never anything else: the CPU rasteriser refuses an
                        // image sampler with an opacity on it, and it is the
                        // one that answers every readback. Opacity on an image
                        // is a layer, opened by whoever is drawing.
                        alpha: 1.0,
                    },
                    transform,
                })))
            }
            _ => Err(CanvasError::Malformed),
        }
    }

    /// Reads a gradient, whose coordinates are in the user space of the draw.
    fn gradient(&mut self) -> Result<Gradient, CanvasError> {
        let kind = match self.tag()? {
            0 => GradientKind::Linear(LinearGradientPosition {
                start: Point::new(self.number()?, self.number()?),
                end: Point::new(self.number()?, self.number()?),
            }),
            1 => GradientKind::Radial(RadialGradientPosition {
                start_center: Point::new(self.number()?, self.number()?),
                start_radius: self.number()? as f32,
                end_center: Point::new(self.number()?, self.number()?),
                end_radius: self.number()? as f32,
            }),
            2 => GradientKind::Sweep(SweepGradientPosition {
                center: Point::new(self.number()?, self.number()?),
                start_angle: self.number()? as f32,
                end_angle: self.number()? as f32,
            }),
            _ => return Err(CanvasError::Malformed),
        };
        let stops = self.count()?;
        let mut ramp = ColorStops::new();
        for _ in 0..stops {
            let offset = self.number()? as f32;
            ramp.push(ColorStop {
                offset,
                color: DynamicColor::from_alpha_color(self.color()?),
            });
        }
        // A gradient with no stops paints nothing in a browser, and peniko
        // treats an empty ramp as undefined rather than as transparent.
        if ramp.is_empty() {
            ramp.push(ColorStop {
                offset: 0.0,
                color: DynamicColor::from_alpha_color(css::TRANSPARENT),
            });
        }
        Ok(Gradient {
            kind,
            extend: Extend::Pad,
            stops: ramp,
            ..Default::default()
        })
    }

    /// Reads a stroke style, including its dash pattern.
    pub(crate) fn stroke(&mut self) -> Result<Stroke, CanvasError> {
        let width = self.number()?;
        let cap = match self.tag()? {
            0 => kurbo::Cap::Butt,
            1 => kurbo::Cap::Round,
            2 => kurbo::Cap::Square,
            _ => return Err(CanvasError::Malformed),
        };
        let join = match self.tag()? {
            0 => kurbo::Join::Miter,
            1 => kurbo::Join::Round,
            2 => kurbo::Join::Bevel,
            _ => return Err(CanvasError::Malformed),
        };
        let miter_limit = self.number()?;
        let dash_offset = self.number()?;
        let dashes = self.count()?;
        let mut pattern = Vec::with_capacity(dashes);
        for _ in 0..dashes {
            pattern.push(self.number()?);
        }
        let stroke = Stroke::new(width)
            .with_caps(cap)
            .with_join(join)
            .with_miter_limit(miter_limit);
        Ok(if pattern.is_empty() {
            stroke
        } else {
            stroke.with_dashes(dash_offset, pattern)
        })
    }
}

/// A paint resolved out of the stream, owning whatever it referenced.
pub(crate) enum Brush {
    /// One colour.
    Solid(peniko::Color),
    /// A gradient in the user space of the draw it paints.
    Gradient(Box<Gradient>),
    /// An image, with the transform that maps it into user space.
    Image(Box<ImagePaint>),
}

/// An image paint and where it sits, which is what `createPattern` produces.
pub(crate) struct ImagePaint {
    /// Pixels being sampled.
    pub(crate) image: peniko::ImageData,
    /// How the sample is taken outside the image and between its pixels.
    pub(crate) sampler: peniko::ImageSampler,
    /// Maps image space into the user space of the draw.
    pub(crate) transform: Affine,
}

fn extend(tag: u8) -> Result<Extend, CanvasError> {
    match tag {
        0 => Ok(Extend::Pad),
        1 => Ok(Extend::Repeat),
        2 => Ok(Extend::Reflect),
        _ => Err(CanvasError::Malformed),
    }
}

fn quality(tag: u8) -> Result<peniko::ImageQuality, CanvasError> {
    match tag {
        0 => Ok(peniko::ImageQuality::Low),
        1 => Ok(peniko::ImageQuality::Medium),
        2 => Ok(peniko::ImageQuality::High),
        _ => Err(CanvasError::Malformed),
    }
}

/// The blend mode one `globalCompositeOperation` names.
///
/// The numbering is the order the bootstrap lists the operation names in, so
/// the two tables are one fact written twice and a mismatch is a test failure
/// rather than a wrong picture. Every operation the specification defines is
/// here — all 27 of them are a `peniko` mix or compose mode — which is why the
/// whole set is expressible rather than the handful `source-over` and friends
/// would cover.
///
/// `destination-out` is also what `clearRect` and `putImageData` reach for, and
/// deliberately: the renderer applies a compose function to the whole surface
/// rather than only inside the layer's clip, so an operation whose result for
/// an absent source is the destination unchanged — `destination-out` is, and
/// `clear` and `copy` are not — is the only kind that can erase a rectangle
/// without erasing the canvas.
pub(crate) fn blend_mode(tag: u8) -> Result<BlendMode, CanvasError> {
    let mode = match tag {
        0 => BlendMode::new(Mix::Normal, Compose::SrcOver),
        1 => BlendMode::new(Mix::Normal, Compose::SrcIn),
        2 => BlendMode::new(Mix::Normal, Compose::SrcOut),
        3 => BlendMode::new(Mix::Normal, Compose::SrcAtop),
        4 => BlendMode::new(Mix::Normal, Compose::DestOver),
        5 => BlendMode::new(Mix::Normal, Compose::DestIn),
        6 => BlendMode::new(Mix::Normal, Compose::DestOut),
        7 => BlendMode::new(Mix::Normal, Compose::DestAtop),
        8 => BlendMode::new(Mix::Normal, Compose::Plus),
        9 => BlendMode::new(Mix::Normal, Compose::Copy),
        10 => BlendMode::new(Mix::Normal, Compose::Xor),
        11 => BlendMode::new(Mix::Multiply, Compose::SrcOver),
        12 => BlendMode::new(Mix::Screen, Compose::SrcOver),
        13 => BlendMode::new(Mix::Overlay, Compose::SrcOver),
        14 => BlendMode::new(Mix::Darken, Compose::SrcOver),
        15 => BlendMode::new(Mix::Lighten, Compose::SrcOver),
        16 => BlendMode::new(Mix::ColorDodge, Compose::SrcOver),
        17 => BlendMode::new(Mix::ColorBurn, Compose::SrcOver),
        18 => BlendMode::new(Mix::HardLight, Compose::SrcOver),
        19 => BlendMode::new(Mix::SoftLight, Compose::SrcOver),
        20 => BlendMode::new(Mix::Difference, Compose::SrcOver),
        21 => BlendMode::new(Mix::Exclusion, Compose::SrcOver),
        22 => BlendMode::new(Mix::Hue, Compose::SrcOver),
        23 => BlendMode::new(Mix::Saturation, Compose::SrcOver),
        24 => BlendMode::new(Mix::Color, Compose::SrcOver),
        25 => BlendMode::new(Mix::Luminosity, Compose::SrcOver),
        26 => BlendMode::new(Mix::Normal, Compose::PlusLighter),
        _ => return Err(CanvasError::Malformed),
    };
    Ok(mode)
}

/// The fill rule a tag names.
pub(crate) fn fill_rule(tag: u8) -> Result<peniko::Fill, CanvasError> {
    match tag {
        0 => Ok(peniko::Fill::NonZero),
        1 => Ok(peniko::Fill::EvenOdd),
        _ => Err(CanvasError::Malformed),
    }
}
