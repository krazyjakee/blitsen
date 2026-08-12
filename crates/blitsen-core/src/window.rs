//! The window metrics a document can observe.

use blitsen_js::{JsEngine, JsError};

/// Viewport-backed properties exposed on the JavaScript `window` object.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowState {
    width: u32,
    height: u32,
    device_pixel_ratio: f64,
}

impl WindowState {
    /// Creates viewport state in logical CSS pixels.
    pub fn new(width: u32, height: u32, device_pixel_ratio: f64) -> Self {
        Self {
            width,
            height,
            device_pixel_ratio,
        }
    }

    /// Updates logical dimensions after a native resize event.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
    }

    /// Installs `window` as the global object and attaches the document.
    pub fn install<J: JsEngine>(
        self,
        engine: &mut J,
        document: &J::Value,
    ) -> Result<J::Value, JsError> {
        let window = engine.evaluate_script(
            "for (const key of ['location','history','navigator','localStorage','sessionStorage']) { try { delete globalThis[key] } catch {} } globalThis",
            "blitsen:window-bootstrap",
        )?;
        engine.set_global("window", &window)?;
        engine.set_property(&window, "document", document)?;
        self.sync(engine, &window)?;
        Ok(window)
    }

    /// Synchronizes viewport properties after state changes.
    pub fn sync<J: JsEngine>(self, engine: &mut J, window: &J::Value) -> Result<(), JsError> {
        let width = engine.number(f64::from(self.width));
        let height = engine.number(f64::from(self.height));
        let ratio = engine.number(self.device_pixel_ratio);
        engine.set_property(window, "innerWidth", &width)?;
        engine.set_property(window, "innerHeight", &height)?;
        engine.set_property(window, "devicePixelRatio", &ratio)
    }

    /// Returns logical viewport width.
    pub fn width(self) -> u32 {
        self.width
    }

    /// Returns logical viewport height.
    pub fn height(self) -> u32 {
        self.height
    }

    /// Returns native pixels per CSS pixel.
    pub fn device_pixel_ratio(self) -> f64 {
        self.device_pixel_ratio
    }
}
