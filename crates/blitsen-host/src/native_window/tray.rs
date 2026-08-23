//! Declarative system tray support owned by one native window session.

use std::collections::VecDeque;
#[cfg(any(target_os = "windows", target_os = "macos"))]
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use image::GenericImageView;
use winit::event_loop::EventLoopProxy;

use crate::{TrayAction, TrayMenuDefinition, TrayOptions};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum TraySignal {
    Command(TrayAction),
    Click,
    Action { id: String, checked: Option<bool> },
}

impl TraySignal {
    fn with_checked(&self, checked: bool) -> Self {
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
pub(crate) enum TrayItemKind {
    Action,
    Checkbox { checked: bool },
    Radio { group: String, checked: bool },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TrayItem {
    pub(crate) label: String,
    pub(crate) enabled: bool,
    pub(crate) accelerator: Option<String>,
    pub(crate) icon: Option<Vec<u8>>,
    pub(crate) signal: TraySignal,
    pub(crate) kind: TrayItemKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum TrayEntry {
    Separator,
    Item(TrayItem),
    Submenu {
        label: String,
        enabled: bool,
        icon: Option<Vec<u8>>,
        menu: Vec<TrayEntry>,
    },
}

#[derive(Clone)]
pub(crate) struct TraySpec {
    pub(crate) icon: Vec<u8>,
    pub(crate) tooltip: Option<String>,
    pub(crate) open_on_click: bool,
    pub(crate) close_to_tray: bool,
    pub(crate) menu: Vec<TrayEntry>,
}

impl TryFrom<TrayOptions> for TraySpec {
    type Error = String;

    fn try_from(options: TrayOptions) -> Result<Self, Self::Error> {
        let menu = if let Some(menu) = options.menu {
            let (entries, has_quit) = parse_menu(menu.entries, &menu.icons)?;
            if options.close_to_tray && !has_quit {
                return Err("closeToTray requires a quit action in the tray menu".to_owned());
            }
            entries
        } else {
            options
                .context_menu
                .into_iter()
                .map(|item| {
                    let action = item.action;
                    if action == TrayAction::Separator {
                        return TrayEntry::Separator;
                    }
                    TrayEntry::Item(TrayItem {
                        label: item
                            .label
                            .unwrap_or_else(|| action.default_label().to_owned()),
                        enabled: item.enabled,
                        accelerator: None,
                        icon: None,
                        signal: TraySignal::Command(action),
                        kind: TrayItemKind::Action,
                    })
                })
                .collect()
        };
        Ok(Self {
            icon: options.icon,
            tooltip: options.tooltip,
            open_on_click: options.open_on_click,
            close_to_tray: options.close_to_tray,
            menu,
        })
    }
}

/// Applies the one semantic contract used by runtime and packaged tray menus.
pub(crate) fn parse_menu(
    raw: Vec<TrayMenuDefinition>,
    icons: &[Vec<u8>],
) -> Result<(Vec<TrayEntry>, bool), String> {
    use std::collections::{HashMap, HashSet};

    struct State {
        ids: HashSet<String>,
        items: usize,
        has_quit: bool,
    }

    fn icon(index: Option<usize>, icons: &[Vec<u8>]) -> Result<Option<Vec<u8>>, String> {
        let Some(index) = index else { return Ok(None) };
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

    fn accelerator(value: Option<String>) -> Result<Option<String>, String> {
        let Some(value) = value else { return Ok(None) };
        if value.is_empty() {
            return Err("tray accelerators must not be empty".to_owned());
        }
        let parts = value.split('+').map(str::trim).collect::<Vec<_>>();
        if parts.iter().any(|part| part.is_empty()) {
            return Err(format!(
                "invalid tray accelerator {value:?}: empty key or modifier"
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
                "invalid tray accelerator {value:?}: modifiers must precede exactly one key"
            ));
        }
        let mut seen = HashSet::new();
        if modifiers
            .iter()
            .map(|part| part.to_ascii_lowercase())
            .any(|part| !seen.insert(part))
        {
            return Err(format!(
                "invalid tray accelerator {value:?}: duplicate modifier"
            ));
        }
        Ok(Some(value))
    }

    fn unique_id(value: Option<String>, state: &mut State) -> Result<String, String> {
        let id = non_empty(value, "tray menu item id")?;
        if !state.ids.insert(id.clone()) {
            return Err("tray menu item ids must be unique across the whole menu tree".to_owned());
        }
        Ok(id)
    }

    fn parse_level(
        raw: Vec<TrayMenuDefinition>,
        icons: &[Vec<u8>],
        depth: usize,
        state: &mut State,
    ) -> Result<Vec<TrayEntry>, String> {
        if depth > 16 {
            return Err("tray menus may be nested at most 16 levels".to_owned());
        }
        let mut menu = Vec::with_capacity(raw.len());
        let mut active_radio_group: Option<String> = None;
        let mut closed_radio_groups = HashSet::new();
        let mut radio_checked = HashMap::<String, usize>::new();

        for item in raw {
            state.items += 1;
            if state.items > 512 {
                return Err("tray menus may contain at most 512 entries".to_owned());
            }
            let kind = item.kind.as_deref().unwrap_or_else(|| {
                if item.action.as_deref() == Some("separator") {
                    "separator"
                } else {
                    "action"
                }
            });
            let radio_group =
                (kind == "radio").then(|| item.group.as_deref().unwrap_or_default().to_owned());
            if radio_group != active_radio_group {
                if let Some(group) = active_radio_group.take() {
                    closed_radio_groups.insert(group);
                }
                if let Some(group) = &radio_group {
                    if group.is_empty() {
                        return Err("a radio tray item needs a non-empty group".to_owned());
                    }
                    if closed_radio_groups.contains(group) {
                        return Err(
                            "items in a tray radio group must be consecutive at one menu level"
                                .to_owned(),
                        );
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
                        return Err(
                            "a tray separator cannot have an id, label, menu or icon".to_owned()
                        );
                    }
                    menu.push(TrayEntry::Separator);
                }
                "submenu" => {
                    if item.id.is_some() || item.action.is_some() || item.accelerator.is_some() {
                        return Err(
                            "a tray submenu cannot have an id, action or accelerator".to_owned()
                        );
                    }
                    let label = non_empty(item.label, "tray submenu label")?;
                    let children = item
                        .menu
                        .ok_or_else(|| "a tray submenu needs a menu array".to_owned())?;
                    menu.push(TrayEntry::Submenu {
                        label,
                        enabled: item.enabled.unwrap_or(true),
                        icon: icon(item.icon_index, icons)?,
                        menu: parse_level(children, icons, depth + 1, state)?,
                    });
                }
                "action" => {
                    if item.menu.is_some()
                        || item.checked.is_some()
                        || item.group.is_some()
                        || item.id.is_some() == item.action.is_some()
                    {
                        return Err("an action tray item needs exactly one of id or action and cannot have menu, checked or group".to_owned());
                    }
                    let (label, signal) = if item.id.is_some() {
                        let id = unique_id(item.id, state)?;
                        (
                            non_empty(item.label, "tray action label")?,
                            TraySignal::Action { id, checked: None },
                        )
                    } else {
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
                            TraySignal::Command(action),
                        )
                    };
                    menu.push(TrayEntry::Item(TrayItem {
                        label,
                        enabled: item.enabled.unwrap_or(true),
                        accelerator: accelerator(item.accelerator)?,
                        icon: icon(item.icon_index, icons)?,
                        signal,
                        kind: TrayItemKind::Action,
                    }));
                }
                "checkbox" | "radio" => {
                    if item.action.is_some() || item.menu.is_some() || item.icon_index.is_some() {
                        return Err(
                            "a checkable tray item cannot have an action, submenu or icon"
                                .to_owned(),
                        );
                    }
                    let id = unique_id(item.id, state)?;
                    let checked = item.checked.unwrap_or(false);
                    let item_kind = if kind == "checkbox" {
                        if item.group.is_some() {
                            return Err("a checkbox tray item cannot have a group".to_owned());
                        }
                        TrayItemKind::Checkbox { checked }
                    } else {
                        let group = non_empty(item.group, "tray radio group")?;
                        *radio_checked.entry(group.clone()).or_default() += usize::from(checked);
                        TrayItemKind::Radio { group, checked }
                    };
                    menu.push(TrayEntry::Item(TrayItem {
                        label: non_empty(item.label, "checkable tray item label")?,
                        enabled: item.enabled.unwrap_or(true),
                        accelerator: accelerator(item.accelerator)?,
                        icon: None,
                        signal: TraySignal::Action {
                            id,
                            checked: Some(checked),
                        },
                        kind: item_kind,
                    }));
                }
                other => return Err(format!("unknown tray menu item type: {other}")),
            }
        }
        for (group, checked) in radio_checked {
            if checked != 1 {
                return Err(format!(
                    "tray radio group {group:?} must have exactly one checked item"
                ));
            }
        }
        Ok(menu)
    }

    let mut state = State {
        ids: HashSet::new(),
        items: 0,
        has_quit: false,
    };
    let menu = parse_level(raw, icons, 1, &mut state)?;
    Ok((menu, state.has_quit))
}

fn queue(signal: TraySignal, pending: &Mutex<VecDeque<TraySignal>>, proxy: &EventLoopProxy) {
    crate::dom_bridge::net_lock(pending).push_back(signal);
    proxy.wake_up();
}

struct DecodedIcon {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

fn decode_icon(bytes: &[u8]) -> Result<DecodedIcon, String> {
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
static NEXT_TRAY_ID: AtomicU64 = AtomicU64::new(1);

#[cfg(any(target_os = "windows", target_os = "macos"))]
#[derive(Clone)]
enum NativeMenuBinding {
    Action(TraySignal),
    Check {
        signal: TraySignal,
        item: tray_icon::menu::CheckMenuItem,
        radio_group: Option<String>,
    },
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
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
                .map_err(|error| format!("invalid tray accelerator {accelerator:?}: {error}"))
        })
        .transpose()
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn native_menu_icon(bytes: &[u8]) -> Result<tray_icon::menu::Icon, String> {
    let decoded = decode_icon(bytes)?;
    tray_icon::menu::Icon::from_rgba(decoded.rgba, decoded.width, decoded.height)
        .map_err(|error| format!("could not create tray menu icon: {error}"))
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn append_native_menu<F>(
    entries: &[TrayEntry],
    id_prefix: &str,
    next_id: &mut usize,
    bindings: &mut std::collections::HashMap<String, NativeMenuBinding>,
    mut append: F,
) -> Result<(), String>
where
    F: FnMut(&dyn tray_icon::menu::IsMenuItem) -> Result<(), tray_icon::menu::Error>,
{
    use tray_icon::menu::{CheckMenuItem, IconMenuItem, MenuItem, PredefinedMenuItem, Submenu};

    for entry in entries {
        let native_id = format!("{id_prefix}-{}", *next_id);
        *next_id += 1;
        match entry {
            TrayEntry::Separator => append(&PredefinedMenuItem::separator())
                .map_err(|error| format!("could not create tray menu: {error}"))?,
            TrayEntry::Submenu {
                label,
                enabled,
                icon,
                menu,
            } => {
                let submenu = Submenu::with_id(&native_id, label, *enabled);
                append_native_menu(menu, id_prefix, next_id, bindings, |item| {
                    submenu.append(item)
                })?;
                if let Some(icon) = icon {
                    submenu.set_icon(Some(native_menu_icon(icon)?));
                }
                append(&submenu)
                    .map_err(|error| format!("could not create tray submenu: {error}"))?;
            }
            TrayEntry::Item(item) => {
                let accelerator = native_accelerator(&item.accelerator)?;
                match &item.kind {
                    TrayItemKind::Action => {
                        if let Some(icon) = &item.icon {
                            let entry = IconMenuItem::with_id(
                                &native_id,
                                &item.label,
                                item.enabled,
                                Some(native_menu_icon(icon)?),
                                accelerator,
                            );
                            append(&entry).map_err(|error| {
                                format!("could not create tray menu item: {error}")
                            })?;
                        } else {
                            let entry = MenuItem::with_id(
                                &native_id,
                                &item.label,
                                item.enabled,
                                accelerator,
                            );
                            append(&entry).map_err(|error| {
                                format!("could not create tray menu item: {error}")
                            })?;
                        }
                        bindings.insert(native_id, NativeMenuBinding::Action(item.signal.clone()));
                    }
                    TrayItemKind::Checkbox { checked } => {
                        let entry = CheckMenuItem::with_id(
                            &native_id,
                            &item.label,
                            item.enabled,
                            *checked,
                            accelerator,
                        );
                        append(&entry).map_err(|error| {
                            format!("could not create tray checkbox item: {error}")
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
                    TrayItemKind::Radio { group, checked } => {
                        let entry = CheckMenuItem::with_id(
                            &native_id,
                            &item.label,
                            item.enabled,
                            *checked,
                            accelerator,
                        );
                        append(&entry).map_err(|error| {
                            format!("could not create tray radio item: {error}")
                        })?;
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

pub(crate) struct TrayController {
    pending: Arc<Mutex<VecDeque<TraySignal>>>,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    proxy: EventLoopProxy,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    open_on_click: bool,
    close_to_tray: bool,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    options: TraySpec,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    decoded: Option<DecodedIcon>,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    icon: Option<tray_icon::TrayIcon>,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    menu_actions: std::collections::HashMap<String, NativeMenuBinding>,
    #[cfg(target_os = "linux")]
    _service: ksni::Handle<LinuxTray>,
}

impl TrayController {
    pub(crate) fn new(
        options: TraySpec,
        application_title: &str,
        proxy: EventLoopProxy,
        runtime: &tokio::runtime::Runtime,
    ) -> Result<Self, String> {
        let decoded = decode_icon(&options.icon)?;
        let pending = Arc::new(Mutex::new(VecDeque::new()));

        #[cfg(target_os = "linux")]
        let service = {
            use ksni::TrayMethods;
            let mut argb = decoded.rgba.clone();
            for pixel in argb.as_chunks_mut::<4>().0 {
                pixel.rotate_right(1);
            }
            let tray = LinuxTray {
                title: application_title.to_owned(),
                tooltip: options.tooltip.clone(),
                icon: ksni::Icon {
                    width: decoded.width as i32,
                    height: decoded.height as i32,
                    data: argb,
                },
                menu: options.menu.clone(),
                open_on_click: options.open_on_click,
                pending: Arc::clone(&pending),
                proxy: proxy.clone(),
            };
            runtime
                .block_on(tray.spawn())
                .map_err(|error| format!("could not create tray icon: {error}"))?
        };

        #[cfg(not(target_os = "linux"))]
        let _ = (runtime, application_title);
        #[cfg(target_os = "android")]
        let _ = (&decoded, &proxy);

        Ok(Self {
            pending,
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            proxy,
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            open_on_click: options.open_on_click,
            close_to_tray: options.close_to_tray,
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            options,
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            decoded: Some(decoded),
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            icon: None,
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            menu_actions: std::collections::HashMap::new(),
            #[cfg(target_os = "linux")]
            _service: service,
        })
    }

    pub(crate) fn close_to_tray(&self) -> bool {
        self.close_to_tray
    }

    /// Creates AppKit/Win32 tray state after winit's event loop has started.
    pub(crate) fn initialize(&mut self) -> Result<(), String> {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        {
            use tray_icon::menu::Menu;

            if self.icon.is_some() {
                return Ok(());
            }
            let menu = Menu::new();
            let tray_id = NEXT_TRAY_ID.fetch_add(1, Ordering::Relaxed);
            let id_prefix = format!("blitsen-tray-{tray_id}");
            let mut next_id = 0;
            append_native_menu(
                &self.options.menu,
                &id_prefix,
                &mut next_id,
                &mut self.menu_actions,
                |item| menu.append(item),
            )?;
            let decoded = self.decoded.take().expect("a tray icon is decoded once");
            let icon = tray_icon::Icon::from_rgba(decoded.rgba, decoded.width, decoded.height)
                .map_err(|error| format!("could not create tray icon: {error}"))?;
            let mut builder = tray_icon::TrayIconBuilder::new()
                .with_id(&id_prefix)
                .with_menu(Box::new(menu))
                .with_icon(icon);
            if let Some(tooltip) = &self.options.tooltip {
                builder = builder.with_tooltip(tooltip);
            }
            self.icon = Some(
                builder
                    .build()
                    .map_err(|error| format!("could not create tray icon: {error}"))?,
            );
        }
        Ok(())
    }

    /// Pulls native menu/icon events into the session's single command slot.
    pub(crate) fn poll(&self) {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        {
            use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};

            while let Ok(event) = tray_icon::menu::MenuEvent::receiver().try_recv() {
                let Some(binding) = self.menu_actions.get(event.id().as_ref()).cloned() else {
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
                        for binding in self.menu_actions.values() {
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
                queue(signal, &self.pending, &self.proxy);
            }
            while let Ok(event) = TrayIconEvent::receiver().try_recv() {
                if matches!(
                    event,
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    }
                ) {
                    queue(TraySignal::Click, &self.pending, &self.proxy);
                    if self.open_on_click {
                        queue(
                            TraySignal::Command(TrayAction::Show),
                            &self.pending,
                            &self.proxy,
                        );
                    }
                }
            }
        }
    }

    pub(crate) fn take_signals(&self) -> Vec<TraySignal> {
        crate::dom_bridge::net_lock(&self.pending)
            .drain(..)
            .collect()
    }
}

#[cfg(target_os = "linux")]
struct LinuxTray {
    title: String,
    tooltip: Option<String>,
    icon: ksni::Icon,
    menu: Vec<TrayEntry>,
    open_on_click: bool,
    pending: Arc<Mutex<VecDeque<TraySignal>>>,
    proxy: EventLoopProxy,
}

#[cfg(target_os = "linux")]
fn action_id(signal: &TraySignal) -> Option<&str> {
    match signal {
        TraySignal::Action { id, .. } => Some(id),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn toggle_checkbox(entries: &mut [TrayEntry], id: &str) -> Option<bool> {
    for entry in entries {
        match entry {
            TrayEntry::Item(item) if action_id(&item.signal) == Some(id) => {
                if let TrayItemKind::Checkbox { checked } = &mut item.kind {
                    *checked = !*checked;
                    return Some(*checked);
                }
            }
            TrayEntry::Submenu { menu, .. } => {
                if let Some(checked) = toggle_checkbox(menu, id) {
                    return Some(checked);
                }
            }
            TrayEntry::Separator | TrayEntry::Item(_) => {}
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn select_radio(entries: &mut [TrayEntry], id: &str) -> bool {
    let group = entries.iter().find_map(|entry| match entry {
        TrayEntry::Item(item) if action_id(&item.signal) == Some(id) => match &item.kind {
            TrayItemKind::Radio { group, .. } => Some(group.clone()),
            _ => None,
        },
        _ => None,
    });
    if let Some(group) = group {
        for entry in entries {
            if let TrayEntry::Item(item) = entry
                && let TrayItemKind::Radio {
                    group: item_group,
                    checked,
                } = &mut item.kind
                && item_group == &group
            {
                *checked = action_id(&item.signal) == Some(id);
            }
        }
        return true;
    }
    for entry in entries {
        if let TrayEntry::Submenu { menu, .. } = entry
            && select_radio(menu, id)
        {
            return true;
        }
    }
    false
}

#[cfg(target_os = "linux")]
fn linux_shortcut(accelerator: &Option<String>) -> Vec<Vec<String>> {
    accelerator
        .as_deref()
        .map(|accelerator| {
            vec![
                accelerator
                    .split('+')
                    .map(|part| match part.trim().to_ascii_lowercase().as_str() {
                        "ctrl" | "control" | "cmdorctrl" | "commandorcontrol" => "Control".into(),
                        "alt" | "option" => "Alt".into(),
                        "shift" => "Shift".into(),
                        "cmd" | "command" | "super" | "meta" => "Super".into(),
                        _ => part.trim().to_owned(),
                    })
                    .collect(),
            ]
        })
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
fn linux_menu(entries: &[TrayEntry]) -> Vec<ksni::MenuItem<LinuxTray>> {
    use ksni::menu::{CheckmarkItem, RadioGroup, RadioItem, StandardItem, SubMenu};

    let mut menu = Vec::with_capacity(entries.len());
    let mut index = 0;
    while index < entries.len() {
        match &entries[index] {
            TrayEntry::Separator => menu.push(ksni::MenuItem::Separator),
            TrayEntry::Submenu {
                label,
                enabled,
                icon,
                menu: children,
            } => menu.push(
                SubMenu {
                    label: label.clone(),
                    enabled: *enabled,
                    icon_data: icon.clone().unwrap_or_default(),
                    submenu: linux_menu(children),
                    ..Default::default()
                }
                .into(),
            ),
            TrayEntry::Item(item) => match &item.kind {
                TrayItemKind::Action => {
                    let signal = item.signal.clone();
                    menu.push(
                        StandardItem {
                            label: item.label.clone(),
                            enabled: item.enabled,
                            icon_data: item.icon.clone().unwrap_or_default(),
                            shortcut: linux_shortcut(&item.accelerator),
                            activate: Box::new(move |tray: &mut LinuxTray| {
                                queue(signal.clone(), &tray.pending, &tray.proxy);
                            }),
                            ..Default::default()
                        }
                        .into(),
                    );
                }
                TrayItemKind::Checkbox { checked } => {
                    let signal = item.signal.clone();
                    let id = action_id(&signal)
                        .expect("a checkbox has a public id")
                        .to_owned();
                    menu.push(
                        CheckmarkItem {
                            label: item.label.clone(),
                            enabled: item.enabled,
                            checked: *checked,
                            shortcut: linux_shortcut(&item.accelerator),
                            activate: Box::new(move |tray: &mut LinuxTray| {
                                if let Some(checked) = toggle_checkbox(&mut tray.menu, &id) {
                                    queue(signal.with_checked(checked), &tray.pending, &tray.proxy);
                                }
                            }),
                            ..Default::default()
                        }
                        .into(),
                    );
                }
                TrayItemKind::Radio { group, .. } => {
                    let group = group.clone();
                    let start = index;
                    let mut end = index;
                    let mut selected = 0;
                    let mut signals = Vec::new();
                    let mut options = Vec::new();
                    while let Some(TrayEntry::Item(radio)) = entries.get(end) {
                        let TrayItemKind::Radio {
                            group: radio_group,
                            checked,
                        } = &radio.kind
                        else {
                            break;
                        };
                        if radio_group != &group {
                            break;
                        }
                        if *checked {
                            selected = end - start;
                        }
                        signals.push(radio.signal.clone());
                        options.push(RadioItem {
                            label: radio.label.clone(),
                            enabled: radio.enabled,
                            shortcut: linux_shortcut(&radio.accelerator),
                            ..Default::default()
                        });
                        end += 1;
                    }
                    menu.push(
                        RadioGroup {
                            selected,
                            options,
                            select: Box::new(move |tray: &mut LinuxTray, selected| {
                                let Some(signal) = signals.get(selected) else {
                                    return;
                                };
                                let id = action_id(signal).expect("a radio item has a public id");
                                if select_radio(&mut tray.menu, id) {
                                    queue(signal.with_checked(true), &tray.pending, &tray.proxy);
                                }
                            }),
                        }
                        .into(),
                    );
                    index = end - 1;
                }
            },
        }
        index += 1;
    }
    menu
}

#[cfg(target_os = "linux")]
impl ksni::Tray for LinuxTray {
    fn id(&self) -> String {
        self.title.clone()
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![self.icon.clone()]
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            icon_pixmap: vec![self.icon.clone()],
            title: self.tooltip.clone().unwrap_or_else(|| self.title.clone()),
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        queue(TraySignal::Click, &self.pending, &self.proxy);
        if self.open_on_click {
            queue(
                TraySignal::Command(TrayAction::Show),
                &self.pending,
                &self.proxy,
            );
        }
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        linux_menu(&self.menu)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event_item(id: &str, kind: TrayItemKind) -> TrayEntry {
        let checked = match &kind {
            TrayItemKind::Checkbox { checked } | TrayItemKind::Radio { checked, .. } => {
                Some(*checked)
            }
            TrayItemKind::Action => None,
        };
        TrayEntry::Item(TrayItem {
            label: id.into(),
            enabled: true,
            accelerator: None,
            icon: None,
            signal: TraySignal::Action {
                id: id.into(),
                checked,
            },
            kind,
        })
    }

    #[test]
    fn configured_actions_become_signals_and_separators_do_not() {
        let spec = TraySpec::try_from(TrayOptions {
            icon: Vec::new(),
            tooltip: None,
            open_on_click: true,
            close_to_tray: false,
            context_menu: vec![crate::TrayMenuItem {
                label: None,
                action: TrayAction::Show,
                enabled: true,
            }],
            menu: None,
        })
        .expect("legacy tray options are valid");
        assert!(matches!(
            &spec.menu[0],
            TrayEntry::Item(TrayItem {
                signal: TraySignal::Command(TrayAction::Show),
                ..
            })
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn checkbox_and_nested_radio_state_changes_before_the_event_is_queued() {
        let mut menu = vec![
            event_item("launch", TrayItemKind::Checkbox { checked: false }),
            TrayEntry::Submenu {
                label: "Theme".into(),
                enabled: true,
                icon: None,
                menu: vec![
                    event_item(
                        "light",
                        TrayItemKind::Radio {
                            group: "theme".into(),
                            checked: true,
                        },
                    ),
                    event_item(
                        "dark",
                        TrayItemKind::Radio {
                            group: "theme".into(),
                            checked: false,
                        },
                    ),
                ],
            },
        ];
        assert_eq!(toggle_checkbox(&mut menu, "launch"), Some(true));
        assert!(select_radio(&mut menu, "dark"));
        let TrayEntry::Submenu { menu: theme, .. } = &menu[1] else {
            panic!("the theme menu remains nested")
        };
        assert!(matches!(
            &theme[0],
            TrayEntry::Item(TrayItem {
                kind: TrayItemKind::Radio { checked: false, .. },
                ..
            })
        ));
        assert!(matches!(
            &theme[1],
            TrayEntry::Item(TrayItem {
                kind: TrayItemKind::Radio { checked: true, .. },
                ..
            })
        ));
    }
}
