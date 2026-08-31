//! Startup reveal: the `load` dispatch and first-frame mapping of a native
//! window that is created hidden.
//!
//! Mapping a window before the renderer has a frame lets the compositor expose
//! its uninitialised contents, so the first redraw after critical resources and
//! `load` paints while the window is still hidden, then reveals it in the same
//! callback.

use blitsen_js::JsEngine;
use winit::window::WindowId;

use super::WindowApplication;

impl<Rend: anyrender::WindowRenderer, E: JsEngine + Clone> WindowApplication<Rend, E> {
    pub(crate) fn maybe_dispatch_load(&mut self) {
        if self.load_dispatched || self.has_parked_error() {
            return;
        }
        if self
            .document
            .borrow()
            .document_ref()
            .has_pending_critical_resources()
        {
            return;
        }
        match self.dispatch_window_event("load") {
            Ok(_) => {
                self.load_dispatched = true;
                for view in self.inner.windows.values() {
                    view.window.request_redraw();
                }
            }
            Err(error) => self.park_error(error),
        }
    }

    /// Makes the next redraw the startup frame, if everything it needs is ready.
    ///
    /// `blitz-shell` suppresses ordinary redraws for a view it considers
    /// invisible, so its view-side flag is raised before painting while the
    /// actual platform window remains hidden. [`Self::finish_startup_reveal`]
    /// maps it only after that paint has returned.
    pub(super) fn prepare_startup_reveal(&mut self, window_id: WindowId) -> bool {
        if self.startup_revealed
            || !self.load_dispatched
            || self.has_parked_error()
            || self.surface.is_lost()
            || self
                .document
                .borrow()
                .document_ref()
                .has_pending_critical_resources()
        {
            return false;
        }
        let Some(view) = self.inner.windows.get_mut(&window_id) else {
            return false;
        };
        if !view.renderer.is_active() {
            return false;
        }
        view.is_visible = true;
        true
    }

    /// Asks for the hidden paint once renderer and document readiness coincide.
    pub(super) fn request_startup_redraw_if_ready(&self) {
        if self.startup_revealed
            || !self.load_dispatched
            || self.has_parked_error()
            || self.surface.is_lost()
            || self
                .document
                .borrow()
                .document_ref()
                .has_pending_critical_resources()
        {
            return;
        }
        for view in self.inner.windows.values() {
            if view.renderer.is_active() {
                view.window.request_redraw();
            }
        }
    }

    /// Maps the native window after its prepared startup redraw was submitted.
    pub(super) fn finish_startup_reveal(&mut self, window_id: WindowId) {
        if self.has_parked_error()
            || self
                .document
                .borrow()
                .document_ref()
                .has_pending_critical_resources()
        {
            // A first-frame callback may have started another critical load.
            // Keep the native window hidden and let that resource's wake-up ask
            // for the eventual startup frame.
            if let Some(view) = self.inner.windows.get_mut(&window_id) {
                view.is_visible = false;
            }
            return;
        }
        let Some(view) = self.inner.windows.get(&window_id) else {
            return;
        };
        if self.reveal_on_startup {
            view.window.set_visible(true);
        }
        // Set either way: what it gates below is ordinary redraws, and a hidden
        // window that never set it would never animate or paint again.
        self.startup_revealed = true;
    }
}
