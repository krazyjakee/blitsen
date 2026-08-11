//! The surface behind one `<blitsen-view>` element.
//!
//! Blitz owns the element's box; this module owns the pixels inside it. The
//! surface is recorded through Blitz's custom-widget seam, which appends it to
//! the same scene as the DOM at the element's content-box origin. Z-order
//! interleaving, ancestor `overflow` and border-radius clipping therefore come
//! from the element's own paint position rather than from a second pass over
//! the frame, and the renderer presents one composited surface per frame.
//!
//! Contents travel as a `peniko` image whose blob identity changes exactly when
//! the application writes a frame, so Vello uploads a written frame once and
//! re-uses its atlas entry for frames the application leaves alone.
//!
//! A wgpu texture registered through [`anyrender::RenderContext`] was the other
//! candidate transport, and it is not viable at the pinned Vello: registered
//! textures are copied into Vello's image atlas rather than sampled in place —
//! an extra full-frame GPU copy — and the atlas entry is only refreshed by
//! `vello::Renderer::mark_override_image_dirty`, which AnyRender's
//! `RenderContext` does not expose. Re-using one texture across frames would
//! silently composite stale pixels.

use std::cell::RefCell;
use std::rc::Rc;

use anyrender::{Paint, PaintScene as _, RenderContext, Scene};
use blitsen_dom::{DomError, NATIVE_VIEWPORT_BYTES_PER_PIXEL};
use blitz::dom::Widget;
use blitz::dom::node::ComputedStyles;
use kurbo::{Affine, Rect};
use peniko::{
    Blob, Extend, Fill, ImageAlphaType, ImageBrush, ImageData, ImageFormat, ImageQuality,
    ImageSampler,
};

/// User-agent rules that give `<blitsen-view>` a replaced element's box.
///
/// The default object size matches `<canvas>`. `overflow: hidden` is what makes
/// the element clip its own composited contents to its padding box, so a
/// `border-radius` on the element rounds the surface too, and descendants are
/// suppressed because a replaced element's box is drawn by its content, not by
/// its children.
pub(crate) const NATIVE_VIEWPORT_UA_CSS: &str = "\
blitsen-view { display: block; width: 300px; height: 150px; overflow: hidden }
blitsen-view > * { display: none }
";

/// Surface parameters and contents shared with the JavaScript handle.
#[derive(Debug, Default)]
pub(crate) struct ViewportState {
    width: u32,
    height: u32,
    device_pixel_ratio: f64,
    generation: u64,
    revision: u64,
    contents: Option<ImageData>,
}

impl ViewportState {
    /// Adopts a layout-derived surface size, reporting whether it changed.
    ///
    /// Contents drawn for the previous size are dropped rather than stretched:
    /// a resized surface has no correct old frame, and keeping one would show
    /// the application a buffer whose length no longer matches its own idea of
    /// the surface.
    pub(crate) fn resize(&mut self, width: u32, height: u32, device_pixel_ratio: f64) -> bool {
        if self.width == width
            && self.height == height
            && self.device_pixel_ratio == device_pixel_ratio
        {
            return false;
        }
        self.width = width;
        self.height = height;
        self.device_pixel_ratio = device_pixel_ratio;
        self.generation += 1;
        self.revision += 1;
        self.contents = None;
        true
    }

    /// Reports the current surface size in physical pixels.
    pub(crate) fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Reports the physical pixels per CSS pixel of the current surface.
    pub(crate) fn device_pixel_ratio(&self) -> f64 {
        self.device_pixel_ratio
    }

    /// Reports how many times the surface size or density has changed.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Replaces the surface contents with one complete RGBA frame.
    pub(crate) fn write(&mut self, pixels: &[u8]) -> Result<(), DomError> {
        let expected = self.width as usize * self.height as usize * NATIVE_VIEWPORT_BYTES_PER_PIXEL;
        if pixels.len() != expected {
            return Err(DomError::Backend(format!(
                "<blitsen-view> surface needs {expected} RGBA bytes, received {}",
                pixels.len()
            )));
        }
        self.contents = Some(ImageData {
            data: Blob::from(pixels.to_vec()),
            format: ImageFormat::Rgba8,
            // Straight alpha, matching what Vello assumes of an application's
            // own texture, so one written frame reads the same either way.
            alpha_type: ImageAlphaType::Alpha,
            width: self.width,
            height: self.height,
        });
        self.revision += 1;
        Ok(())
    }
}

/// Paints one `<blitsen-view>` element's surface into the document's scene.
pub(crate) struct ViewportWidget {
    state: Rc<RefCell<ViewportState>>,
    /// Revision of the contents last recorded into a scene.
    painted_revision: u64,
}

impl ViewportWidget {
    pub(crate) fn new(state: Rc<RefCell<ViewportState>>) -> Self {
        Self {
            state,
            painted_revision: 0,
        }
    }
}

impl Widget for ViewportWidget {
    fn requires_redraw(&self) -> bool {
        self.state.borrow().revision != self.painted_revision
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
        let state = self.state.borrow();
        self.painted_revision = state.revision;

        // A viewport the application has not drawn at this size stays empty
        // rather than showing a frame it did not produce.
        let Some(image) = state
            .contents
            .as_ref()
            .filter(|image| image.width == width && image.height == height)
        else {
            return scene;
        };
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            Paint::Image(ImageBrush {
                image,
                sampler: ImageSampler {
                    x_extend: Extend::Pad,
                    y_extend: Extend::Pad,
                    quality: ImageQuality::Low,
                    alpha: 1.0,
                },
            }),
            None,
            &Rect::new(0.0, 0.0, f64::from(width), f64::from(height)),
        );
        scene
    }
}
