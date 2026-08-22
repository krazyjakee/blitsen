//! Replaying a command stream into a canvas's recorded scene.
//!
//! One function, because a command stream is one pass: the operations arrive in
//! drawing order and nothing later changes what an earlier one meant. What the
//! pass is careful about is the layer stack — a submission that opened a clip
//! and did not close it would leave every later submission drawing inside it,
//! and a stray `POP_LAYER` would close a layer the DOM's own paint opened. So
//! the depth is counted, and an unbalanced stream is refused whole rather than
//! recorded halfway.

use anyrender::{Paint, PaintRef, PaintScene as _, Scene};
use kurbo::{Affine, Rect};
use peniko::{ImageBrush, ImageData, StyleRef};

use super::text::{TextEngine, TextRequest};
use super::wire::{Brush, Reader, blend_mode, fill_rule, op};
use super::{CanvasError, TextAnchor, anchor_offsets};

/// Records one submission's commands into `scene`.
///
/// `images` are the sources the stream referenced, already resolved by the
/// caller: resolving them needs the document, and the scene being written needs
/// the canvas, and the two cannot be borrowed at once.
///
/// `destructive` is set when the stream records a compose function that can
/// erase what is under it — see [`super::CanvasState::is_destructive`] for what
/// the canvas then does about it — and cleared by a `RESET`, which is the only
/// command that takes such a layer back out of the scene.
pub(crate) fn replay(
    scene: &mut Scene,
    reader: &mut Reader<'_>,
    images: &[ImageData],
    text_engine: &mut TextEngine,
    destructive: &mut bool,
) -> Result<(), CanvasError> {
    let mut depth = 0_usize;
    while !reader.is_empty() {
        match reader.tag()? {
            op::RESET => {
                scene.reset();
                *destructive = false;
            }
            op::FILL => {
                let brush = reader.paint(images)?;
                let rule = fill_rule(reader.tag()?)?;
                let transform = reader.transform()?;
                let path = reader.path()?;
                with_paint(&brush, |paint, brush_transform| {
                    scene.fill(rule, transform, paint, brush_transform, &path);
                });
            }
            op::STROKE => {
                let brush = reader.paint(images)?;
                let style = reader.stroke()?;
                let transform = reader.transform()?;
                let path = reader.path()?;
                with_paint(&brush, |paint, brush_transform| {
                    scene.stroke(&style, transform, paint, brush_transform, &path);
                });
            }
            op::PUSH_CLIP => {
                let transform = reader.transform()?;
                let path = reader.path()?;
                scene.push_clip_layer(transform, &path);
                depth += 1;
            }
            op::PUSH_LAYER => {
                let blend = blend_mode(reader.tag()?)?;
                let alpha = reader.number()? as f32;
                let transform = reader.transform()?;
                let path = reader.path()?;
                *destructive |= erases_backdrop(blend.compose);
                scene.push_layer(blend, alpha, transform, &path, None, None);
                depth += 1;
            }
            op::POP_LAYER => {
                depth = depth.checked_sub(1).ok_or(CanvasError::Unbalanced)?;
                scene.pop_layer();
            }
            op::TEXT => draw_text(scene, reader, images, text_engine)?,
            op::IMAGE => draw_image(scene, reader, images)?,
            op::PUT_IMAGE => put_image(scene, reader, images)?,
            _ => return Err(CanvasError::Malformed),
        }
    }
    if depth != 0 {
        return Err(CanvasError::Unbalanced);
    }
    Ok(())
}

/// Lends a decoded paint to a drawing call as the reference it takes.
///
/// The three paint kinds borrow differently — a colour is copied, a gradient
/// and an image are borrowed — and every call site needs all three, so the
/// match lives here once instead of at each of them.
fn with_paint<'a>(brush: &'a Brush, draw: impl FnOnce(PaintRef<'a>, Option<Affine>)) {
    match brush {
        Brush::Solid(color) => draw(Paint::Solid(*color), None),
        Brush::Gradient(gradient) => draw(Paint::Gradient(gradient.as_ref()), None),
        Brush::Image(image) => draw(
            Paint::Image(ImageBrush {
                image: &image.image,
                sampler: image.sampler,
            }),
            Some(image.transform),
        ),
    }
}

/// Records `fillText` or `strokeText`.
fn draw_text(
    scene: &mut Scene,
    reader: &mut Reader<'_>,
    images: &[ImageData],
    text_engine: &mut TextEngine,
) -> Result<(), CanvasError> {
    let brush = reader.paint(images)?;
    let stroked = reader.tag()? == 1;
    let stroke = if stroked {
        Some(reader.stroke()?)
    } else {
        None
    };
    let transform = reader.transform()?;
    let families = reader.string()?;
    let size = reader.number()? as f32;
    let weight = reader.number()? as f32;
    let style = reader.tag()?;
    let stretch = reader.number()? as f32;
    let anchor = TextAnchor {
        align: reader.tag()?,
        baseline: reader.tag()?,
        rtl: reader.tag()? == 1,
    };
    let x = reader.number()?;
    let y = reader.number()?;
    // Zero is "no limit": `maxWidth` is defined as a positive number, and a
    // browser draws nothing at all for a zero or negative one.
    let max_width = reader.number()?;
    let content = reader.string()?;

    let shaped = text_engine.shape(&TextRequest {
        families,
        size,
        weight,
        style,
        stretch,
        text: content,
    });
    if shaped.runs.is_empty() {
        return Ok(());
    }
    let (dx, dy) = anchor_offsets(&anchor, &shaped);
    let mut placement = transform * Affine::translate((x + dx, y + dy));
    // A run wider than `maxWidth` is condensed rather than clipped or wrapped,
    // which is the one of the two behaviours the specification allows that does
    // not need a second font to be chosen.
    if max_width > 0.0 && shaped.width > max_width {
        placement *= Affine::scale_non_uniform(max_width / shaped.width, 1.0);
    }
    for run in &shaped.runs {
        let style: StyleRef<'_> = match &stroke {
            Some(stroke) => StyleRef::Stroke(stroke),
            None => StyleRef::Fill(peniko::Fill::NonZero),
        };
        with_paint(&brush, |paint, _| {
            scene.draw_glyphs(
                &run.font,
                run.size,
                true,
                &run.coords,
                kurbo::Vec2::default(),
                style,
                paint,
                1.0,
                placement,
                run.skew,
                run.glyphs.iter().copied(),
            );
        });
    }
    Ok(())
}

/// Records `drawImage`, as a rectangle filled with the source it names.
///
/// The brush transform is what carries the source rectangle: it maps the
/// image's own pixels onto the destination box, so a nine-argument `drawImage`
/// and a three-argument one are the same recorded command with different
/// numbers in it.
fn draw_image(
    scene: &mut Scene,
    reader: &mut Reader<'_>,
    images: &[ImageData],
) -> Result<(), CanvasError> {
    let index = reader.count()?;
    let image = images.get(index).ok_or(CanvasError::Malformed)?;
    let quality = match reader.tag()? {
        0 => peniko::ImageQuality::Low,
        1 => peniko::ImageQuality::Medium,
        _ => peniko::ImageQuality::High,
    };
    let transform = reader.transform()?;
    let source = read_rect(reader)?;
    let destination = read_rect(reader)?;
    if source.width() == 0.0 || source.height() == 0.0 {
        return Ok(());
    }
    let brush_transform = Affine::translate((destination.x0, destination.y0))
        * Affine::scale_non_uniform(
            destination.width() / source.width(),
            destination.height() / source.height(),
        )
        * Affine::translate((-source.x0, -source.y0));
    scene.fill(
        peniko::Fill::NonZero,
        transform,
        Paint::Image(ImageBrush {
            image,
            sampler: peniko::ImageSampler {
                x_extend: peniko::Extend::Pad,
                y_extend: peniko::Extend::Pad,
                quality,
                // See `wire::Reader::paint`: `globalAlpha` on an image is a
                // layer the caller opened, never a property of the sampler.
                alpha: 1.0,
            },
        }),
        Some(brush_transform),
        &destination,
    );
    Ok(())
}

/// Records `putImageData`, which replaces pixels rather than drawing over them.
///
/// "Replaces" is two recorded operations rather than one: erase the destination
/// rectangle, then draw the supplied pixels over the hole. A single layer
/// composed with `Copy` would say it in one, and would erase the rest of the
/// canvas doing it — see [`blend_mode`]. Neither `globalAlpha` nor the current
/// transform nor the composite operation reaches this command, which is why
/// nothing here reads any of them.
fn put_image(
    scene: &mut Scene,
    reader: &mut Reader<'_>,
    images: &[ImageData],
) -> Result<(), CanvasError> {
    let index = reader.count()?;
    let image = images.get(index).ok_or(CanvasError::Malformed)?;
    let origin_x = reader.number()?;
    let origin_y = reader.number()?;
    let destination = read_rect(reader)?;
    if destination.width() <= 0.0 || destination.height() <= 0.0 {
        return Ok(());
    }
    erase(scene, Affine::IDENTITY, &destination);
    scene.fill(
        peniko::Fill::NonZero,
        Affine::IDENTITY,
        Paint::Image(ImageBrush {
            image,
            sampler: peniko::ImageSampler {
                x_extend: peniko::Extend::Pad,
                y_extend: peniko::Extend::Pad,
                // Nearest sampling, because the pixels are being placed rather
                // than drawn: `putImageData` is defined to write them through.
                quality: peniko::ImageQuality::Low,
                alpha: 1.0,
            },
        }),
        Some(Affine::translate((origin_x, origin_y))),
        &destination,
    );
    Ok(())
}

/// Records the erasure of one shape, leaving everything outside it alone.
fn erase(scene: &mut Scene, transform: Affine, shape: &impl kurbo::Shape) {
    scene.push_layer(
        peniko::BlendMode::new(peniko::Mix::Normal, peniko::Compose::DestOut),
        1.0,
        transform,
        shape,
        None,
        None,
    );
    scene.fill(
        peniko::Fill::NonZero,
        transform,
        Paint::Solid(peniko::Color::BLACK),
        None,
        shape,
    );
    scene.pop_layer();
}

/// Whether a compose function can remove what is already on the canvas.
///
/// The five that can are the ones whose result is transparent where the source
/// is absent, and a canvas carrying one has to be composited as a group so it
/// erases itself rather than the document behind it.
fn erases_backdrop(compose: peniko::Compose) -> bool {
    matches!(
        compose,
        peniko::Compose::Clear
            | peniko::Compose::Copy
            | peniko::Compose::SrcIn
            | peniko::Compose::SrcOut
            | peniko::Compose::DestIn
            | peniko::Compose::DestAtop
    )
}

fn read_rect(reader: &mut Reader<'_>) -> Result<Rect, CanvasError> {
    let x = reader.number()?;
    let y = reader.number()?;
    let width = reader.number()?;
    let height = reader.number()?;
    Ok(Rect::new(x, y, x + width, y + height))
}
