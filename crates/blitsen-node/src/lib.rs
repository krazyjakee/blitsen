//! Bun-loadable Node-API addon: the Phase 1 JavaScript host.
//!
//! Two things live here and nowhere else — the [`JsEngine`](blitsen_js::JsEngine)
//! implementation over
//! Node-API, and the `#[napi]` surface Bun calls. Everything between the DOM and
//! the application is in `blitsen-host`, which names no engine at all.

mod engine;
mod exports;
mod workers;

use std::cell::RefCell;

use blitsen_host::app::AppFiles;
use blitsen_host::{
    MenuDefinition, NativeWindowOptions as HostWindowOptions, OpenDirectoryOptions as HostOptions,
    TrayAction, TrayMenu, TrayMenuItem as HostTrayMenuItem, TrayOptions as HostTrayOptions,
    WindowSession, WindowType, native_window,
};
use blitsen_js::JsError;
use napi::{Env, Status};
use napi_derive::napi;

pub(crate) use engine::napi_error;
pub use engine::{NodeApiEngine, NodeClass, NodeWeakRef};
pub use exports::*;

/// JavaScript-facing engine owner loaded by `new Engine()`.
#[napi]
pub struct Engine {
    runtime: RefCell<NodeApiEngine>,
    session: RefCell<Option<WindowSession<NodeApiEngine>>>,
}

/// Options passed from directory-mode CLI to the native window.
#[napi(object)]
#[derive(Clone)]
pub struct OpenDirectoryOptions {
    /// Canonical application root.
    pub root: String,
    /// Canonical `index.html` path.
    pub entrypoint: String,
    /// Initial logical width.
    pub width: u32,
    /// Initial logical height.
    pub height: u32,
    /// Native title-bar text.
    pub title: String,
    /// Original directory argument, retained for diagnostics.
    pub directory: String,
    /// Stable application identity used for durable Web Storage.
    pub storage_identity: Option<String>,
    /// Native creation-time window behavior.
    pub window: Option<NativeWindowOptions>,
    /// Optional system tray icon and context menu.
    pub tray: Option<NativeTrayOptions>,
    /// Optional application menu, independent of the tray.
    pub menu: Option<NativeAppMenuOptions>,
    /// Optional notification-activation identity and launch envelope.
    pub activation: Option<NativeActivationOptions>,
}

/// JavaScript-facing notification activation options (#252).
///
/// Carried by the Phase 1 host as well as Phase 2 because an export that links a
/// Node-API addon is still an installed application: the identity the packaging
/// step registered, and the envelope the platform launched it with, are
/// properties of the artifact rather than of which host it was linked into.
#[napi(object)]
#[derive(Clone)]
pub struct NativeActivationOptions {
    /// The installed application identity, when the export recorded one.
    pub identity: Option<String>,
    /// What the platform's notification service knows the entry point by.
    pub entry: Option<String>,
    /// The serialized activation envelope this process was launched with.
    pub launched_by: Option<String>,
}

#[napi(object)]
#[derive(Clone)]
/// JavaScript-facing native window creation options.
pub struct NativeWindowOptions {
    /// Initial presentation type.
    #[napi(js_name = "type")]
    pub window_type: Option<String>,
    /// Whether the window is resizable.
    pub resizable: Option<bool>,
    /// Whether the surface preserves alpha.
    pub transparent: Option<bool>,
    /// Whether an above-normal stacking level is requested.
    pub always_on_top: Option<bool>,
}

#[napi(object)]
#[derive(Clone)]
/// JavaScript-facing tray context-menu entry.
pub struct NativeTrayMenuItem {
    /// Built-in action name.
    pub action: String,
    /// Optional displayed label.
    pub label: Option<String>,
    /// Optional enabled state.
    pub enabled: Option<bool>,
}

#[napi(object)]
#[derive(Clone)]
/// JavaScript-facing system tray options.
pub struct NativeTrayOptions {
    /// PNG file path.
    pub icon: String,
    /// Optional hover tooltip.
    pub tooltip: Option<String>,
    /// Whether primary activation reveals the window.
    pub open_on_click: Option<bool>,
    /// Whether the native close control hides the window.
    pub close_to_tray: Option<bool>,
    /// Ordered context-menu entries.
    pub context_menu: Option<Vec<NativeTrayMenuItem>>,
    /// Rich recursive menu serialized as JSON by the CLI adapter.
    pub menu_json: Option<String>,
    /// PNG paths addressed by `iconIndex` values in `menu_json`.
    pub menu_icons: Option<Vec<String>>,
}

#[napi(object)]
#[derive(Clone)]
/// JavaScript-facing application-menu options.
///
/// Only the tree: an application menu carries no icon, no tooltip and no
/// window behaviour, which is most of what separates it from the tray.
pub struct NativeAppMenuOptions {
    /// Top-level submenus serialized as JSON by the CLI adapter.
    pub menu_json: String,
}

impl TryFrom<OpenDirectoryOptions> for HostOptions {
    type Error = JsError;

    fn try_from(options: OpenDirectoryOptions) -> Result<Self, Self::Error> {
        let window = options.window.map_or_else(
            || Ok(HostWindowOptions::default()),
            |window| {
                let window_type = match window.window_type.as_deref().unwrap_or("normal") {
                    "normal" => WindowType::Normal,
                    "borderless" => WindowType::Borderless,
                    "fullscreen" => WindowType::Fullscreen,
                    "hidden" => WindowType::Hidden,
                    value => return Err(JsError::new(format!("unknown window type: {value}"))),
                };
                Ok(HostWindowOptions {
                    window_type,
                    resizable: window.resizable.unwrap_or(true),
                    transparent: window.transparent.unwrap_or(false),
                    always_on_top: window.always_on_top.unwrap_or(false),
                })
            },
        )?;
        let tray = options
            .tray
            .map(|tray| {
                let icon = std::fs::read(&tray.icon).map_err(|error| {
                    JsError::new(format!("could not read tray icon {}: {error}", tray.icon))
                })?;
                let context_menu = tray
                    .context_menu
                    .unwrap_or_default()
                    .into_iter()
                    .map(|item| {
                        let action = match item.action.as_str() {
                            "show" => TrayAction::Show,
                            "hide" => TrayAction::Hide,
                            "quit" => TrayAction::Quit,
                            "separator" => TrayAction::Separator,
                            value => {
                                return Err(JsError::new(format!(
                                    "unknown tray menu action: {value}"
                                )));
                            }
                        };
                        Ok(HostTrayMenuItem {
                            action,
                            label: item.label,
                            enabled: item.enabled.unwrap_or(true),
                        })
                    })
                    .collect::<Result<Vec<_>, JsError>>()?;
                let menu = tray
                    .menu_json
                    .map(|json| {
                        let entries: Vec<MenuDefinition> =
                            serde_json::from_str(&json).map_err(|error| {
                                JsError::new(format!("invalid tray menu configuration: {error}"))
                            })?;
                        let icons = tray
                            .menu_icons
                            .unwrap_or_default()
                            .into_iter()
                            .map(|path| {
                                std::fs::read(&path).map_err(|error| {
                                    JsError::new(format!(
                                        "could not read tray menu icon {path}: {error}"
                                    ))
                                })
                            })
                            .collect::<Result<Vec<_>, _>>()?;
                        Ok(TrayMenu { entries, icons })
                    })
                    .transpose()?;
                Ok(HostTrayOptions {
                    icon,
                    tooltip: tray.tooltip,
                    open_on_click: tray.open_on_click.unwrap_or(true),
                    close_to_tray: tray.close_to_tray.unwrap_or(false),
                    context_menu,
                    menu,
                })
            })
            .transpose()?;
        let menu = options
            .menu
            .map(|menu| {
                serde_json::from_str::<Vec<MenuDefinition>>(&menu.menu_json).map_err(|error| {
                    JsError::new(format!("invalid application menu configuration: {error}"))
                })
            })
            .transpose()?;
        Ok(Self {
            storage_identity: options
                .storage_identity
                .unwrap_or_else(|| options.root.clone()),
            root: options.root,
            entrypoint: options.entrypoint,
            width: options.width,
            height: options.height,
            title: options.title,
            directory: options.directory,
            window,
            tray,
            menu,
            // An identity is only an identity when both halves are present: the
            // application it names, and what the platform's notification service
            // knows the entry point by. A launch envelope with neither is
            // refused by the session, which is where that sentence is written.
            activation: options.activation.map_or_else(
                blitsen_host::ActivationOptions::default,
                |activation| blitsen_host::ActivationOptions {
                    entry_point: activation.identity.zip(activation.entry).map(
                        |(identity, entry)| blitsen_host::ActivationEntryPoint { identity, entry },
                    ),
                    launched_by: activation.launched_by,
                },
            ),
        })
    }
}

#[napi]
impl Engine {
    /// Creates an engine in the current Bun/Node-API environment.
    #[napi(constructor)]
    pub fn new(env: Env) -> Self {
        Self {
            runtime: RefCell::new(NodeApiEngine::new(env)),
            session: RefCell::new(None),
        }
    }

    /// Parses `index.html` and initializes a native Blitz window session.
    #[napi(js_name = "openDirectory")]
    pub fn open_directory(&self, options: OpenDirectoryOptions) -> napi::Result<()> {
        // Rejected before any observable work: a second call must not build a
        // runtime, create an event loop, or run the document's scripts again.
        if self.session.borrow().is_some() {
            return Err(napi::Error::new(
                Status::GenericFailure,
                "a native window session is already open",
            ));
        }
        let options: HostOptions = options.try_into().map_err(napi_error)?;
        // A URL is a dev server to read the application from rather than a
        // directory to read it from (#67). Both hosts take the same branch,
        // because both open the same session over the same `AppFiles`.
        let files = if options.entrypoint.starts_with("http://")
            || options.entrypoint.starts_with("https://")
        {
            AppFiles::server(&options.entrypoint).map_err(napi_error)?
        } else {
            AppFiles::directory(&options.entrypoint).map_err(napi_error)?
        };
        let mut engine = *self.runtime.borrow();
        let session = WindowSession::open(&mut engine, files, options).map_err(napi_error)?;
        *self.session.borrow_mut() = Some(session);
        Ok(())
    }

    /// Re-parses the directory entrypoint and replaces the document in the existing window.
    #[napi(js_name = "reloadDirectory")]
    pub fn reload_directory(&self) -> napi::Result<()> {
        self.with_session(|session, engine| session.reload(engine))
    }

    /// Reloads a linked CSS file into the current document without rerunning JavaScript.
    #[napi(js_name = "reloadCSS")]
    pub fn reload_css(&self, file: String) -> napi::Result<bool> {
        self.with_session(|session, _| session.reload_css(&file))
    }

    /// Advances winit once without blocking Bun's outer event loop.
    #[napi(js_name = "pumpWindow")]
    pub fn pump_window(&self) -> napi::Result<bool> {
        let alive = self.with_session(|session, _| session.pump())?;
        if !alive {
            native_window::release_window();
            self.session.borrow_mut().take();
        }
        Ok(alive)
    }

    fn with_session<T>(
        &self,
        action: impl FnOnce(&mut WindowSession<NodeApiEngine>, &mut NodeApiEngine) -> Result<T, JsError>,
    ) -> napi::Result<T> {
        let mut session = self.session.borrow_mut();
        let session = session.as_mut().ok_or_else(|| {
            napi::Error::new(Status::GenericFailure, "no native window session is open")
        })?;
        let mut engine = *self.runtime.borrow();
        action(session, &mut engine).map_err(napi_error)
    }
}
