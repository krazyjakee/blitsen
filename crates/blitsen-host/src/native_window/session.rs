//! Opening, reloading and pumping one native-window session.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use blitsen_dom::{DomBackend, DomName};
use blitsen_js::{JsEngine, JsError};
use blitz::shell::{BlitzApplication, BlitzShellProxy, WindowConfig, create_default_event_loop};
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
}

impl<E: JsEngine + Clone + 'static> WindowSession<E> {
    /// Parses the entrypoint, runs its scripts, and opens a native window.
    pub fn open(
        engine: &mut E,
        files: AppFiles,
        options: OpenDirectoryOptions,
    ) -> Result<Self, JsError> {
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
        let event_loop = create_default_event_loop();
        let tray = options
            .tray
            .clone()
            .map(|tray| {
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
        let net_provider = files.net_provider().unwrap_or_else(|| {
            Arc::new(blitz::net::Provider::new(Some(Arc::new(proxy.clone()))))
                as Arc<dyn NetProvider>
        });
        let document = crate::app::load_document(
            engine,
            &files,
            net_provider,
            crate::app::LoadOptions::new(
                options.width,
                options.height,
                crate::dom_bridge::DocumentMode::Application,
            ),
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
            pending_pointer_input: Vec::new(),
            pending_keyboard_input: Vec::new(),
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
        })
    }

    /// Re-parses the entrypoint and replaces the document in the open window.
    pub fn reload(&mut self, engine: &mut E) -> Result<(), JsError> {
        let _guard = self.runtime.enter();
        let window_id = self
            .application
            .inner
            .windows
            .keys()
            .copied()
            .next()
            .ok_or_else(|| JsError::new("native window is not ready"))?;
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
        let document = crate::app::load_document(
            engine,
            &self.files,
            net_provider,
            crate::app::LoadOptions::new(
                logical.width,
                logical.height,
                crate::dom_bridge::DocumentMode::Application,
            )
            .with_viewport(viewport),
        )?;

        let view = self
            .application
            .inner
            .windows
            .get_mut(&window_id)
            .expect("window id was read from this map");
        view.replace_document(
            Box::new(SharedBlitzDocument(Rc::clone(&document.document))),
            false,
        );
        let application = &mut self.application;
        application.state = document.window_state;
        application.document = document.document;
        application.started_at = Instant::now();
        application.pending_pointer_input.clear();
        application.pending_keyboard_input.clear();
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
        if let Some(error) = self.error.borrow_mut().take() {
            return Err(error);
        }
        Ok(!self.application.quit_requested
            && (!self.application.inner.windows.is_empty()
                || !self.application.inner.pending_windows.is_empty()))
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
