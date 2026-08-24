//! Opening, reloading and pumping one native-window session.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use blitsen_dom::{DomBackend, DomName};
use blitsen_js::{JsEngine, JsError};
#[cfg(not(target_os = "windows"))]
use blitz::shell::create_default_event_loop;
use blitz::shell::{BlitzApplication, BlitzShellProxy, WindowConfig};
use blitz::traits::net::NetProvider;
use winit::dpi::LogicalSize;
use winit::event_loop::EventLoop;
use winit::event_loop::pump_events::EventLoopExtPumpEvents;
use winit::keyboard::ModifiersState;
use winit::monitor::Fullscreen;
use winit::window::{WindowAttributes, WindowLevel};

use super::{NativeWindowRenderer, SharedBlitzDocument, WindowApplication, native_window_renderer};
use crate::app::AppFiles;
use crate::pointer_input::PointerIds;
use crate::surface_lifecycle::SurfaceState;
use crate::{OpenDirectoryOptions, WindowType};

/// The winit loop this session pumps.
///
/// Everywhere but Windows this is `blitz::shell`'s own builder, unchanged. A
/// win32 menu-bar accelerator is the exception, and not a small one: the menu
/// bar delivers its own clicks through the subclass muda installs, but a key
/// combination only becomes one if `TranslateAcceleratorW` runs inside the
/// message pump, and the pump is winit's. `with_msg_hook` is the seam winit
/// offers for exactly this, and it has to be given at build time — so the loop
/// is built here rather than by the shared helper, and the table it translates
/// against is read live from the installed menu.
#[cfg(not(target_os = "windows"))]
fn create_event_loop() -> EventLoop {
    create_default_event_loop()
}

/// The winit loop this session pumps; see the non-Windows definition.
#[cfg(target_os = "windows")]
fn create_event_loop() -> EventLoop {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MSG, TranslateAcceleratorW};
    use winit::event_loop::ControlFlow;
    use winit::platform::windows::EventLoopBuilderExtWindows;

    let mut builder = EventLoop::builder();
    builder.with_msg_hook(|message| {
        let table = super::menu::accelerator_table::get();
        if table == 0 {
            return false;
        }
        let message = message.cast::<MSG>();
        // SAFETY: winit hands the hook the `MSG` it just peeked, and
        // `TranslateAcceleratorW` only reads it. The table belongs to the menu
        // installed on this thread and is cleared before that menu is dropped.
        unsafe { TranslateAcceleratorW((*message).hwnd, table as _, message) != 0 }
    });
    let event_loop = builder.build().expect("a winit event loop");
    event_loop.set_control_flow(ControlFlow::Wait);
    event_loop
}

/// One open native window, its document, and the I/O runtime behind them.
///
/// Both hosts drive a session the same way: [`open`](Self::open) once, then
/// [`pump`](Self::pump) until it reports the window is gone. Phase 1 pumps from
/// a task on Bun's loop (TECH.md §3, S1 option 1); Phase 2 pumps from its own
/// outer loop. Nothing else about the session differs between them.
pub struct WindowSession<E: JsEngine + Clone> {
    /// I/O runtime entered around every winit turn.
    pub runtime: tokio::runtime::Runtime,
    /// The winit loop, advanced without blocking by [`pump`](Self::pump).
    pub event_loop: EventLoop,
    /// Window, document and input translation.
    pub application: WindowApplication<NativeWindowRenderer, E>,
    /// The first error raised inside a winit callback, surfaced by `pump`.
    pub error: Rc<RefCell<Option<JsError>>>,
    /// Where the application's files come from, retained for reload.
    pub files: AppFiles,
    /// What the session was opened with, retained for reload.
    pub options: OpenDirectoryOptions,
    /// Durable storage shared by every reload of this session.
    storage: crate::storage::LocalStorage,
}

impl<E: JsEngine + Clone> Drop for WindowSession<E> {
    fn drop(&mut self) {
        // A notification is platform-owned once shown and deliberately
        // outlives a graceful process exit: that is what leaves something for
        // #252's registered entry point to activate. Reload calls `clear`
        // explicitly because it replaces one live document/session with
        // another; dropping the process only detaches callback state.
        self.application.notify.detach();
    }
}

impl<E: JsEngine + Clone + 'static> WindowSession<E> {
    /// Parses the entrypoint, runs its scripts, and opens a native window.
    pub fn open(
        engine: &mut E,
        files: AppFiles,
        options: OpenDirectoryOptions,
    ) -> Result<Self, JsError> {
        crate::dom_bridge::tray::reset();
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        crate::dom_bridge::menu::reset();
        crate::dom_bridge::notify::reset();
        // After the reset that empties the queue and before the document's
        // scripts run, which is the only window in which a launch context can be
        // both retained and still ahead of the listener that will receive it
        // (#252). A reload deliberately does not repeat this: the activation
        // belongs to the launch rather than to the document, and the store has
        // already recorded it as delivered.
        super::notify::install(&options.activation, &options.title).map_err(JsError::new)?;
        crate::dom_bridge::hid::reset();
        crate::dom_bridge::gamepad::reset();
        crate::dom_bridge::input::reset();
        let storage = crate::storage::LocalStorage::for_application(&options.storage_identity)
            .map_err(JsError::new)?;
        let started_at = Instant::now();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            // Network and audio work are asynchronous rather than CPU-parallel.
            // Tokio's default is one worker per core: 24 idle workers on the
            // benchmark host bought no throughput and retained allocator arenas.
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|error| JsError::new(error.to_string()))?;
        let guard = runtime.enter();
        let event_loop = create_event_loop();
        // Parsed here so that a menu the package configuration got wrong fails
        // the same way on every platform, whether or not this one installs one.
        let menu = options
            .menu
            .clone()
            .map(|entries| {
                super::menu::parse_menu(entries, &[], super::menu::MenuSurface::Application)
                    .map(|(entries, _)| entries)
            })
            .transpose()
            .map_err(JsError::new)?;
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        let app_menu = menu.map(|entries| {
            super::menu::AppMenuController::new(entries, &options.title, event_loop.create_proxy())
        });
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        drop(menu);
        let tray = options
            .tray
            .clone()
            .map(|tray| {
                let tray = super::tray::TraySpec::try_from(tray)?;
                super::tray::TrayController::new(
                    tray,
                    &options.title,
                    event_loop.create_proxy(),
                    &runtime,
                )
            })
            .transpose()
            .map_err(JsError::new)?;
        let (proxy, receiver) = BlitzShellProxy::new(event_loop.create_proxy());
        let notify = super::notify::NotifyController::new(event_loop.create_proxy());
        let net_provider = files.net_provider().unwrap_or_else(|| {
            Arc::new(blitz::net::Provider::new(Some(Arc::new(proxy.clone()))))
                as Arc<dyn NetProvider>
        });
        let document = crate::app::load_window_document(
            engine,
            &files,
            net_provider,
            crate::app::LoadOptions::new(
                options.width,
                options.height,
                crate::dom_bridge::DocumentMode::Application,
            )
            .with_storage(storage.clone()),
        )?;
        // Renderer selection happens before winit creates a surface. On an
        // unsafe target this must never construct wgpu: recovering from device
        // loss is too late when a Metal compute submission wedges WindowServer.
        let renderer = native_window_renderer();
        let window_options = &options.window;
        // A window this type asks to be shown is still created hidden:
        // blitz-shell otherwise maps it in `View::init`, before wgpu has a
        // surface or a frame. The reveal happens after the first complete
        // redraw instead; see `prepare_startup_reveal`. A `hidden` window is
        // the one that stays unmapped when that moment comes.
        let reveal_on_startup = window_options.window_type != WindowType::Hidden;
        let attributes = WindowAttributes::default()
            .with_title(options.title.clone())
            .with_surface_size(LogicalSize::new(options.width, options.height))
            .with_decorations(window_options.window_type != WindowType::Borderless)
            .with_fullscreen(
                (window_options.window_type == WindowType::Fullscreen)
                    .then(|| Fullscreen::Borderless(None)),
            )
            .with_visible(false)
            .with_resizable(window_options.resizable)
            .with_transparent(window_options.transparent)
            .with_window_level(if window_options.always_on_top {
                WindowLevel::AlwaysOnTop
            } else {
                WindowLevel::Normal
            });
        let window = WindowConfig::with_attributes(
            Box::new(SharedBlitzDocument(Rc::clone(&document.document))),
            renderer,
            attributes,
        );
        let mut application = BlitzApplication::new(proxy, receiver);
        application.add_window(window);
        let error = Rc::new(RefCell::new(None));
        let application = WindowApplication {
            inner: application,
            engine: engine.clone(),
            state: document.window_state,
            error: Rc::clone(&error),
            started_at,
            document: document.document,
            host_hooks: document.host_hooks,
            pending_pointer_input: Vec::new(),
            pending_locked_pointer_movement: Vec::new(),
            pending_keyboard_input: Vec::new(),
            ime_targets: HashMap::new(),
            pending_drag_input: Vec::new(),
            drag_paths: std::rc::Rc::from([]),
            pending_resize: HashMap::new(),
            applied_resize: HashMap::new(),
            pointer_positions: HashMap::new(),
            cursor_resolved_from: HashMap::new(),
            applied_cursor: HashMap::new(),
            pointer_ids: PointerIds::default(),
            modifiers: ModifiersState::empty(),
            load_dispatched: false,
            startup_revealed: false,
            reveal_on_startup,
            surface: SurfaceState::Initial,
            synthetic_phase: None,
            tray,
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            app_menu,
            notify,
            hid: super::hid::controller(event_loop.create_proxy()),
            gamepads: super::gamepad::Controller::platform(),
            quit_requested: false,
        };
        drop(guard);
        Ok(Self {
            runtime,
            event_loop,
            application,
            error,
            files,
            options,
            storage,
        })
    }

    /// Re-parses the entrypoint and replaces the document in the open window.
    pub fn reload(&mut self, engine: &mut E) -> Result<(), JsError> {
        let _guard = self.runtime.enter();
        crate::dom_bridge::tray::reset();
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        crate::dom_bridge::menu::reset();
        self.application.notify.clear();
        crate::dom_bridge::notify::reset();
        // A reload replaces the document, so every handle the previous one
        // opened is orphaned. Dropping the controller closes them; nothing else
        // would, and a device left claimed across a reload would be a device the
        // reloaded application cannot open.
        self.application.hid = super::hid::controller(self.event_loop.create_proxy());
        crate::dom_bridge::hid::reset();
        crate::dom_bridge::gamepad::reset();
        crate::dom_bridge::input::reset();
        let window_id = self
            .application
            .inner
            .windows
            .keys()
            .copied()
            .next()
            .ok_or_else(|| JsError::new("native window is not ready"))?;
        // A replaced document cannot own a cursor grab or fullscreen window.
        // Release the platform immediately; its DOM is about to be discarded,
        // so there is no old-document event to queue.
        crate::dom_bridge::window::release_web_modes();
        let viewport = self.application.inner.windows[&window_id]
            .doc
            .inner()
            .viewport()
            .clone();
        let scale = f64::from(viewport.hidpi_scale);
        let logical = winit::dpi::PhysicalSize::new(viewport.window_size.0, viewport.window_size.1)
            .to_logical::<u32>(scale);
        let proxy = self.application.inner.proxy.clone();
        let net_provider = self.files.net_provider().unwrap_or_else(|| {
            Arc::new(blitz::net::Provider::new(Some(Arc::new(proxy)))) as Arc<dyn NetProvider>
        });
        let document = crate::app::load_window_document(
            engine,
            &self.files,
            net_provider,
            crate::app::LoadOptions::new(
                logical.width,
                logical.height,
                crate::dom_bridge::DocumentMode::Application,
            )
            .with_viewport(viewport)
            .with_storage(self.storage.clone()),
        )?;

        let view = self
            .application
            .inner
            .windows
            .get_mut(&window_id)
            .expect("window id was read from this map");
        if self.application.ime_targets.remove(&window_id).is_some() {
            view.window
                .request_ime_update(winit::window::ImeRequest::Disable)
                .map_err(|error| {
                    JsError::new(format!("could not disable IME for reload: {error}"))
                })?;
        }
        view.replace_document(
            Box::new(SharedBlitzDocument(Rc::clone(&document.document))),
            false,
        );
        let application = &mut self.application;
        application.state = document.window_state;
        application.document = document.document;
        application.host_hooks = document.host_hooks;
        application.started_at = Instant::now();
        application.pending_pointer_input.clear();
        application.pending_locked_pointer_movement.clear();
        application.pending_keyboard_input.clear();
        application.pending_drag_input.clear();
        application.drag_paths = std::rc::Rc::from([]);
        application.pointer_positions.clear();
        application.cursor_resolved_from.clear();
        application.applied_cursor.clear();
        application.pointer_ids.clear();
        application.load_dispatched = false;
        Ok(())
    }

    /// Reloads one linked stylesheet without rerunning JavaScript.
    ///
    /// Reports whether the file was actually linked by the document.
    pub fn reload_css(&mut self, file: &str) -> Result<bool, JsError> {
        let _guard = self.runtime.enter();
        let root = Path::new(&self.options.root);
        let changed = root
            .join(file)
            .canonicalize()
            .map_err(|error| JsError::new(format!("could not reload CSS file {file}: {error}")))?;
        if !changed.starts_with(root) {
            return Err(JsError::new(format!(
                "CSS reload escaped application directory: {file}"
            )));
        }
        let href_name = DomName::attribute("href");
        let rel_name = DomName::attribute("rel");
        let hrefs = {
            let document = self.application.document.borrow();
            document
                .query_selector_all(document.document(), "link[href]")
                .map_err(crate::dom_error)?
                .into_iter()
                .filter_map(|node| {
                    let rel = document.attribute(node, &rel_name).ok().flatten()?;
                    if !rel
                        .split_ascii_whitespace()
                        .any(|value| value.eq_ignore_ascii_case("stylesheet"))
                    {
                        return None;
                    }
                    let href = document.attribute(node, &href_name).ok().flatten()?;
                    let local = href.split(['?', '#']).next().unwrap_or_default();
                    root.join(local)
                        .canonicalize()
                        .ok()
                        .filter(|candidate| *candidate == changed)
                        .map(|_| href)
                })
                .collect::<Vec<_>>()
        };
        for href in &hrefs {
            self.application
                .document
                .borrow_mut()
                .document_mut()
                .reload_resource_by_href(href);
        }
        Ok(!hrefs.is_empty())
    }

    /// Advances winit once without blocking, reporting whether a window remains.
    ///
    /// The one place a windowed frame turns. An error raised inside a winit
    /// callback cannot be returned from there, so it is parked and surfaces
    /// here, at the first call after it happened.
    pub fn pump(&mut self) -> Result<bool, JsError> {
        self.pump_for(Some(Duration::ZERO))
    }

    /// Advances winit once, waiting at most `timeout` for an event.
    ///
    /// `None` waits until an event arrives. The standalone runtime uses this
    /// while nothing is animating, so a static window consumes no polling
    /// turns. Callers embedded in another event loop should use [`Self::pump`].
    pub fn pump_for(&mut self, timeout: Option<Duration>) -> Result<bool, JsError> {
        let _guard = self.runtime.enter();
        let timeout = if timeout.is_none() && self.application.tray.is_some() {
            Some(Duration::from_millis(100))
        } else {
            timeout
        };
        self.event_loop
            .pump_app_events(timeout, &mut self.application);
        self.application.notify.poll();
        #[cfg(target_os = "linux")]
        if self.application.notify.take_present_request() {
            for view in self.application.inner.windows.values() {
                view.window.focus_window();
                view.window
                    .request_user_attention(Some(winit::window::UserAttentionType::Informational));
            }
        }
        if crate::dom_bridge::notify::pending() {
            self.request_redraw();
        }
        self.apply_tray_requests();
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        self.apply_menu_requests();
        self.apply_notify_requests();
        self.apply_hid_requests();
        if self.application.gamepads.apply_requests() {
            self.request_redraw();
        }
        self.application.hid.poll();
        if crate::dom_bridge::hid::pending() {
            self.request_redraw();
        }
        if let Some(error) = self.error.borrow_mut().take() {
            return Err(error);
        }
        if crate::dom_bridge::window::take_close_requested() {
            self.application.quit_requested = true;
        }
        Ok(!self.application.quit_requested
            && (!self.application.inner.windows.is_empty()
                || !self.application.inner.pending_windows.is_empty()))
    }

    fn apply_tray_requests(&mut self) {
        use crate::dom_bridge::tray::RequestKind;

        let requests = crate::dom_bridge::tray::take_requests();
        if requests.is_empty() {
            return;
        }
        for request in requests {
            let result = match request.kind {
                RequestKind::Configure(spec) => super::tray::TrayController::new(
                    spec,
                    &self.options.title,
                    self.event_loop.create_proxy(),
                    &self.runtime,
                )
                .and_then(|mut tray| {
                    tray.initialize()?;
                    self.application.tray = Some(tray);
                    Ok(())
                }),
                RequestKind::Remove => {
                    self.application.tray = None;
                    Ok(())
                }
            };
            crate::dom_bridge::tray::complete(request.command_id, result);
        }
        self.request_redraw();
    }

    /// Runs the HID commands JavaScript queued during the last frame turn.
    ///
    /// Enumeration and open both talk to the platform, so neither can happen
    /// inside the native call that requested it: winit already has the
    /// application borrowed there, and a device tree walk on that path is a
    /// frame the window did not paint.
    fn apply_hid_requests(&mut self) {
        use crate::dom_bridge::hid::{self, RequestKind};

        let requests = hid::take_requests();
        if requests.is_empty() {
            return;
        }
        let controller = &mut self.application.hid;
        for request in requests {
            let command_id = request.command_id;
            match request.kind {
                RequestKind::Devices => hid::complete(command_id, controller.devices()),
                // The one command that need not settle on the turn that made
                // it: Android raises a permission dialog, and `Ok(None)` means
                // the controller is holding this id until a person answers it.
                RequestKind::Open { device_id } => match controller.open(&device_id, command_id) {
                    Ok(None) => {}
                    Ok(Some(opened)) => hid::complete(command_id, Ok(opened)),
                    Err(failure) => hid::complete(command_id, Err(failure)),
                },
                RequestKind::Close { device_id } => {
                    hid::complete(command_id, controller.close(&device_id));
                }
                // The transfers settle on the device's own worker, so a failure
                // to *queue* one is the only thing answered here.
                RequestKind::Write { device_id, data } => {
                    if let Err(failure) = controller.write(&device_id, command_id, data) {
                        hid::complete(command_id, Err(failure));
                    }
                }
                RequestKind::SendFeatureReport { device_id, data } => {
                    if let Err(failure) =
                        controller.send_feature_report(&device_id, command_id, data)
                    {
                        hid::complete(command_id, Err(failure));
                    }
                }
                RequestKind::ReceiveFeatureReport {
                    device_id,
                    report_id,
                } => {
                    if let Err(failure) =
                        controller.receive_feature_report(&device_id, command_id, report_id)
                    {
                        hid::complete(command_id, Err(failure));
                    }
                }
            }
        }
        self.request_redraw();
    }

    /// Applies configure/remove requests, and attaches a menu that was waiting.
    ///
    /// Replacement is one step from JavaScript's side and three here, in this
    /// order: the incoming menu is *built* first, so a tree the platform
    /// refuses leaves the running one exactly as it was; then the outgoing one
    /// is detached, because on macOS detaching sets `NSApp.mainMenu` to nothing
    /// whichever menu asks and doing it second would take the replacement with
    /// it; then the replacement is attached. Nothing paints between the three,
    /// which is what makes the swap atomic as far as an application can tell.
    ///
    /// Dropping the old controller drops its bindings, so a click muda already
    /// queued against the old menu now matches no id and is ignored.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn apply_menu_requests(&mut self) {
        let requests = crate::dom_bridge::menu::take_requests();
        let settled = !requests.is_empty();
        for request in requests {
            let result = match request.kind {
                crate::dom_bridge::menu::RequestKind::Configure(entries) => {
                    super::menu::parse_menu(entries, &[], super::menu::MenuSurface::Application)
                        .and_then(|(entries, _)| {
                            let mut replacement = super::menu::AppMenuController::new(
                                entries,
                                &self.options.title,
                                self.event_loop.create_proxy(),
                            );
                            replacement.build()?;
                            if let Some(mut previous) = self.application.app_menu.take() {
                                previous.uninstall();
                            }
                            let attached = replacement.install(self.native_window_handle());
                            self.application.app_menu = Some(replacement);
                            attached
                        })
                }
                crate::dom_bridge::menu::RequestKind::Remove => {
                    if let Some(mut previous) = self.application.app_menu.take() {
                        previous.uninstall();
                    }
                    Ok(())
                }
            };
            crate::dom_bridge::menu::complete(request.command_id, result);
        }
        // The completion is a frame-turn message, so the frame it settles on
        // has to be asked for; nothing else in this turn would ask.
        if settled {
            self.request_redraw();
        }
        // A package-configured menu bar cannot be attached until the window it
        // belongs to exists, which is several turns after the session opened.
        let handle = self.native_window_handle();
        if let Some(menu) = &mut self.application.app_menu
            && menu.needs_install()
            && let Err(error) = menu.install(handle)
        {
            self.application.park_error(JsError::new(error));
        }
    }

    /// The win32 `HWND` a menu bar attaches to, once a window has one.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn native_window_handle(&self) -> Option<isize> {
        #[cfg(target_os = "windows")]
        {
            use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

            let view = self.application.inner.windows.values().next()?;
            match view.window.window_handle().ok()?.as_raw() {
                RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
                _ => None,
            }
        }
        #[cfg(not(target_os = "windows"))]
        None
    }

    fn apply_notify_requests(&mut self) {
        use crate::dom_bridge::notify::RequestKind;

        let requests = crate::dom_bridge::notify::take_requests();
        if requests.is_empty() {
            return;
        }
        for request in requests {
            match request.kind {
                RequestKind::RequestPermission => self
                    .application
                    .notify
                    .request_permission(request.command_id),
                RequestKind::Show { public_id, options } => {
                    let result = self.application.notify.show(public_id.clone(), options);
                    let error = result.as_ref().err().cloned();
                    let shown = result.is_ok();
                    crate::dom_bridge::notify::complete(request.command_id, result);
                    if shown {
                        crate::dom_bridge::notify::shown(public_id);
                    } else if let Some(error) = error {
                        crate::dom_bridge::notify::failed(public_id, error);
                    }
                }
                RequestKind::Update { public_id, patch } => {
                    let result = self.application.notify.update(&public_id, patch);
                    let error = result.as_ref().err().cloned();
                    crate::dom_bridge::notify::complete(request.command_id, result);
                    if let Some(error) = error {
                        crate::dom_bridge::notify::failed(public_id, error);
                    }
                }
                RequestKind::Close { public_id } => {
                    let result = self.application.notify.close(&public_id);
                    let error = result.as_ref().err().cloned();
                    crate::dom_bridge::notify::complete(request.command_id, result);
                    if let Some(error) = error {
                        crate::dom_bridge::notify::failed(public_id, error);
                    }
                }
            }
        }
        self.request_redraw();
    }

    /// Reports whether JavaScript has an animation-frame callback to run.
    pub fn animation_frames_pending(&self) -> bool {
        self.application.animation_frames_pending()
    }

    /// Schedules a redraw of every open window.
    pub fn request_redraw(&self) {
        for view in self.application.inner.windows.values() {
            view.window.request_redraw();
        }
    }

    /// Whether the first complete frame has been submitted and the window mapped.
    ///
    /// Frame-limited acceptance runs use this to avoid counting surface-setup
    /// turns as frames and exiting before the application was ever visible.
    pub fn startup_revealed(&self) -> bool {
        self.application.startup_revealed
    }
}
