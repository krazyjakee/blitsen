//! Native surface loss and recreation.
//!
//! Desktop backends normally create one surface for the process lifetime;
//! Android can destroy it while retaining the activity, window handle, DOM,
//! JavaScript heap, timers, and CPU-backed canvas/native-view state. Only the
//! renderer is rebuilt when the surface returns.
//!
//! Losing a surface cancels live pointer contacts and clears modifier state
//! because the platform will not send their terminal events while unfocused.
//! The outer runtime also consults [`SurfaceState`] so it does not keep polling
//! animation frames that cannot be presented. Timers retain their normal
//! schedule; background throttling would require a visibility API Blitsen does
//! not yet expose.
//!
//! A memory warning collects the JavaScript heap without discarding visible
//! application state. Android configuration changes are handled as viewport
//! changes rather than activity restarts, preserving the same state across
//! rotation. The synthetic phases below exercise the real winit handlers in
//! `tests/surface_lifecycle.rs`.

use std::sync::Arc;

use blitsen_js::JsEngine;
use winit::application::ApplicationHandler as _;
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::ModifiersState;

use crate::WindowSession;
use crate::native_window::WindowApplication;
use crate::pointer_input::{PendingPointerInput, PointerDetails};

/// Whether the window this session paints into currently has a surface.
///
/// `Initial` is not the same as `Lost`, and the difference is load-bearing: an
/// outer loop that treats "no surface yet" as "suspended" would block before
/// the first `can_create_surfaces` and never open the window at all.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SurfaceState {
    /// No surface has been created yet. The first pump is what creates it.
    Initial,
    /// A surface exists and can be painted into.
    Present,
    /// A surface existed and was taken away. Nothing will paint until it is back.
    Lost,
}

impl SurfaceState {
    /// Whether the frame loop should stop asking for frames.
    ///
    /// True only for a surface that was here and went; see the module comment
    /// on why [`Initial`](Self::Initial) must not answer the same way.
    pub fn is_lost(self) -> bool {
        matches!(self, Self::Lost)
    }
}

/// A synthetic surface cycle, queued by a test and run at the next pump.
///
/// The handlers need an `&dyn ActiveEventLoop`, which only winit can hand out,
/// so a caller outside the loop cannot invoke them directly. This is queued
/// instead and executed from `about_to_wait`, where a real one is in scope.
///
/// The order within a phase follows Android's Activity lifecycle — `onStop`
/// before the window is taken away, `onStart` before it is given back. That is
/// Android's documented ordering rather than something read out of winit's
/// source, which is why both handlers are idempotent and neither depends on the
/// other having run first.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SyntheticPhase {
    /// `suspended` then `destroy_surfaces`.
    Lose,
    /// `resumed` then `can_create_surfaces`.
    Restore,
}

/// The session-level view of the surface, and the seam a test drives it from.
impl<E: JsEngine + Clone + 'static> WindowSession<E> {
    /// Whether the window still has a surface to paint into.
    ///
    /// The frame loop reads this: a surface that has gone away takes
    /// `requestAnimationFrame` with it, and the loop stops asking for frames
    /// nothing can present. See `surface_lifecycle` for the whole argument.
    pub fn surface(&self) -> SurfaceState {
        self.application.surface
    }

    /// Drives a surface loss the platform did not send, for a test.
    ///
    /// Not a public API and not something an application can reach: it exists
    /// because the lifecycle this models is Android's, and a CI-built APK is
    /// not evidence that these handlers ran on a device. The phase is queued
    /// rather than run, because the handlers need
    /// an `ActiveEventLoop` that only a pump can produce — so the effect lands
    /// on the *next* [`pump`](Self::pump), not on this call.
    pub fn lose_surface(&mut self) {
        self.application.synthetic_phase = Some(SyntheticPhase::Lose);
    }

    /// Drives the surface coming back. Counterpart to [`Self::lose_surface`].
    pub fn restore_surface(&mut self) {
        self.application.synthetic_phase = Some(SyntheticPhase::Restore);
    }

    /// How many references the window handle has out.
    ///
    /// The leak assertion for a surface cycle, and the reason it is a count
    /// rather than a byte total: every wgpu surface built on this window holds
    /// a clone of the handle, so a cycle that dropped the old surface and a
    /// cycle that orphaned it differ here by exactly one, every time, with no
    /// allocator noise to see through.
    pub fn window_references(&self) -> usize {
        self.application
            .inner
            .windows
            .values()
            .next()
            .map_or(0, |view| Arc::strong_count(&view.window))
    }

    /// Whether the renderer currently holds a live wgpu surface.
    ///
    /// Read straight off the renderer rather than off Blitsen's own bookkeeping,
    /// so a test asserting a cycle happened is asking the thing that owns the
    /// GPU resources, not the flag that is supposed to agree with it.
    pub fn renderer_is_active(&self) -> bool {
        use anyrender::WindowRenderer as _;
        self.application
            .inner
            .windows
            .values()
            .next()
            .is_some_and(|view| view.renderer.is_active())
    }
}

impl<Rend: anyrender::WindowRenderer, E: JsEngine + Clone> WindowApplication<Rend, E> {
    /// The surface has arrived: publish the window and let the document see it.
    pub(crate) fn on_can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.inner.can_create_surfaces(event_loop);
        self.surface = SurfaceState::Present;
        // Published before `load` is dispatched, so the first listener an
        // application registers already has a window to act on.
        self.publish_window();
        let windows: Vec<_> = self.inner.windows.keys().copied().collect();
        for id in windows {
            self.sync_native_window(id);
        }
        // A recreated surface has nothing in it. The first frame after a cycle
        // has to be asked for, because the document has not changed and so
        // nothing else in the loop will ask.
        for view in self.inner.windows.values() {
            view.window.request_redraw();
        }
        self.maybe_dispatch_load();
    }

    /// The surface is going away: end the gestures that were riding on it.
    pub(crate) fn on_destroy_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        // Before the renderer goes, because cancelling dispatches into
        // JavaScript and hit-tests the document, and both want the viewport the
        // contacts were made against.
        let window_id = self.inner.windows.keys().next().copied();
        if let Some(window_id) = window_id {
            self.release_web_window_modes(window_id, "surface-loss");
            self.drain_keyboard_input(window_id);
        }
        self.cancel_live_contacts();
        self.inner.destroy_surfaces(event_loop);
        self.surface = SurfaceState::Lost;
    }

    /// The application has been stopped, which may or may not precede the above.
    ///
    /// Android sends `Stop` and `TerminateWindow` in either order depending on
    /// why the app is leaving, so the input reset is here as well and both are
    /// written to be idempotent.
    pub(crate) fn on_suspended(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.inner.suspended(event_loop);
        let window_id = self.inner.windows.keys().next().copied();
        if let Some(window_id) = window_id {
            self.release_web_window_modes(window_id, "suspend");
            self.drain_keyboard_input(window_id);
        }
        self.cancel_live_contacts();
        self.modifiers = ModifiersState::empty();
    }

    /// The application has been started again.
    ///
    /// Deliberately empty of recovery work: on Android this arrives *before*
    /// the surface does, so anything that needs a surface would be acting on
    /// one that is not there. Everything a restart needs is in
    /// [`on_can_create_surfaces`](Self::on_can_create_surfaces).
    pub(crate) fn on_resumed(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.inner.resumed(event_loop);
    }

    /// The system is short of memory: collect the JavaScript heap early.
    ///
    /// See the module comment for why this and nothing else.
    pub(crate) fn on_memory_warning(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.inner.memory_warning(event_loop);
        if self.has_parked_error() {
            return;
        }
        if let Err(error) = self.engine.clone().collect_garbage() {
            self.park_error(error);
        }
    }

    /// Tells the DOM that every contact still down has been taken away.
    ///
    /// Queued behind whatever input was already waiting, so a `pointerdown`
    /// that has not been dispatched yet is still delivered before the
    /// `pointercancel` that ends it — a cancel for a pointer JavaScript never
    /// saw begin would be worse than the leak it is fixing.
    fn cancel_live_contacts(&mut self) {
        let contacts = self.pointer_ids.live();
        if contacts.is_empty() {
            return;
        }
        self.pointer_ids.clear();
        let Some(&window_id) = self.inner.windows.keys().next() else {
            return;
        };
        let at = self
            .pointer_positions
            .get(&window_id)
            .copied()
            .unwrap_or_default();
        self.pending_pointer_input
            .extend(cancellations(contacts, at).map(|input| (window_id, input)));
        self.drain_pointer_input(window_id);
    }

    /// Runs a queued synthetic cycle phase, if a test asked for one.
    pub(crate) fn run_synthetic_phase(&mut self, event_loop: &dyn ActiveEventLoop) {
        let Some(phase) = self.synthetic_phase.take() else {
            return;
        };
        match phase {
            SyntheticPhase::Lose => {
                self.on_suspended(event_loop);
                self.on_destroy_surfaces(event_loop);
            }
            SyntheticPhase::Restore => {
                self.on_resumed(event_loop);
                self.on_can_create_surfaces(event_loop);
            }
        }
    }
}

/// One `pointercancel` per live contact, at the last position seen.
///
/// A free function rather than part of the handler so that what a surface loss
/// *says* to the DOM can be asserted without a window, a GPU or a JavaScript
/// engine — which matters because the desktop has no touch contact to leave
/// open, so the windowed test in `tests/surface_lifecycle.rs` cannot reach this
/// path at all and a device is the only other way to see it.
fn cancellations(
    contacts: Vec<PointerDetails>,
    at: (f64, f64),
) -> impl Iterator<Item = PendingPointerInput> {
    contacts
        .into_iter()
        .map(move |pointer| PendingPointerInput::Cancel {
            physical_x: at.0,
            physical_y: at.1,
            pointer,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pointer_input::{PointerContact, PointerIdentity, PointerType};

    #[test]
    fn only_a_surface_that_was_here_and_went_stops_the_frame_loop() {
        assert!(!SurfaceState::Initial.is_lost());
        assert!(!SurfaceState::Present.is_lost());
        assert!(SurfaceState::Lost.is_lost());
    }

    /// Two fingers down when the surface goes: both are cancelled, once each.
    #[test]
    fn losing_the_surface_cancels_every_contact_at_the_position_it_was_last_at() {
        let mut ids = crate::pointer_input::PointerIds::default();
        let first = ids.details_for(PointerIdentity::touch_for_test(3), true);
        let second = ids.details_for(PointerIdentity::touch_for_test(4), false);

        let cancelled: Vec<_> = cancellations(ids.live(), (11.5, 22.5)).collect();
        assert_eq!(cancelled.len(), 2);
        let ids_and_positions: Vec<_> = cancelled
            .iter()
            .map(|input| match input {
                PendingPointerInput::Cancel {
                    physical_x,
                    physical_y,
                    pointer,
                } => (
                    pointer.pointer_id,
                    pointer.pointer_type,
                    *physical_x,
                    *physical_y,
                ),
                _ => panic!("a surface loss produces cancellations and nothing else"),
            })
            .collect();
        assert_eq!(
            ids_and_positions,
            vec![
                (first.pointer_id, PointerType::Touch, 11.5, 22.5),
                (second.pointer_id, PointerType::Touch, 11.5, 22.5),
            ]
        );

        // The mouse is not among them: it is never taken away silently, and a
        // spurious `pointercancel` for it would break every desktop drag.
        ids.details_for(PointerIdentity::mouse_for_test(), true);
        assert_eq!(cancellations(ids.live(), (0.0, 0.0)).count(), 2);
        assert!(!ids.is_live(PointerContact::Mouse));
    }
}
