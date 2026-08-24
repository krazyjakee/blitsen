//! The menu tree shared by the system tray and the application menu.
//!
//! A status-item menu and a menu bar are the same declarative thing — nested
//! submenus, checkable items, radio groups, separators and accelerators — put
//! on two surfaces that have different owners and different lifetimes. One
//! model and one parser serve both, so an entry means the same thing wherever
//! it is written; what the two do not share is the handful of entries only one
//! surface can carry, and [`MenuSurface`] is where that decision is recorded
//! rather than in a second copy of the tree.

use std::collections::VecDeque;

use image::GenericImageView;
use parking_lot::Mutex;
use winit::event_loop::EventLoopProxy;

use crate::{MenuDefinition, TrayAction};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum MenuSignal {
    Command(TrayAction),
    Click,
    Action { id: String, checked: Option<bool> },
}

impl MenuSignal {
    pub(crate) fn with_checked(&self, checked: bool) -> Self {
        match self {
            Self::Action { id, .. } => Self::Action {
                id: id.clone(),
                checked: Some(checked),
            },
            signal => signal.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum MenuItemKind {
    Action,
    Checkbox { checked: bool },
    Radio { group: String, checked: bool },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MenuItem {
    pub(crate) label: String,
    pub(crate) enabled: bool,
    pub(crate) accelerator: Option<String>,
    pub(crate) icon: Option<Vec<u8>>,
    pub(crate) signal: MenuSignal,
    pub(crate) kind: MenuItemKind,
}

/// An entry whose behaviour is the platform's rather than the application's.
///
/// A role is not a label plus a callback. `Copy` is the first responder's copy
/// on macOS and `WM_COPY` on Windows; `Quit` is the terminate the platform
/// already knows how to run every other application's shutdown through. None
/// of that reaches application JavaScript, which is the point: an application
/// that had to implement `Paste` itself would be implementing it wrongly.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MenuRole {
    About,
    Services,
    Hide,
    HideOthers,
    ShowAll,
    Quit,
    CloseWindow,
    Minimize,
    Maximize,
    Fullscreen,
    BringAllToFront,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
}

impl MenuRole {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "about" => Self::About,
            "services" => Self::Services,
            "hide" => Self::Hide,
            "hideOthers" => Self::HideOthers,
            "showAll" => Self::ShowAll,
            "quit" => Self::Quit,
            "closeWindow" => Self::CloseWindow,
            "minimize" => Self::Minimize,
            "maximize" => Self::Maximize,
            "fullscreen" => Self::Fullscreen,
            "bringAllToFront" => Self::BringAllToFront,
            "undo" => Self::Undo,
            "redo" => Self::Redo,
            "cut" => Self::Cut,
            "copy" => Self::Copy,
            "paste" => Self::Paste,
            "selectAll" => Self::SelectAll,
            _ => return None,
        })
    }
}

/// The place a top-level application submenu occupies on macOS.
///
/// AppKit does not read a submenu's title to decide what it is; the first
/// submenu of the main menu *is* the application menu whatever it is called,
/// and the window and help menus are positional too. Declaring the role is
/// therefore how an application says "this one is mine" and takes the standard
/// contents Blitsen would otherwise have supplied for it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum SubmenuRole {
    Application,
    Edit,
    Window,
    Help,
}

impl SubmenuRole {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "application" => Self::Application,
            "edit" => Self::Edit,
            "window" => Self::Window,
            "help" => Self::Help,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum MenuEntry {
    Separator,
    Role(MenuRole),
    Item(MenuItem),
    Submenu {
        label: String,
        role: Option<SubmenuRole>,
        enabled: bool,
        icon: Option<Vec<u8>>,
        menu: Vec<MenuEntry>,
    },
}

/// Which surface a tree is being parsed for.
///
/// Three things differ, and all three are things one surface cannot represent
/// rather than things it merely does not use. A tray menu has no roles, because
/// a status item is not a responder chain and `Copy` there would copy from
/// nothing. An application menu has no `show`/`hide`/`quit` actions, because
/// those are the tray's own bargain with the window and `quit` is a role here.
/// And an application menu is a bar: every top-level entry is a submenu,
/// because that is the only thing a main menu or a menu bar can hold.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MenuSurface {
    Tray,
    Application,
}

impl MenuSurface {
    /// The word an error uses for this surface, so one parser reports two.
    fn noun(self) -> &'static str {
        match self {
            Self::Tray => "tray",
            Self::Application => "application",
        }
    }
}

/// Applies the one semantic contract used by every menu Blitsen installs.
pub(crate) fn parse_menu(
    raw: Vec<MenuDefinition>,
    icons: &[Vec<u8>],
    surface: MenuSurface,
) -> Result<(Vec<MenuEntry>, bool), String> {
    use std::collections::{HashMap, HashSet};

    struct State {
        ids: HashSet<String>,
        items: usize,
        has_quit: bool,
        surface: MenuSurface,
        submenu_roles: HashSet<SubmenuRole>,
    }

    fn icon(
        index: Option<usize>,
        icons: &[Vec<u8>],
        state: &State,
    ) -> Result<Option<Vec<u8>>, String> {
        let Some(index) = index else { return Ok(None) };
        if state.surface == MenuSurface::Application {
            return Err(
                "application menu entries cannot carry icons: a macOS main menu shows none"
                    .to_owned(),
            );
        }
        let bytes = icons
            .get(index)
            .ok_or_else(|| "tray menu icon index is out of range".to_owned())?;
        image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
            .map_err(|error| format!("tray menu icon is not a valid PNG: {error}"))?;
        Ok(Some(bytes.clone()))
    }

    fn non_empty(value: Option<String>, description: &str) -> Result<String, String> {
        let value = value.ok_or_else(|| format!("{description} is required"))?;
        if value.is_empty() {
            Err(format!("{description} must not be empty"))
        } else {
            Ok(value)
        }
    }

    fn accelerator(value: Option<String>, noun: &str) -> Result<Option<String>, String> {
        let Some(value) = value else { return Ok(None) };
        if value.is_empty() {
            return Err(format!("{noun} accelerators must not be empty"));
        }
        let parts = value.split('+').map(str::trim).collect::<Vec<_>>();
        if parts.iter().any(|part| part.is_empty()) {
            return Err(format!(
                "invalid {noun} accelerator {value:?}: empty key or modifier"
            ));
        }
        let is_modifier = |part: &str| {
            matches!(
                part.to_ascii_lowercase().as_str(),
                "ctrl"
                    | "control"
                    | "alt"
                    | "option"
                    | "shift"
                    | "cmd"
                    | "command"
                    | "super"
                    | "meta"
                    | "cmdorctrl"
                    | "commandorcontrol"
            )
        };
        let Some((key, modifiers)) = parts.split_last() else {
            unreachable!()
        };
        if is_modifier(key) || modifiers.iter().any(|part| !is_modifier(part)) {
            return Err(format!(
                "invalid {noun} accelerator {value:?}: modifiers must precede exactly one key"
            ));
        }
        let mut seen = HashSet::new();
        if modifiers
            .iter()
            .map(|part| part.to_ascii_lowercase())
            .any(|part| !seen.insert(part))
        {
            return Err(format!(
                "invalid {noun} accelerator {value:?}: duplicate modifier"
            ));
        }
        Ok(Some(value))
    }

    fn unique_id(value: Option<String>, state: &mut State) -> Result<String, String> {
        let noun = state.surface.noun();
        let id = non_empty(value, &format!("{noun} menu item id"))?;
        if !state.ids.insert(id.clone()) {
            return Err(format!(
                "{noun} menu item ids must be unique across the whole menu tree"
            ));
        }
        Ok(id)
    }

    fn parse_level(
        raw: Vec<MenuDefinition>,
        icons: &[Vec<u8>],
        depth: usize,
        state: &mut State,
    ) -> Result<Vec<MenuEntry>, String> {
        let noun = state.surface.noun();
        if depth > 16 {
            return Err(format!("{noun} menus may be nested at most 16 levels"));
        }
        let mut menu = Vec::with_capacity(raw.len());
        let mut active_radio_group: Option<String> = None;
        let mut closed_radio_groups = HashSet::new();
        let mut radio_checked = HashMap::<String, usize>::new();

        for item in raw {
            state.items += 1;
            if state.items > 512 {
                return Err(format!("{noun} menus may contain at most 512 entries"));
            }
            let kind = item.kind.as_deref().unwrap_or_else(|| {
                if item.action.as_deref() == Some("separator") {
                    "separator"
                } else {
                    "action"
                }
            });
            // A menu bar holds submenus and nothing else: macOS refuses
            // anything else outright, and a bare command in a Windows menu bar
            // fires on a single click with no menu ever opening.
            if state.surface == MenuSurface::Application && depth == 1 && kind != "submenu" {
                return Err("every top-level application menu entry must be a submenu".to_owned());
            }
            let radio_group =
                (kind == "radio").then(|| item.group.as_deref().unwrap_or_default().to_owned());
            if radio_group != active_radio_group {
                if let Some(group) = active_radio_group.take() {
                    closed_radio_groups.insert(group);
                }
                if let Some(group) = &radio_group {
                    if group.is_empty() {
                        return Err(format!("a radio {noun} item needs a non-empty group"));
                    }
                    if closed_radio_groups.contains(group) {
                        return Err(format!(
                            "items in a {noun} radio group must be consecutive at one menu level"
                        ));
                    }
                }
                active_radio_group = radio_group.clone();
            }

            match kind {
                "separator" => {
                    if item.id.is_some()
                        || item.label.is_some()
                        || item.menu.is_some()
                        || item.icon_index.is_some()
                    {
                        return Err(format!(
                            "a {noun} separator cannot have an id, label, menu or icon"
                        ));
                    }
                    menu.push(MenuEntry::Separator);
                }
                "role" => {
                    if state.surface != MenuSurface::Application {
                        return Err(format!("unknown {noun} menu item type: role"));
                    }
                    if item.id.is_some()
                        || item.action.is_some()
                        || item.label.is_some()
                        || item.menu.is_some()
                    {
                        return Err(
                            "a role item is the platform's own command: it cannot have an id, \
                             action, label or menu"
                                .to_owned(),
                        );
                    }
                    let role = non_empty(item.role, "an application menu role")?;
                    let role = MenuRole::parse(&role)
                        .ok_or_else(|| format!("unknown application menu role: {role}"))?;
                    menu.push(MenuEntry::Role(role));
                }
                "submenu" => {
                    if item.id.is_some() || item.action.is_some() || item.accelerator.is_some() {
                        return Err(format!(
                            "a {noun} submenu cannot have an id, action or accelerator"
                        ));
                    }
                    let role = item
                        .role
                        .as_deref()
                        .map(|role| {
                            if state.surface != MenuSurface::Application || depth != 1 {
                                return Err("only a top-level application submenu carries a role"
                                    .to_owned());
                            }
                            let parsed = SubmenuRole::parse(role).ok_or_else(|| {
                                format!("unknown application submenu role: {role}")
                            })?;
                            if !state.submenu_roles.insert(parsed) {
                                return Err(format!(
                                    "the application menu declares the {role} role twice"
                                ));
                            }
                            Ok(parsed)
                        })
                        .transpose()?;
                    let label = non_empty(item.label, &format!("{noun} submenu label"))?;
                    let children = item
                        .menu
                        .ok_or_else(|| format!("a {noun} submenu needs a menu array"))?;
                    let icon = icon(item.icon_index, icons, state)?;
                    menu.push(MenuEntry::Submenu {
                        label,
                        role,
                        enabled: item.enabled.unwrap_or(true),
                        icon,
                        menu: parse_level(children, icons, depth + 1, state)?,
                    });
                }
                "action" => {
                    if item.menu.is_some()
                        || item.checked.is_some()
                        || item.group.is_some()
                        || item.id.is_some() == item.action.is_some()
                    {
                        return Err(format!(
                            "an action {noun} item needs exactly one of id or action and cannot \
                             have menu, checked or group"
                        ));
                    }
                    let (label, signal) = if item.id.is_some() {
                        let id = unique_id(item.id, state)?;
                        (
                            non_empty(item.label, &format!("{noun} action label"))?,
                            MenuSignal::Action { id, checked: None },
                        )
                    } else {
                        if state.surface == MenuSurface::Application {
                            return Err("show, hide and quit are the tray's built-in actions; an \
                                 application menu spells quit as a role"
                                .to_owned());
                        }
                        let action = match item.action.as_deref() {
                            Some("show") => TrayAction::Show,
                            Some("hide") => TrayAction::Hide,
                            Some("quit") => {
                                state.has_quit = true;
                                TrayAction::Quit
                            }
                            Some(other) => {
                                return Err(format!("unknown tray menu action: {other}"));
                            }
                            None => unreachable!("the action discriminator was validated"),
                        };
                        (
                            item.label
                                .unwrap_or_else(|| action.default_label().to_owned()),
                            MenuSignal::Command(action),
                        )
                    };
                    let icon = icon(item.icon_index, icons, state)?;
                    menu.push(MenuEntry::Item(MenuItem {
                        label,
                        enabled: item.enabled.unwrap_or(true),
                        accelerator: accelerator(item.accelerator, noun)?,
                        icon,
                        signal,
                        kind: MenuItemKind::Action,
                    }));
                }
                "checkbox" | "radio" => {
                    if item.action.is_some() || item.menu.is_some() || item.icon_index.is_some() {
                        return Err(format!(
                            "a checkable {noun} item cannot have an action, submenu or icon"
                        ));
                    }
                    let id = unique_id(item.id, state)?;
                    let checked = item.checked.unwrap_or(false);
                    let item_kind = if kind == "checkbox" {
                        if item.group.is_some() {
                            return Err(format!("a checkbox {noun} item cannot have a group"));
                        }
                        MenuItemKind::Checkbox { checked }
                    } else {
                        let group = non_empty(item.group, &format!("{noun} radio group"))?;
                        *radio_checked.entry(group.clone()).or_default() += usize::from(checked);
                        MenuItemKind::Radio { group, checked }
                    };
                    menu.push(MenuEntry::Item(MenuItem {
                        label: non_empty(item.label, &format!("checkable {noun} item label"))?,
                        enabled: item.enabled.unwrap_or(true),
                        accelerator: accelerator(item.accelerator, noun)?,
                        icon: None,
                        signal: MenuSignal::Action {
                            id,
                            checked: Some(checked),
                        },
                        kind: item_kind,
                    }));
                }
                other => return Err(format!("unknown {noun} menu item type: {other}")),
            }
        }
        for (group, checked) in radio_checked {
            if checked != 1 {
                return Err(format!(
                    "{noun} radio group {group:?} must have exactly one checked item"
                ));
            }
        }
        Ok(menu)
    }

    let mut state = State {
        ids: HashSet::new(),
        items: 0,
        has_quit: false,
        surface,
        submenu_roles: HashSet::new(),
    };
    let menu = parse_level(raw, icons, 1, &mut state)?;
    Ok((menu, state.has_quit))
}

pub(crate) fn queue(
    signal: MenuSignal,
    pending: &Mutex<VecDeque<MenuSignal>>,
    proxy: &EventLoopProxy,
) {
    pending.lock().push_back(signal);
    proxy.wake_up();
}

/// The standard macOS submenu for a role the application did not declare.
///
/// Compiled into the test build on every platform, because this and
/// [`with_required_roles`] are the whole of "macOS always has the required
/// roles" and a rule only macOS could compile would be a rule nothing checks.
#[cfg(any(target_os = "macos", test))]
fn standard_submenu(role: SubmenuRole, application: &str) -> MenuEntry {
    use MenuRole::{
        About, BringAllToFront, CloseWindow, Copy, Cut, Fullscreen, Hide, HideOthers, Maximize,
        Minimize, Paste, Quit, Redo, SelectAll, Services, ShowAll, Undo,
    };

    let (label, roles): (String, &[MenuRole]) = match role {
        SubmenuRole::Application => (
            application.to_owned(),
            &[About, Services, Hide, HideOthers, ShowAll, Quit],
        ),
        SubmenuRole::Edit => (
            "Edit".to_owned(),
            &[Undo, Redo, Cut, Copy, Paste, SelectAll],
        ),
        SubmenuRole::Window => (
            "Window".to_owned(),
            &[Minimize, Maximize, Fullscreen, CloseWindow, BringAllToFront],
        ),
        SubmenuRole::Help => ("Help".to_owned(), &[]),
    };
    // A separator before each role that opens a new group, which is what makes
    // a synthesized menu read as the platform's own rather than as a flat list.
    let breaks: &[MenuRole] = &[Services, Hide, Quit, Cut, Fullscreen];
    let mut menu = Vec::with_capacity(roles.len() * 2);
    for role in roles {
        if breaks.contains(role) && !menu.is_empty() {
            menu.push(MenuEntry::Separator);
        }
        menu.push(MenuEntry::Role(*role));
    }
    MenuEntry::Submenu {
        label,
        role: Some(role),
        enabled: true,
        icon: None,
        menu,
    }
}

/// Places the roles a macOS main menu is required to have.
///
/// AppKit reads the *position* of a top-level submenu rather than its title:
/// the first one is the application menu whatever it is called, and the window
/// and help menus are the last two. So a submenu the application declared with
/// one of those roles is moved into place, and an application that declared
/// none still gets a working main menu — without the application submenu there
/// is no About or Quit anywhere, and without the edit submenu ⌘C and ⌘V do
/// nothing in a text field, because on macOS those are menu commands sent down
/// the responder chain rather than key events.
///
/// Help is placed and never synthesized: its role is a position, there is no
/// predefined command to put in one, and a submenu with nothing in it is a
/// greyed-out title.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn with_required_roles(entries: Vec<MenuEntry>, application: &str) -> Vec<MenuEntry> {
    fn take(entries: &mut Vec<MenuEntry>, wanted: SubmenuRole) -> Option<MenuEntry> {
        let index = entries.iter().position(
            |entry| matches!(entry, MenuEntry::Submenu { role: Some(role), .. } if *role == wanted),
        )?;
        Some(entries.remove(index))
    }

    let mut rest = entries;
    let application_menu = take(&mut rest, SubmenuRole::Application)
        .unwrap_or_else(|| standard_submenu(SubmenuRole::Application, application));
    let window_menu = take(&mut rest, SubmenuRole::Window)
        .unwrap_or_else(|| standard_submenu(SubmenuRole::Window, application));
    let help_menu = take(&mut rest, SubmenuRole::Help);
    let has_edit = rest.iter().any(|entry| {
        matches!(
            entry,
            MenuEntry::Submenu {
                role: Some(SubmenuRole::Edit),
                ..
            }
        )
    });

    let mut placed = Vec::with_capacity(rest.len() + 4);
    placed.push(application_menu);
    placed.append(&mut rest);
    // A declared edit menu keeps the position the application chose; a
    // synthesized one goes immediately before the window menu, which is where
    // the platform's own ordering puts it.
    if !has_edit {
        placed.push(standard_submenu(SubmenuRole::Edit, application));
    }
    placed.push(window_menu);
    placed.extend(help_menu);
    placed
}

pub(crate) struct DecodedIcon {
    pub(crate) rgba: Vec<u8>,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

pub(crate) fn decode_icon(bytes: &[u8]) -> Result<DecodedIcon, String> {
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .map_err(|error| format!("tray icon is not a valid PNG: {error}"))?;
    let (width, height) = image.dimensions();
    Ok(DecodedIcon {
        rgba: image.into_rgba8().into_raw(),
        width,
        height,
    })
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
mod native {
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
        CheckMenuItem, IconMenuItem, IsMenuItem, Menu, MenuItem as NativeMenuItem,
        PredefinedMenuItem, Submenu,
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
                    append(&submenu)
                        .map_err(|error| format!("could not create submenu: {error}"))?;
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
                                append(&entry).map_err(|error| {
                                    format!("could not create menu item: {error}")
                                })?;
                            } else {
                                let entry = NativeMenuItem::with_id(
                                    &native_id,
                                    &item.label,
                                    item.enabled,
                                    accelerator,
                                );
                                append(&entry).map_err(|error| {
                                    format!("could not create menu item: {error}")
                                })?;
                            }
                            bindings
                                .insert(native_id, NativeMenuBinding::Action(item.signal.clone()));
                        }
                        MenuItemKind::Checkbox { checked } => {
                            let entry = CheckMenuItem::with_id(
                                &native_id,
                                &item.label,
                                item.enabled,
                                *checked,
                                accelerator,
                            );
                            append(&entry).map_err(|error| {
                                format!("could not create checkbox item: {error}")
                            })?;
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
        pub(crate) fn new(
            entries: Vec<MenuEntry>,
            application: &str,
            proxy: EventLoopProxy,
        ) -> Self {
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
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
pub(crate) use native::{
    AppMenuController, Bindings, build_menu, claim_native_menu_events, next_id_prefix,
    take_native_menu_events,
};

#[cfg(target_os = "windows")]
pub(crate) use native::accelerator_table;

#[cfg(test)]
mod tests {
    use super::*;

    fn definition(json: serde_json::Value) -> Vec<MenuDefinition> {
        serde_json::from_value(json).expect("the fixture is a menu definition array")
    }

    #[test]
    fn an_application_menu_is_a_bar_of_submenus() {
        let flat = definition(serde_json::json!([{ "id": "open", "label": "Open" }]));
        assert!(
            parse_menu(flat, &[], MenuSurface::Application)
                .expect_err("a bare command cannot sit in a menu bar")
                .contains("top-level")
        );
        let nested = definition(serde_json::json!([
            { "type": "submenu", "label": "File", "menu": [{ "id": "open", "label": "Open" }] },
        ]));
        assert!(parse_menu(nested, &[], MenuSurface::Application).is_ok());
    }

    #[test]
    fn roles_belong_to_the_application_menu_and_tray_actions_do_not() {
        let role = serde_json::json!([
            { "type": "submenu", "label": "Edit", "menu": [{ "type": "role", "role": "copy" }] },
        ]);
        let (entries, _) = parse_menu(definition(role.clone()), &[], MenuSurface::Application)
            .expect("copy is a role");
        let MenuEntry::Submenu { menu, .. } = &entries[0] else {
            panic!("the edit menu stays a submenu")
        };
        assert_eq!(menu[0], MenuEntry::Role(MenuRole::Copy));
        assert!(parse_menu(definition(role), &[], MenuSurface::Tray).is_err());

        let quit = definition(serde_json::json!([
            { "type": "submenu", "label": "File", "menu": [{ "action": "quit" }] },
        ]));
        assert!(
            parse_menu(quit, &[], MenuSurface::Application)
                .expect_err("quit is a role in an application menu")
                .contains("role")
        );
    }

    #[test]
    fn a_submenu_role_is_top_level_and_declared_once() {
        let nested = definition(serde_json::json!([
            {
                "type": "submenu", "label": "File", "menu": [
                    { "type": "submenu", "role": "edit", "label": "Edit", "menu": [] },
                ],
            },
        ]));
        assert!(parse_menu(nested, &[], MenuSurface::Application).is_err());
        let twice = definition(serde_json::json!([
            { "type": "submenu", "role": "edit", "label": "Edit", "menu": [] },
            { "type": "submenu", "role": "edit", "label": "Also Edit", "menu": [] },
        ]));
        assert!(parse_menu(twice, &[], MenuSurface::Application).is_err());
    }

    /// The label and role of each top-level submenu, which is what the macOS
    /// placement rules are about.
    fn bar(entries: &[MenuEntry]) -> Vec<(&str, Option<SubmenuRole>)> {
        entries
            .iter()
            .map(|entry| match entry {
                MenuEntry::Submenu { label, role, .. } => (label.as_str(), *role),
                _ => panic!("an application menu is a bar of submenus"),
            })
            .collect()
    }

    #[test]
    fn macos_always_has_the_required_roles() {
        let (own, _) = parse_menu(
            definition(serde_json::json!([
                { "type": "submenu", "label": "File", "menu": [] },
            ])),
            &[],
            MenuSurface::Application,
        )
        .expect("one plain submenu is a valid bar");
        assert_eq!(
            bar(&with_required_roles(own, "Notes")),
            [
                ("Notes", Some(SubmenuRole::Application)),
                ("File", None),
                ("Edit", Some(SubmenuRole::Edit)),
                ("Window", Some(SubmenuRole::Window)),
            ]
        );

        // The synthesized application submenu really carries the roles, rather
        // than being an empty title in the right place.
        let MenuEntry::Submenu { menu, .. } = standard_submenu(SubmenuRole::Application, "Notes")
        else {
            panic!("a standard submenu is a submenu")
        };
        assert!(menu.contains(&MenuEntry::Role(MenuRole::About)));
        assert!(menu.contains(&MenuEntry::Role(MenuRole::Quit)));
    }

    #[test]
    fn a_declared_role_is_placed_rather_than_synthesized() {
        let (own, _) = parse_menu(
            definition(serde_json::json!([
                { "type": "submenu", "role": "help", "label": "Help", "menu": [] },
                { "type": "submenu", "role": "edit", "label": "Edit", "menu": [] },
                { "type": "submenu", "label": "File", "menu": [] },
                { "type": "submenu", "role": "application", "label": "Mine", "menu": [] },
            ])),
            &[],
            MenuSurface::Application,
        )
        .expect("declared roles are valid");
        // Application first and help last whatever order they were written in;
        // a declared edit menu keeps the place the application chose for it.
        assert_eq!(
            bar(&with_required_roles(own, "Notes")),
            [
                ("Mine", Some(SubmenuRole::Application)),
                ("Edit", Some(SubmenuRole::Edit)),
                ("File", None),
                ("Window", Some(SubmenuRole::Window)),
                ("Help", Some(SubmenuRole::Help)),
            ]
        );
    }

    #[test]
    fn the_shared_contract_still_holds_on_both_surfaces() {
        for surface in [MenuSurface::Tray, MenuSurface::Application] {
            let wrap = |items: serde_json::Value| match surface {
                MenuSurface::Tray => items,
                MenuSurface::Application => {
                    serde_json::json!([{ "type": "submenu", "label": "View", "menu": items }])
                }
            };
            let split = definition(wrap(serde_json::json!([
                { "type": "radio", "id": "light", "label": "Light", "group": "theme",
                  "checked": true },
                { "type": "separator" },
                { "type": "radio", "id": "dark", "label": "Dark", "group": "theme" },
            ])));
            assert!(parse_menu(split, &[], surface).is_err());
            let duplicated = definition(wrap(serde_json::json!([
                { "id": "open", "label": "Open" },
                { "id": "open", "label": "Open Again" },
            ])));
            assert!(parse_menu(duplicated, &[], surface).is_err());
            let accelerator = definition(wrap(serde_json::json!([
                { "id": "open", "label": "Open", "accelerator": "KeyO+Control" },
            ])));
            assert!(parse_menu(accelerator, &[], surface).is_err());
        }
    }
}
