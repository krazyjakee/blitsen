//! Window-state synchronisation: keeping the JavaScript `window` snapshot, the
//! native cursor and the applied surface size in step with the native window.

use std::sync::Arc;

use blitsen_core::WindowState;
use blitsen_dom::DomBackend;
use blitsen_js::{JsEngine, JsError};
use winit::application::ApplicationHandler;
use winit::cursor::CursorIcon;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::WindowApplication;
use super::borderless_resize::borderless_resize_direction;

impl<Rend: anyrender::WindowRenderer, E: JsEngine + Clone> WindowApplication<Rend, E> {
    fn sync_window(&self, width: u32, height: u32, device_pixel_ratio: f64) {
        if self.has_parked_error() {
            return;
        }
        *self.state.borrow_mut() = WindowState::new(width, height, device_pixel_ratio);
        let result = (|| {
            let mut engine = self.engine.clone();
            let window = engine.retained_value(&self.host_hooks.window)?;
            self.state.borrow().sync(&mut engine, &window)
        })();
        if let Err(error) = result {
            self.park_error(error);
        }
    }

    /// Puts the cursor the document resolves under the pointer on the window.
    ///
    /// Blitz sets a cursor of its own from its hover state, and its hover hit
    /// test cannot reach an element laid out past its parent's box, so this runs
    /// after the frame Blitz painted and has the last word (issue #128).
    ///
    /// Resolved once per frame rather than once per pointer move: a cursor is
    /// also changed by a class or an inline style landing under a pointer that
    /// never moved, and a frame is the moment both are settled. The pointer
    /// position and the tree revision together say whether the answer could have
    /// changed at all, which keeps a still pointer over a still document from
    /// paying for a hit test every frame.
    pub(super) fn sync_cursor(&mut self, window_id: WindowId) {
        if self.has_parked_error() {
            return;
        }
        let Some(&(physical_x, physical_y)) = self.pointer_positions.get(&window_id) else {
            return;
        };
        let Some((scale, resize_direction)) = self.inner.windows.get(&window_id).map(|view| {
            (
                f64::from(view.doc.inner().viewport().hidpi_scale),
                borderless_resize_direction(view.window.as_ref(), physical_x, physical_y),
            )
        }) else {
            return;
        };
        // The runtime owns the otherwise-missing frame of an undecorated
        // window. Its resize cursor therefore takes precedence over CSS in the
        // narrow edge hit area, just as an operating-system decoration does.
        if let Some(direction) = resize_direction {
            self.cursor_resolved_from.remove(&window_id);
            let cursor_icon = CursorIcon::from(direction);
            let icon = Some(cursor_icon);
            if self.applied_cursor.get(&window_id) != Some(&icon) {
                self.applied_cursor.insert(window_id, icon);
                if let Some(view) = self.inner.windows.get(&window_id) {
                    view.window.set_cursor(cursor_icon.into());
                    view.window.set_cursor_visible(true);
                }
            }
            return;
        }
        let client_x = physical_x / scale;
        let client_y = physical_y / scale;
        let revision = match self.document.borrow_mut().flush_layout() {
            Ok(snapshot) => snapshot.revision(),
            Err(error) => {
                self.park_error(crate::dom_error(error));
                return;
            }
        };
        let source = (client_x.to_bits(), client_y.to_bits(), revision);
        if self.cursor_resolved_from.get(&window_id) == Some(&source) {
            return;
        }
        self.cursor_resolved_from.insert(window_id, source);
        let icon = match self
            .document
            .borrow()
            .cursor_at(client_x as f32, client_y as f32)
        {
            Ok(icon) => icon,
            Err(error) => {
                self.park_error(crate::dom_error(error));
                return;
            }
        };
        if self.applied_cursor.get(&window_id) == Some(&icon) {
            return;
        }
        self.applied_cursor.insert(window_id, icon);
        let Some(view) = self.inner.windows.get(&window_id) else {
            return;
        };
        // `cursor: none` is a hidden pointer rather than an arrow, which is the
        // one value winit spells with a second call.
        match icon {
            Some(icon) => {
                view.window.set_cursor(icon.into());
                view.window.set_cursor_visible(true);
            }
            None => view.window.set_cursor_visible(false),
        }
    }

    pub(crate) fn sync_native_window(&mut self, window_id: WindowId) {
        if let Some(scale) = self.system_scale_override
            && let Some(view) = self.inner.windows.get_mut(&window_id)
            && f64::from(view.doc.inner().viewport().hidpi_scale) < scale
        {
            view.doc
                .inner_mut()
                .viewport_mut()
                .set_hidpi_scale(scale as f32);
        }
        let Some((width, height, scale)) = self.inner.windows.get(&window_id).map(|view| {
            let document = view.doc.inner();
            let viewport = document.viewport();
            let logical =
                winit::dpi::PhysicalSize::new(viewport.window_size.0, viewport.window_size.1)
                    .to_logical::<u32>(f64::from(viewport.hidpi_scale));
            (logical.width, logical.height, viewport.hidpi_scale)
        }) else {
            return;
        };
        self.sync_window(width, height, f64::from(scale));
    }

    pub(super) fn dispatch_window_event(&self, event_type: &str) -> Result<bool, JsError> {
        if let Some(error) = self.parked_error() {
            return Err(error);
        }
        let mut engine = self.engine.clone();
        let event_type = engine.string(event_type)?;
        let hook = engine.retained_value(&self.host_hooks.lifecycle)?;
        let result = engine.call(&hook, None, &[event_type])?;
        engine.to_boolean(&result)
    }

    /// Hands `blitsen/window` the window it acts on, or takes it away.
    ///
    /// There is deliberately only one in the current host. Issue #105 chooses
    /// isolated JavaScript contexts for future windows, so `create` remains
    /// absent until this session can publish the window belonging to the
    /// calling context instead of one process-wide slot.
    pub(crate) fn publish_window(&self) {
        crate::dom_bridge::window::publish(
            self.inner
                .windows
                .values()
                .next()
                .map(|view| Arc::clone(&view.window)),
        );
    }

    /// Reports a resize winit applied outright, which raises no event of its own.
    ///
    /// Wayland resizes the surface there and then; X11 asks the server and the
    /// answer arrives as `SurfaceResized`. Feeding the Wayland answer through
    /// the same event keeps one path to the viewport, `innerWidth` and the
    /// `resize` event rather than a second one that could disagree.
    pub(super) fn settle_native_resize(&mut self, event_loop: &dyn ActiveEventLoop) {
        let Some(size) = crate::dom_bridge::window::take_applied_resize() else {
            return;
        };
        let Some(&window_id) = self.inner.windows.keys().next() else {
            return;
        };
        self.window_event(event_loop, window_id, WindowEvent::SurfaceResized(size));
    }

    /// Applies the size the window arrived at, if it is not the one it is at.
    ///
    /// The whole cost of a resize is here — the swapchain reconfigure, the
    /// relayout the new viewport forces, and the `resize` listeners an
    /// application registered — so a size that changes nothing is dropped
    /// rather than paid for. Winit reports the size again on every configure a
    /// window manager sends, including the ones that only moved it.
    pub(super) fn apply_pending_resize(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
        window_id: WindowId,
    ) {
        // Leave the resize queued until `pump` has surfaced the earlier error.
        if self.has_parked_error() {
            return;
        }
        let Some(size) = self.pending_resize.remove(&window_id) else {
            return;
        };
        if self.applied_resize.get(&window_id) == Some(&size) {
            return;
        }
        self.applied_resize.insert(window_id, size);
        self.inner
            .window_event(event_loop, window_id, WindowEvent::SurfaceResized(size));
        self.sync_native_window(window_id);
        if !self.has_parked_error()
            && let Err(error) = self.dispatch_window_event("resize")
        {
            self.park_error(error);
        }
        if let Some(view) = self.inner.windows.get(&window_id) {
            view.window.request_redraw();
        }
    }
}
