//! Surface loss and recreation: what a window that can be taken away needs.
//!
//! On the six shipping desktop targets a window exists for the process's life,
//! and winit says so in code: `x11`, `wayland`, `appkit` and `win32` call
//! [`can_create_surfaces`] exactly once, at startup, and never call
//! `destroy_surfaces`, `suspended`, `resumed` or `memory_warning` at all. Only
//! `winit-uikit` and `winit-android` call the other four. So the honest
//! starting position for issue #146 is that four of the five handlers this
//! module owns have never executed on any target Blitsen ships — they were
//! delegation stubs, and a stub is not a desktop implementation that might
//! survive a cycle. There was nothing to survive.
//!
//! What Android does with them, read from `winit-android` 0.31.0-beta.2:
//!
//! | Android event | winit calls |
//! | --- | --- |
//! | `InitWindow` (surface created) | `can_create_surfaces` |
//! | `TerminateWindow` (surface destroyed) | `destroy_surfaces` |
//! | `Start` (`onStart`) | `resumed` |
//! | `Stop` (`onStop`) | `suspended` |
//! | `LowMemory` | `memory_warning` |
//! | `ConfigChanged` | `ScaleFactorChanged` window event |
//! | `WindowResized` | `SurfaceResized` window event |
//!
//! Note what `resumed`/`suspended` are *not*: they are `onStart`/`onStop`, not
//! `onResume`/`onPause`. The pause pair only flips a `running` flag inside
//! winit, which gates redraw and resize dispatch and is not visible from here.
//!
//! ## What a destroy/recreate cycle does to each piece of state
//!
//! The renderer is the only thing that actually dies. `View::suspend` drops
//! `anyrender_vello`'s `RenderState::Active` — the wgpu surface, the swapchain
//! and the `vello::Renderer` — and `View::resume` builds all three again from
//! the retained window handle and the document's own viewport. Everything else
//! is on this side of the boundary and is *kept*. Kept is the right answer for
//! most of it, and the wrong answer for exactly two — the last two below:
//!
//! * **The document, the JavaScript heap and the timer queue are untouched.**
//!   They are owned by [`WindowSession`](crate::WindowSession), not by the
//!   surface; nothing in the cycle drops or reloads them. This is the reason
//!   the config-change decision below matters so much — losing them is only
//!   possible by letting the Activity restart.
//! * **`<canvas>` and `<blitsen-view>` hold no GPU resources.** A canvas keeps
//!   a recorded `anyrender::Scene` and a native view keeps an `ImageData` of
//!   RGBA bytes; both are CPU-side and are re-uploaded by whatever renderer is
//!   active when the next frame paints. So the paint-side custom-widget seam
//!   needs nothing on a cycle — which is just as well, because `blitz-shell`'s
//!   `custom-widget` feature (the one that would call `destroy_surfaces` on the
//!   document) is not enabled in this workspace, only `blitz-dom`'s and
//!   `blitz-paint`'s. If a widget ever holds a texture, that feature has to go
//!   on and this comment has to change.
//! * **`started_at` is kept deliberately.** `requestAnimationFrame` timestamps
//!   are measured from it, and a clock that restarted would hand JavaScript a
//!   timestamp earlier than the last one it saw. Keeping it means the first
//!   timestamp after a long backgrounding jumps forward by however long the app
//!   was away, which is exactly what a browser tab does.
//! * **Live pointer contacts are wrong, and are cancelled here.** A finger down
//!   when the app is backgrounded is a contact the platform will never send a
//!   release for, and `dom_bridge/bootstrap/events.js` would hold `buttons`,
//!   capture and the pending `click` for it forever. Every live contact is
//!   spelled out as a `pointercancel` before the surface goes, which is what a
//!   browser does to a gesture interrupted by a page becoming hidden.
//! * **Modifier state is wrong, and is reset here.** Whether shift was held is
//!   a fact about a keyboard the app no longer has focus on.
//!
//! Two things are known-incomplete and named rather than papered over:
//!
//! * A key held down when the app is backgrounded gets no `keyup`. The host
//!   does not track which keys are down — JavaScript does — so synthesising one
//!   would mean moving that state across the boundary. Cancelling pointers is
//!   possible only because [`PointerIds`](crate::pointer_input::PointerIds)
//!   already had to keep the live contacts to allocate their ids.
//! * Nothing dispatches a DOM visibility change. `document.visibilityState` and
//!   `visibilitychange` do not exist in Blitsen's DOM at all, on any target, so
//!   there is no existing surface to drive from here; adding one is its own
//!   issue and its own conformance question.
//!
//! ## Decision: `memory_warning` trims the JavaScript heap and nothing else
//!
//! Android sends `LowMemory` to a process that is *still in the foreground and
//! still painting*. So the handler must not do anything that costs the user a
//! frame or a state: not dropping the document, not dropping the surface, not
//! clearing `<canvas>` backing stores — each of those is a visible regression
//! traded for memory the system may not even have needed.
//!
//! What is left is the JavaScript heap, which is the one large allocation
//! Blitsen owns that has slack in it by design: QuickJS collects on its own
//! threshold, so at any moment it is holding some quantity of unreachable
//! objects it has not got round to. [`JsEngine::collect_garbage`] runs that
//! collection early. It is implemented for QuickJS and left as the trait's
//! no-op default for JavaScriptCore and Node-API, because those two host only
//! desktop targets and no desktop backend delivers `memory_warning` — a GC that
//! can never be triggered is not worth the symbol lookup, and pretending
//! otherwise is the kind of claim this repo has been burned by.
//!
//! ## Decision: the frame loop stops while the surface is gone
//!
//! winit already stops *its* half. Between `onPause` and `onResume` the Android
//! backend refuses to dispatch `RedrawRequested`, and it drops the wake-ups a
//! `request_redraw` sends, so no frame turns and no `requestAnimationFrame`
//! callback runs. That matches a hidden browser tab, and it is the behaviour to
//! keep: rAF means "before the next paint", and there is no next paint.
//!
//! What winit cannot stop is Blitsen's *outer* loop, because that loop is not
//! winit's. `blitsen-runtime`'s session pumps with a zero timeout and paces
//! itself to 60 Hz whenever `animation_frames_pending()` is true — and it stays
//! true forever while backgrounded, because the callback that would clear the
//! queue is exactly the one that is not running. Left alone, a backgrounded
//! Blitsen application wakes 60 times a second to evaluate a script that says
//! "still nothing to draw". [`SurfaceState`] is what that loop reads to stop:
//! see `blitsen_runtime::loop_pacing::paces_a_frame`.
//!
//! The redraw request is stopped here as well, in `native_window`'s
//! `about_to_wait` and its redraw branch, so that a callback is not run for a
//! frame that cannot be presented on the four desktop backends either — they
//! have no `running` gate of their own and would happily go on delivering
//! `RedrawRequested` into a window whose surface had been destroyed.
//!
//! Timers keep running to their own schedule while suspended, unthrottled. A
//! browser clamps background timers to about 1 Hz, but a browser also gives the
//! page `document.hidden` to explain the clamp; Blitsen does not, so a clamp
//! here would be an unannounced change in what `setTimeout` means. The cost is
//! that an application which polls on a timer goes on polling in the
//! background. That is a policy at one seam and can be changed there once a
//! device measurement says what it is worth.
//!
//! ## Decision: declare `configChanges`, and handle the viewport change
//!
//! The manifest should carry
//! `android:configChanges="orientation|keyboardHidden|screenSize|screenLayout|smallestScreenSize|density|uiMode|layoutDirection"`.
//!
//! Without it, rotating the device destroys and recreates the Activity, and
//! with `android-activity` that means `android_main` returns and is called
//! again: a new JavaScript engine, a re-parsed document, everything the user
//! typed gone. There is no state-restoration path to soften it —
//! `MainEvent::SaveState` is an unimplemented `warn!("TODO")` in winit, and
//! Blitsen has no serialisation of a JavaScript heap to hand it if it were not.
//! Declaring the config changes is therefore not an optimisation, it is the
//! only version of rotation that keeps the application alive.
//!
//! What it gives up: the Android resource system stops re-resolving
//! configuration-qualified resources on rotation. Blitsen does not use it — the
//! layout is CSS and the assets are the app bundle's — so the price is one
//! Blitsen does not pay, and any future JNI code that reads a qualified
//! resource has to re-read it itself.
//!
//! With `configChanges` declared, a rotation is **not** a surface loss. It is a
//! `ConfigChanged` (which winit turns into `ScaleFactorChanged`) followed by a
//! `WindowResized` (which winit turns into `SurfaceResized`), and both already
//! run through the paths `native_window` built for a desktop window drag. The
//! handlers in this module are then for backgrounding and for the OS reclaiming
//! a surface, not for rotation.
//!
//! ## What is proven, and what still needs a device
//!
//! Android now has an entry point and CI-built APK, but the lifecycle still has
//! not been observed on a device or emulator. The local evidence is a synthetic
//! cycle on a desktop window: [`WindowSession::lose_surface`] and
//! [`WindowSession::restore_surface`] drive the real handlers with a real
//! `ActiveEventLoop`, tearing down and rebuilding a real wgpu surface. See
//! `tests/surface_lifecycle.rs` for the boundary of that evidence.
//!
//! [`can_create_surfaces`]: winit::application::ApplicationHandler::can_create_surfaces
//! [`JsEngine::collect_garbage`]: blitsen_js::JsEngine::collect_garbage

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
