//! The muda half: turning a parsed tree into the platform's own objects.
//!
//! muda is `tray-icon`'s own menu crate and is already linked behind it, so
//! the application menu costs no new dependency — the same builder produces
//! the status item's menu and the menu bar, which is what keeps the two
//! surfaces honouring one tree rather than two implementations of it.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;
use tray_icon::menu::{
    CheckMenuItem, IconMenuItem, IsMenuItem, Menu, MenuItem as NativeMenuItem, PredefinedMenuItem,
    Submenu,
};
use winit::event_loop::EventLoopProxy;

use super::{MenuEntry, MenuItemKind, MenuRole, MenuSignal, decode_icon, queue};

/// A process-unique prefix for every native id a controller hands out.
///
/// muda's event channel is global and carries no sender identity, so a
/// click queued by a menu that has since been replaced is indistinguishable
/// from a live one by anything except its id. A replacement takes a fresh
/// prefix, so the stale event matches no binding and is dropped.
static NEXT_MENU_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_id_prefix(surface: &str) -> String {
    format!(
        "blitsen-{surface}-{}",
        NEXT_MENU_ID.fetch_add(1, Ordering::Relaxed)
    )
}

#[derive(Clone)]
pub(crate) enum NativeMenuBinding {
    Action(MenuSignal),
    Check {
        signal: MenuSignal,
        item: CheckMenuItem,
        radio_group: Option<String>,
    },
}

pub(crate) type Bindings = HashMap<String, NativeMenuBinding>;

fn native_accelerator(
    accelerator: &Option<String>,
) -> Result<Option<tray_icon::menu::accelerator::Accelerator>, String> {
    accelerator
        .as_deref()
        .map(|accelerator| {
            let normalized = accelerator
                .split('+')
                .map(|part| match part.trim().to_ascii_lowercase().as_str() {
                    "cmdorctrl" | "commandorcontrol" if cfg!(target_os = "macos") => "Super",
                    "cmdorctrl" | "commandorcontrol" => "Control",
                    _ => part.trim(),
                })
                .collect::<Vec<_>>()
                .join("+");
            normalized
                .parse()
                .map_err(|error| format!("invalid accelerator {accelerator:?}: {error}"))
        })
        .transpose()
}

fn native_menu_icon(bytes: &[u8]) -> Result<tray_icon::menu::Icon, String> {
    let decoded = decode_icon(bytes)?;
    tray_icon::menu::Icon::from_rgba(decoded.rgba, decoded.width, decoded.height)
        .map_err(|error| format!("could not create menu icon: {error}"))
}

/// The predefined item a role installs, or `None` where the platform has
/// no such command.
///
/// Windows has no services menu, no "show all" and no application-wide
/// fullscreen item, and muda builds an inert entry for each — a line in the
/// menu that greys nothing and does nothing when clicked. Omitting them is
/// what PRODUCT.md §7 asks for: the role is absent on Windows rather than
/// present and dead.
fn predefined(role: MenuRole) -> Option<PredefinedMenuItem> {
    let macos_only = matches!(
        role,
        MenuRole::Services
            | MenuRole::ShowAll
            | MenuRole::HideOthers
            | MenuRole::Fullscreen
            | MenuRole::BringAllToFront
    );
    if macos_only && !cfg!(target_os = "macos") {
        return None;
    }
    Some(match role {
        MenuRole::About => PredefinedMenuItem::about(None, None),
        MenuRole::Services => PredefinedMenuItem::services(None),
        MenuRole::Hide => PredefinedMenuItem::hide(None),
        MenuRole::HideOthers => PredefinedMenuItem::hide_others(None),
        MenuRole::ShowAll => PredefinedMenuItem::show_all(None),
        MenuRole::Quit => PredefinedMenuItem::quit(None),
        MenuRole::CloseWindow => PredefinedMenuItem::close_window(None),
        MenuRole::Minimize => PredefinedMenuItem::minimize(None),
        MenuRole::Maximize => PredefinedMenuItem::maximize(None),
        MenuRole::Fullscreen => PredefinedMenuItem::fullscreen(None),
        MenuRole::BringAllToFront => PredefinedMenuItem::bring_all_to_front(None),
        MenuRole::Undo => PredefinedMenuItem::undo(None),
        MenuRole::Redo => PredefinedMenuItem::redo(None),
        MenuRole::Cut => PredefinedMenuItem::cut(None),
        MenuRole::Copy => PredefinedMenuItem::copy(None),
        MenuRole::Paste => PredefinedMenuItem::paste(None),
        MenuRole::SelectAll => PredefinedMenuItem::select_all(None),
    })
}

/// `append` is a trait object rather than a generic parameter, and it has to
/// stay one. A submenu recurses with a closure that appends to *that*
/// submenu, so a generic version instantiates itself at a new closure type
/// for every level of nesting and monomorphisation never terminates — which
/// rustc reports as a recursion limit rather than as the infinite regress it
/// is. Nothing here is hot enough to want the static dispatch back: this
/// runs once per menu configuration, not once per frame.
pub(crate) fn append_native_menu(
    entries: &[MenuEntry],
    id_prefix: &str,
    next_id: &mut usize,
    bindings: &mut Bindings,
    append: &mut dyn FnMut(&dyn IsMenuItem) -> Result<(), tray_icon::menu::Error>,
) -> Result<(), String> {
    for entry in entries {
        let native_id = format!("{id_prefix}-{}", *next_id);
        *next_id += 1;
        match entry {
            MenuEntry::Separator => append(&PredefinedMenuItem::separator())
                .map_err(|error| format!("could not create menu: {error}"))?,
            MenuEntry::Role(role) => {
                if let Some(item) = predefined(*role) {
                    append(&item)
                        .map_err(|error| format!("could not create menu role: {error}"))?;
                }
            }
            MenuEntry::Submenu {
                label,
                enabled,
                icon,
                menu,
                role: _,
            } => {
                let submenu = Submenu::with_id(&native_id, label, *enabled);
                append_native_menu(menu, id_prefix, next_id, bindings, &mut |item| {
                    submenu.append(item)
                })?;
                if let Some(icon) = icon {
                    submenu.set_icon(Some(native_menu_icon(icon)?));
                }
                append(&submenu).map_err(|error| format!("could not create submenu: {error}"))?;
            }
            MenuEntry::Item(item) => {
                let accelerator = native_accelerator(&item.accelerator)?;
                match &item.kind {
                    MenuItemKind::Action => {
                        if let Some(icon) = &item.icon {
                            let entry = IconMenuItem::with_id(
                                &native_id,
                                &item.label,
                                item.enabled,
                                Some(native_menu_icon(icon)?),
                                accelerator,
                            );
                            append(&entry)
                                .map_err(|error| format!("could not create menu item: {error}"))?;
                        } else {
                            let entry = NativeMenuItem::with_id(
                                &native_id,
                                &item.label,
                                item.enabled,
                                accelerator,
                            );
                            append(&entry)
                                .map_err(|error| format!("could not create menu item: {error}"))?;
                        }
                        bindings.insert(native_id, NativeMenuBinding::Action(item.signal.clone()));
                    }
                    MenuItemKind::Checkbox { checked } => {
                        let entry = CheckMenuItem::with_id(
                            &native_id,
                            &item.label,
                            item.enabled,
                            *checked,
                            accelerator,
                        );
                        append(&entry)
                            .map_err(|error| format!("could not create checkbox item: {error}"))?;
                        bindings.insert(
                            native_id,
                            NativeMenuBinding::Check {
                                signal: item.signal.clone(),
                                item: entry,
                                radio_group: None,
                            },
                        );
                    }
                    MenuItemKind::Radio { group, checked } => {
                        let entry = CheckMenuItem::with_id(
                            &native_id,
                            &item.label,
                            item.enabled,
                            *checked,
                            accelerator,
                        );
                        append(&entry)
                            .map_err(|error| format!("could not create radio item: {error}"))?;
                        bindings.insert(
                            native_id,
                            NativeMenuBinding::Check {
                                signal: item.signal.clone(),
                                item: entry,
                                radio_group: Some(group.clone()),
                            },
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn build_menu(
    entries: &[MenuEntry],
    id_prefix: &str,
    bindings: &mut Bindings,
) -> Result<Menu, String> {
    let menu = Menu::new();
    let mut next_id = 0;
    append_native_menu(entries, id_prefix, &mut next_id, bindings, &mut |item| {
        menu.append(item)
    })?;
    Ok(menu)
}

/// Drains muda's process-wide event channel.
///
/// There is exactly one receiver for every menu in the process, so no owner
/// may drain it alone: the first to look would swallow the other's clicks.
/// `native_window` takes the ids here once per turn and offers them to each
/// owner in order.
pub(crate) fn take_native_menu_events() -> Vec<String> {
    let mut ids = Vec::new();
    while let Ok(event) = tray_icon::menu::MenuEvent::receiver().try_recv() {
        ids.push(event.id().as_ref().to_owned());
    }
    ids
}

/// Queues the signals for the ids this owner's bindings recognise.
///
/// An id that matches nothing belonged to a menu that has since been
/// replaced or removed. Dropping it is the whole of "teardown ignores
/// already-queued stale events".
pub(crate) fn claim_native_menu_events(
    ids: &[String],
    bindings: &Bindings,
    pending: &Mutex<VecDeque<MenuSignal>>,
    proxy: &EventLoopProxy,
) {
    for id in ids {
        let Some(binding) = bindings.get(id).cloned() else {
            continue;
        };
        let signal = match binding {
            NativeMenuBinding::Action(signal) => signal,
            NativeMenuBinding::Check {
                signal,
                item,
                radio_group: None,
            } => signal.with_checked(item.is_checked()),
            NativeMenuBinding::Check {
                signal,
                item,
                radio_group: Some(group),
            } => {
                item.set_checked(true);
                for binding in bindings.values() {
                    if let NativeMenuBinding::Check {
                        item: other,
                        radio_group: Some(other_group),
                        ..
                    } = binding
                        && other_group == &group
                        && other.id() != item.id()
                    {
                        other.set_checked(false);
                    }
                }
                signal.with_checked(true)
            }
        };
        queue(signal, pending, proxy);
    }
}

/// The application menu owned by one native window session.
pub(crate) struct AppMenuController {
    entries: Vec<MenuEntry>,
    /// The name the synthesized macOS application submenu is titled with.
    #[cfg(target_os = "macos")]
    application: String,
    pending: Arc<Mutex<VecDeque<MenuSignal>>>,
    proxy: EventLoopProxy,
    bindings: Bindings,
    /// Retained because the native menu lives exactly as long as this does.
    menu: Option<Menu>,
    attached: bool,
    #[cfg(target_os = "windows")]
    installed_on: Option<isize>,
}

impl AppMenuController {
    pub(crate) fn new(entries: Vec<MenuEntry>, application: &str, proxy: EventLoopProxy) -> Self {
        #[cfg(not(target_os = "macos"))]
        let _ = application;
        Self {
            entries,
            #[cfg(target_os = "macos")]
            application: application.to_owned(),
            pending: Arc::new(Mutex::new(VecDeque::new())),
            proxy,
            bindings: Bindings::new(),
            menu: None,
            attached: false,
            #[cfg(target_os = "windows")]
            installed_on: None,
        }
    }

    /// Whether this menu is not on a platform surface yet.
    ///
    /// It stays true across several pump turns only on Windows, where a
    /// menu bar belongs to a window and the window is created some turns
    /// after the session opens. On macOS the main menu belongs to the
    /// application, which is what lets an application whose window starts
    /// hidden — or that never shows a tray icon — still have one.
    pub(crate) fn needs_install(&self) -> bool {
        !self.attached
    }

    /// Creates the native menu without putting it on any surface.
    ///
    /// Separate from [`Self::install`] so that a replacement is built
    /// before the menu it replaces is detached: a tree the platform refuses
    /// then leaves the running application's menu exactly as it was.
    pub(crate) fn build(&mut self) -> Result<(), String> {
        if self.menu.is_some() {
            return Ok(());
        }
        #[cfg(target_os = "macos")]
        let entries = super::with_required_roles(self.entries.clone(), &self.application);
        #[cfg(not(target_os = "macos"))]
        let entries = self.entries.clone();

        let mut bindings = Bindings::new();
        self.menu = Some(build_menu(
            &entries,
            &next_id_prefix("menu"),
            &mut bindings,
        )?);
        self.bindings = bindings;
        Ok(())
    }

    /// Builds the native menu if needed, then attaches it.
    ///
    /// `window` is the win32 `HWND` the menu bar belongs to and is ignored
    /// on macOS. On Windows a `None` handle is not a failure: the menu
    /// stays built and [`Self::needs_install`] keeps reporting that it is
    /// waiting for the window this session is opening.
    pub(crate) fn install(&mut self, window: Option<isize>) -> Result<(), String> {
        self.build()?;
        if self.attached {
            return Ok(());
        }
        let menu = self.menu.as_ref().expect("build leaves a menu behind");
        #[cfg(target_os = "macos")]
        {
            let _ = window;
            menu.init_for_nsapp();
            self.attached = true;
        }
        #[cfg(target_os = "windows")]
        {
            let Some(hwnd) = window else {
                return Ok(());
            };
            // SAFETY: the handle comes from the winit window this session
            // owns, and `uninstall` detaches before the menu is dropped.
            unsafe { menu.init_for_hwnd(hwnd) }
                .map_err(|error| format!("could not install the application menu: {error}"))?;
            self.installed_on = Some(hwnd);
            self.attached = true;
            accelerator_table::set(menu.haccel());
        }
        Ok(())
    }

    /// Detaches the native menu, leaving nothing for a stale event to hit.
    ///
    /// Explicit rather than a `Drop`: `remove_for_nsapp` sets the main menu
    /// to nothing whichever menu asks, so an old controller dropped after
    /// its replacement was attached would take the replacement with it.
    pub(crate) fn uninstall(&mut self) {
        let Some(menu) = self.menu.take() else {
            return;
        };
        self.bindings.clear();
        if self.attached {
            #[cfg(target_os = "macos")]
            menu.remove_for_nsapp();
            #[cfg(target_os = "windows")]
            {
                accelerator_table::set(0);
                if let Some(hwnd) = self.installed_on.take() {
                    // SAFETY: the handle is the one `install` attached to,
                    // and the window outlives the session owning this menu.
                    let _ = unsafe { menu.remove_for_hwnd(hwnd) };
                }
            }
            self.attached = false;
        }
    }

    pub(crate) fn claim(&self, ids: &[String]) {
        claim_native_menu_events(ids, &self.bindings, &self.pending, &self.proxy);
    }

    pub(crate) fn take_signals(&self) -> Vec<MenuSignal> {
        self.pending.lock().drain(..).collect()
    }
}

/// The accelerator table the win32 message hook translates against.
///
/// A Windows menu-bar accelerator only fires if `TranslateAcceleratorW`
/// runs inside the message pump, and the pump is winit's. The hook is
/// installed once when the event loop is built and reads the live table
/// from here, so replacing the menu replaces its accelerators without
/// rebuilding the event loop.
#[cfg(target_os = "windows")]
pub(crate) mod accelerator_table {
    use std::sync::atomic::{AtomicIsize, Ordering};

    static ACTIVE: AtomicIsize = AtomicIsize::new(0);

    pub(crate) fn set(handle: isize) {
        ACTIVE.store(handle, Ordering::Relaxed);
    }

    pub(crate) fn get() -> isize {
        ACTIVE.load(Ordering::Relaxed)
    }
}
