//! Turning a recorded canvas back into pixels.
//!
//! This is the one place a canvas costs a rasterisation. Painting does not:
//! the widget hands the compositor a command list that the live backend
//! replays into the frame it was already drawing (see [`super`]). Only the
//! calls the specification defines as reading pixels back — `getImageData`,
//! `toDataURL`, `toBlob`, and using one canvas as another's image source —
//! have to produce a buffer, and they say so in their names.
//!
//! Rasterisation is `vello_cpu` rather than the window's GPU renderer, and
//! deliberately: reading a texture back off the GPU stalls the pipeline for a
//! frame or more, and the answer has to be in hand before the call returns.
//! The same code therefore runs in a window, headless and in the tests.

use anyrender::{PaintScene as _, Scene, render_to_buffer};
use anyrender_vello_cpu::VelloCpuImageRenderer;
use kurbo::Affine;
use peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};

use super::CanvasError;

/// The image formats `toDataURL` and `toBlob` can encode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CanvasImageFormat {
    /// `image/png`, the format a canvas encodes when asked for nothing else.
    Png,
    /// `image/jpeg`, which has no alpha channel — see [`encode`].
    Jpeg,
}

impl CanvasImageFormat {
    /// The MIME type this format is named by, as a data URL spells it.
    pub const fn mime_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
        }
    }

    /// Reads a MIME type, falling back to PNG as `toDataURL` is defined to.
    pub fn from_mime_type(mime: &str) -> Self {
        if mime.eq_ignore_ascii_case("image/jpeg") {
            Self::Jpeg
        } else {
            Self::Png
        }
    }
}

/// Rasterises a region of a recorded scene into straight-alpha RGBA8 rows.
///
/// The region is in canvas pixels and may sit partly or wholly outside the
/// backing store, which `getImageData` explicitly allows: the scene simply
/// paints nothing there, so those pixels come back transparent black rather
/// than as an error.
///
/// The renderer composites in premultiplied alpha and hands back its buffer in
/// that form. Every consumer of this function wants straight alpha —
/// `getImageData` is defined to return it, PNG stores it, and a canvas used as
/// another's image source is declared as it — so the division happens once,
/// here, rather than at three call sites that could each forget.
pub(crate) fn rasterize(scene: &Scene, x: f64, y: f64, width: u32, height: u32) -> Vec<u8> {
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let mut pixels = render_to_buffer::<VelloCpuImageRenderer, _>(
        |target| target.append_scene(scene.clone(), Affine::translate((-x, -y))),
        width,
        height,
    );
    for pixel in pixels.as_chunks_mut::<4>().0 {
        let alpha = u32::from(pixel[3]);
        if alpha == 0 || alpha == 255 {
            continue;
        }
        for channel in &mut pixel[..3] {
            *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
        }
    }
    pixels
}

/// Rasterises a whole canvas as an image another canvas can sample.
pub(crate) fn to_image(scene: &Scene, width: u32, height: u32) -> ImageData {
    ImageData {
        data: Blob::from(rasterize(scene, 0.0, 0.0, width, height)),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width,
        height,
    }
}

/// Encodes straight-alpha RGBA8 rows as a complete image file.
///
/// JPEG has no alpha channel, and a canvas encoded as one is composited over
/// black first — which is what a browser does, and the reason a transparent
/// canvas saved as a JPEG comes back opaque black rather than white.
pub(crate) fn encode(
    pixels: &[u8],
    width: u32,
    height: u32,
    format: CanvasImageFormat,
    quality: f64,
) -> Result<Vec<u8>, CanvasError> {
    use image::{ExtendedColorType, ImageEncoder as _};

    let mut encoded = Vec::new();
    match format {
        CanvasImageFormat::Png => image::codecs::png::PngEncoder::new(&mut encoded)
            .write_image(pixels, width, height, ExtendedColorType::Rgba8)
            .map_err(|error| CanvasError::Encode(error.to_string()))?,
        CanvasImageFormat::Jpeg => {
            let opaque: Vec<u8> = pixels
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|pixel| {
                    let alpha = f32::from(pixel[3]) / 255.0;
                    [
                        (f32::from(pixel[0]) * alpha).round() as u8,
                        (f32::from(pixel[1]) * alpha).round() as u8,
                        (f32::from(pixel[2]) * alpha).round() as u8,
                    ]
                })
                .collect();
            // The specification's range is 0–1 and anything outside it, or not
            // a number at all, means "the encoder's own default".
            let quality = if (0.0..=1.0).contains(&quality) {
                (quality * 100.0).round().clamp(1.0, 100.0) as u8
            } else {
                92
            };
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, quality)
                .write_image(&opaque, width, height, ExtendedColorType::Rgb8)
                .map_err(|error| CanvasError::Encode(error.to_string()))?;
        }
    }
    Ok(encoded)
}

/// The alphabet and padding of standard base64, which a data URL uses.
const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Writes bytes as a `data:` URL's base64 payload.
///
/// Twenty lines rather than a dependency: this is the only base64 encoder in
/// the runtime, standard alphabet and padded, and a crate for it would be a
/// supply-chain entry and a compile unit for something with no variations left
/// to get wrong.
pub(crate) fn base64(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let mut group = [0_u8; 3];
        group[..chunk.len()].copy_from_slice(chunk);
        let bits = u32::from_be_bytes([0, group[0], group[1], group[2]]);
        for index in 0..4 {
            if index <= chunk.len() {
                encoded.push(BASE64[(bits >> (18 - index * 6) & 0x3f) as usize] as char);
            } else {
                encoded.push('=');
            }
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::base64;

    #[test]
    fn base64_pads_every_remainder() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xff, 0xff, 0xff]), "////");
    }
}
