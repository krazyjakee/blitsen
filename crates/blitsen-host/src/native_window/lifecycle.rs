//! Winit lifecycle routing: the [`ApplicationHandler`] implementation that
//! turns event-loop callbacks into input queues, frame pumping and native
//! signal delivery.

use blitsen_js::{JsEngine, JsError};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, StartCause, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

#[cfg(target_os = "macos")]
use winit::application::macos::ApplicationHandlerExtMacOS;

use super::{WindowApplication, menu, tray};

impl<Rend: anyrender::WindowRenderer, E: JsEngine + Clone> WindowApplication<Rend, E> {
    /// Delivers everything the tray and the application menu raised this turn.
    ///
    /// muda's menu-event channel is one channel for every menu in the process,
    /// so it is drained here rather than by either owner: whichever looked
    /// first would take the other's clicks. Each owner then claims the ids its
    /// own bindings recognise, and an id neither claims belonged to a menu that
    /// has since been replaced.
    fn apply_menu_signals(&mut self, event_loop: &dyn ActiveEventLoop) {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        let native = menu::take_native_menu_events();
        let tray_signals = match &self.tray {
            Some(tray) => {
                #[cfg(any(target_os = "windows", target_os = "macos"))]
                tray.claim(&native);
                tray.poll();
                tray.take_signals()
            }
            None => Vec::new(),
        };
        for signal in tray_signals {
            match signal {
                menu::MenuSignal::Command(crate::TrayAction::Show) => {
                    for view in self.inner.windows.values() {
                        view.window.set_visible(true);
                        view.window.focus_window();
                        view.window.request_redraw();
                    }
                }
                menu::MenuSignal::Command(crate::TrayAction::Hide) => {
                    for view in self.inner.windows.values() {
                        view.window.set_visible(false);
                    }
                }
                menu::MenuSignal::Command(crate::TrayAction::Quit) => {
                    self.quit_requested = true;
                    event_loop.exit();
                }
                menu::MenuSignal::Command(crate::TrayAction::Separator) => {}
                menu::MenuSignal::Click => crate::dom_bridge::tray::clicked(),
                menu::MenuSignal::Action { id, checked } => {
                    crate::dom_bridge::tray::action(id, checked);
                }
            }
        }
        // The application menu raises nothing but application-defined actions:
        // its roles are the platform's own commands and never enter JavaScript,
        // which is what separates a role from a custom item.
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        if let Some(app_menu) = &self.app_menu {
            app_menu.claim(&native);
            for signal in app_menu.take_signals() {
                if let menu::MenuSignal::Action { id, checked } = signal {
                    crate::dom_bridge::menu::action(id, checked);
                }
            }
        }
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        let menu_pending = crate::dom_bridge::menu::pending();
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        let menu_pending = false;
        if crate::dom_bridge::tray::pending() || menu_pending {
            for view in self.inner.windows.values() {
                view.window.request_redraw();
            }
        }
    }

    pub(super) fn animation_frames_pending(&self) -> bool {
        if self.has_parked_error() {
            return false;
        }
        let result = (|| {
            let mut engine = self.engine.clone();
            let hook = engine.retained_value(&self.host_hooks.animation_frames_pending)?;
            let pending = engine.call(&hook, None, &[])?;
            engine.to_boolean(&pending)
        })();
        match result {
            Ok(pending) => pending,
            Err(error) => {
                self.park_error(error);
                false
            }
        }
    }

    fn run_animation_frame(&self) {
        if self.has_parked_error() {
            return;
        }
        let timestamp = self.started_at.elapsed().as_secs_f64() * 1_000.0;
        let result = (|| {
            let mut engine = self.engine.clone();
            let timestamp = engine.number(timestamp);
            let hook = engine.retained_value(&self.host_hooks.animation_frame_tick)?;
            engine.call(&hook, None, &[timestamp])?;
            engine.drain_microtasks()?;
            Ok(())
        })();
        if let Err(error) = result {
            self.park_error(error);
        }
    }
}

impl<Rend: anyrender::WindowRenderer, E: JsEngine + Clone> ApplicationHandler
    for WindowApplication<Rend, E>
{
    fn new_events(&mut self, event_loop: &dyn ActiveEventLoop, cause: StartCause) {
        self.inner.new_events(event_loop, cause);
        if cause == StartCause::Init
            && let Some(tray) = &mut self.tray
            && let Err(error) = tray.initialize()
        {
            self.park_error(JsError::new(error));
        }
        self.apply_menu_signals(event_loop);
    }

    fn resumed(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.on_resumed(event_loop);
    }

    fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.on_can_create_surfaces(event_loop);
    }

    fn proxy_wake_up(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.inner.proxy_wake_up(event_loop);
        self.apply_menu_signals(event_loop);
        self.maybe_dispatch_load();
        // Renderer readiness and resource completion both arrive through the
        // proxy. Whichever one was last now schedules the hidden startup paint.
        self.request_startup_redraw_if_ready();
    }

    fn window_event(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        // Before anything else this turn does with the event, and whatever else
        // it does: the native snapshot is what an application polls instead of
        // listening, so it has to reflect the pointer even on the events this
        // handler goes on to consume itself.
        crate::dom_bridge::input::observe(
            &event,
            self.inner
                .windows
                .get(&window_id)
                .map_or(1.0, |view| view.window.scale_factor()),
        );
        if matches!(event, WindowEvent::CloseRequested)
            && self
                .tray
                .as_ref()
                .is_some_and(tray::TrayController::close_to_tray)
        {
            if let Some(view) = self.inner.windows.get(&window_id) {
                view.window.set_visible(false);
            }
            return;
        }
        // Held rather than acted on, and applied below once the turn's last one
        // is known. Winit has already coalesced the redraw requests that follow
        // it, so the frame this turn paints is the one that pays for the size.
        if let WindowEvent::SurfaceResized(size) = event {
            self.pending_resize.insert(window_id, size);
            if let Some(view) = self.inner.windows.get(&window_id) {
                view.window.request_redraw();
            }
            return;
        }
        if matches!(&event, WindowEvent::Focused(false)) {
            self.release_web_window_modes(window_id, "focus-loss");
        }
        if self.start_borderless_resize(window_id, &event) {
            return;
        }
        let suppress_absolute_pointer = crate::dom_bridge::window::web_pointer_locked()
            && matches!(
                &event,
                WindowEvent::PointerMoved { .. }
                    | WindowEvent::PointerEntered { .. }
                    | WindowEvent::PointerLeft { .. }
            );
        let queued_pointer_input =
            !suppress_absolute_pointer && self.queue_pointer_input(window_id, &event);
        let queued_keyboard_input = self.queue_keyboard_input(window_id, &event);
        let queued_drag_input = self.queue_drag_input(window_id, &event);
        // Blitz has its own editor-side keyboard and IME handlers, but they know
        // nothing about this runtime's DOM events. Letting the same event
        // continue there would mutate the shared editor before `keydown` or
        // `compositionupdate`, then mutate it a second time when the bridge
        // applies the event's default action.
        if matches!(
            &event,
            WindowEvent::KeyboardInput { .. } | WindowEvent::Ime(_)
        ) {
            if let Some(view) = self.inner.windows.get(&window_id) {
                view.window.request_redraw();
            }
            return;
        }
        let viewport_changed = matches!(&event, WindowEvent::ScaleFactorChanged { .. });
        let redraw = matches!(&event, WindowEvent::RedrawRequested);
        let startup_paint = redraw && self.prepare_startup_reveal(window_id);
        if redraw {
            // Before the frame rather than after it: a redraw that painted the
            // size before last would be a frame the drag visibly lagged by.
            self.apply_pending_resize(event_loop, window_id);
            self.drain_locked_pointer_movement(window_id);
            self.drain_pointer_input(window_id);
            self.drain_keyboard_input(window_id);
            self.drain_drag_input(window_id);
        }
        if redraw && !self.surface.is_lost() && !self.has_parked_error() {
            self.gamepads
                .poll(self.started_at.elapsed().as_secs_f64() * 1_000.0);
        }
        // `requestAnimationFrame` means "before the next paint", and a window
        // with no surface has no next paint. Android's winit backend stops
        // dispatching redraws entirely while the app is stopped; the desktop
        // backends have no such gate, so the rule is applied here instead and
        // means the same thing on every target (see `surface_lifecycle`).
        if redraw && !self.surface.is_lost() && (self.startup_revealed || startup_paint) {
            self.run_animation_frame();
        }
        // A startup rAF is allowed to discover another critical resource. Do
        // not let blitz-shell paint (or the platform map) until it has settled.
        let startup_paint = startup_paint
            && !self.has_parked_error()
            && !self
                .document
                .borrow()
                .document_ref()
                .has_pending_critical_resources();
        if !startup_paint
            && !self.startup_revealed
            && let Some(view) = self.inner.windows.get_mut(&window_id)
        {
            view.is_visible = false;
        }
        self.inner.window_event(event_loop, window_id, event);
        if redraw
            && !self.has_parked_error()
            && let Err(error) = self.sync_ime(window_id)
        {
            self.park_error(error);
        }
        if startup_paint {
            self.finish_startup_reveal(window_id);
        }
        // After Blitz has had the frame, because painting it re-resolves Blitz's
        // own hover state and sets a cursor from it.
        if redraw && !crate::dom_bridge::window::web_pointer_locked() {
            self.sync_cursor(window_id);
        }
        if viewport_changed {
            if !self.has_parked_error() {
                self.sync_native_window(window_id);
            }
            if !self.has_parked_error()
                && let Err(error) = self.dispatch_window_event("resize")
            {
                self.park_error(error);
            }
            if let Some(view) = self.inner.windows.get(&window_id) {
                view.window.request_redraw();
            }
        }
        if (queued_pointer_input || queued_keyboard_input || queued_drag_input)
            && let Some(view) = self.inner.windows.get(&window_id)
        {
            view.window.request_redraw();
        }
    }

    fn device_event(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
        device_id: Option<winit::event::DeviceId>,
        event: DeviceEvent,
    ) {
        if let DeviceEvent::PointerMotion { delta: (x, y) } = &event {
            crate::dom_bridge::input::pointer_movement(*x, *y);
            if crate::dom_bridge::window::web_pointer_locked()
                && let Some((&window_id, view)) = self.inner.windows.iter().next()
            {
                self.pending_locked_pointer_movement
                    .push((window_id, (*x, *y)));
                view.window.request_redraw();
            }
        }
        self.inner.device_event(event_loop, device_id, event);
    }

    fn about_to_wait(&mut self, event_loop: &dyn ActiveEventLoop) {
        // First: a synthetic cycle is meant to be indistinguishable from one
        // the platform sent, so it runs before the turn's other work, exactly
        // where a real `destroy_surfaces` would have landed.
        self.run_synthetic_phase(event_loop);
        self.apply_menu_signals(event_loop);
        self.inner.about_to_wait(event_loop);
        self.settle_native_resize(event_loop);
        // The turn's last reported size, applied once. A redraw in the same
        // turn has usually taken it already; this is the turn that had none.
        let windows: Vec<_> = self.pending_resize.keys().copied().collect();
        for window_id in windows {
            self.apply_pending_resize(event_loop, window_id);
        }
        self.maybe_dispatch_load();
        // The surface is asked before JavaScript is: a window that cannot
        // present has no frame to ask for. This retained callback is the turn's
        // single pending-work query, after the frame and native work settle.
        if !self.surface.is_lost() && self.animation_frames_pending() {
            for view in self.inner.windows.values() {
                view.window.request_redraw();
            }
        }
    }

    fn suspended(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.on_suspended(event_loop);
    }

    fn destroy_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.on_destroy_surfaces(event_loop);
    }

    fn memory_warning(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.on_memory_warning(event_loop);
    }

    #[cfg(target_os = "macos")]
    fn macos_handler(&mut self) -> Option<&mut dyn ApplicationHandlerExtMacOS> {
        Some(self)
    }
}

#[cfg(target_os = "macos")]
impl<Rend: anyrender::WindowRenderer, E: JsEngine + Clone> ApplicationHandlerExtMacOS
    for WindowApplication<Rend, E>
{
    fn standard_key_binding(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
        window_id: WindowId,
        action: &str,
    ) {
        self.inner
            .standard_key_binding(event_loop, window_id, action);
    }
}
