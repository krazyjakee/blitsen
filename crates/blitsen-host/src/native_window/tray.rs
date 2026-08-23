//! Declarative system tray support owned by one native window session.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use winit::event_loop::EventLoopProxy;

#[cfg(any(target_os = "windows", target_os = "macos"))]
use super::menu::DecodedIcon;
use super::menu::{
    MenuEntry, MenuItem, MenuItemKind, MenuSignal, MenuSurface, decode_icon, parse_menu, queue,
};
use crate::{TrayAction, TrayOptions};

#[derive(Clone)]
pub(crate) struct TraySpec {
    pub(crate) icon: Vec<u8>,
    pub(crate) tooltip: Option<String>,
    pub(crate) open_on_click: bool,
    pub(crate) close_to_tray: bool,
    pub(crate) menu: Vec<MenuEntry>,
}

impl TryFrom<TrayOptions> for TraySpec {
    type Error = String;

    fn try_from(options: TrayOptions) -> Result<Self, Self::Error> {
        let menu = if let Some(menu) = options.menu {
            let (entries, has_quit) = parse_menu(menu.entries, &menu.icons, MenuSurface::Tray)?;
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
                        return MenuEntry::Separator;
                    }
                    MenuEntry::Item(MenuItem {
                        label: item
                            .label
                            .unwrap_or_else(|| action.default_label().to_owned()),
                        enabled: item.enabled,
                        accelerator: None,
                        icon: None,
                        signal: MenuSignal::Command(action),
                        kind: MenuItemKind::Action,
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

pub(crate) struct TrayController {
    pending: Arc<Mutex<VecDeque<MenuSignal>>>,
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
    menu_actions: super::menu::Bindings,
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
            menu_actions: super::menu::Bindings::new(),
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
            if self.icon.is_some() {
                return Ok(());
            }
            let id_prefix = super::menu::next_id_prefix("tray");
            let menu =
                super::menu::build_menu(&self.options.menu, &id_prefix, &mut self.menu_actions)?;
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

    /// Queues the signals for the menu events this tray's own items raised.
    ///
    /// The ids come from `menu::take_native_menu_events`, which drains muda's
    /// one channel for every menu in the process: a tray that drained it here
    /// would swallow the application menu's clicks.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    pub(crate) fn claim(&self, ids: &[String]) {
        super::menu::claim_native_menu_events(ids, &self.menu_actions, &self.pending, &self.proxy);
    }

    /// Pulls native icon events into the session's single command slot.
    pub(crate) fn poll(&self) {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        {
            use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};

            while let Ok(event) = TrayIconEvent::receiver().try_recv() {
                if matches!(
                    event,
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    }
                ) {
                    queue(MenuSignal::Click, &self.pending, &self.proxy);
                    if self.open_on_click {
                        queue(
                            MenuSignal::Command(TrayAction::Show),
                            &self.pending,
                            &self.proxy,
                        );
                    }
                }
            }
        }
    }

    pub(crate) fn take_signals(&self) -> Vec<MenuSignal> {
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
    menu: Vec<MenuEntry>,
    open_on_click: bool,
    pending: Arc<Mutex<VecDeque<MenuSignal>>>,
    proxy: EventLoopProxy,
}

#[cfg(target_os = "linux")]
fn action_id(signal: &MenuSignal) -> Option<&str> {
    match signal {
        MenuSignal::Action { id, .. } => Some(id),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn toggle_checkbox(entries: &mut [MenuEntry], id: &str) -> Option<bool> {
    for entry in entries {
        match entry {
            MenuEntry::Item(item) if action_id(&item.signal) == Some(id) => {
                if let MenuItemKind::Checkbox { checked } = &mut item.kind {
                    *checked = !*checked;
                    return Some(*checked);
                }
            }
            MenuEntry::Submenu { menu, .. } => {
                if let Some(checked) = toggle_checkbox(menu, id) {
                    return Some(checked);
                }
            }
            MenuEntry::Separator | MenuEntry::Role(_) | MenuEntry::Item(_) => {}
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn select_radio(entries: &mut [MenuEntry], id: &str) -> bool {
    let group = entries.iter().find_map(|entry| match entry {
        MenuEntry::Item(item) if action_id(&item.signal) == Some(id) => match &item.kind {
            MenuItemKind::Radio { group, .. } => Some(group.clone()),
            _ => None,
        },
        _ => None,
    });
    if let Some(group) = group {
        for entry in entries {
            if let MenuEntry::Item(item) = entry
                && let MenuItemKind::Radio {
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
        if let MenuEntry::Submenu { menu, .. } = entry
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
fn linux_menu(entries: &[MenuEntry]) -> Vec<ksni::MenuItem<LinuxTray>> {
    use ksni::menu::{CheckmarkItem, RadioGroup, RadioItem, StandardItem, SubMenu};

    let mut menu = Vec::with_capacity(entries.len());
    let mut index = 0;
    while index < entries.len() {
        match &entries[index] {
            MenuEntry::Separator => menu.push(ksni::MenuItem::Separator),
            // Roles are the application menu's, and `MenuSurface::Tray`
            // refuses one before a tray tree ever reaches here.
            MenuEntry::Role(_) => {}
            MenuEntry::Submenu {
                label,
                enabled,
                icon,
                menu: children,
                role: _,
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
            MenuEntry::Item(item) => match &item.kind {
                MenuItemKind::Action => {
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
                MenuItemKind::Checkbox { checked } => {
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
                MenuItemKind::Radio { group, .. } => {
                    let group = group.clone();
                    let start = index;
                    let mut end = index;
                    let mut selected = 0;
                    let mut signals = Vec::new();
                    let mut options = Vec::new();
                    while let Some(MenuEntry::Item(radio)) = entries.get(end) {
                        let MenuItemKind::Radio {
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
        queue(MenuSignal::Click, &self.pending, &self.proxy);
        if self.open_on_click {
            queue(
                MenuSignal::Command(TrayAction::Show),
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

    fn event_item(id: &str, kind: MenuItemKind) -> MenuEntry {
        let checked = match &kind {
            MenuItemKind::Checkbox { checked } | MenuItemKind::Radio { checked, .. } => {
                Some(*checked)
            }
            MenuItemKind::Action => None,
        };
        MenuEntry::Item(MenuItem {
            label: id.into(),
            enabled: true,
            accelerator: None,
            icon: None,
            signal: MenuSignal::Action {
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
            MenuEntry::Item(MenuItem {
                signal: MenuSignal::Command(TrayAction::Show),
                ..
            })
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn checkbox_and_nested_radio_state_changes_before_the_event_is_queued() {
        let mut menu = vec![
            event_item("launch", MenuItemKind::Checkbox { checked: false }),
            MenuEntry::Submenu {
                label: "Theme".into(),
                role: None,
                enabled: true,
                icon: None,
                menu: vec![
                    event_item(
                        "light",
                        MenuItemKind::Radio {
                            group: "theme".into(),
                            checked: true,
                        },
                    ),
                    event_item(
                        "dark",
                        MenuItemKind::Radio {
                            group: "theme".into(),
                            checked: false,
                        },
                    ),
                ],
            },
        ];
        assert_eq!(toggle_checkbox(&mut menu, "launch"), Some(true));
        assert!(select_radio(&mut menu, "dark"));
        let MenuEntry::Submenu { menu: theme, .. } = &menu[1] else {
            panic!("the theme menu remains nested")
        };
        assert!(matches!(
            &theme[0],
            MenuEntry::Item(MenuItem {
                kind: MenuItemKind::Radio { checked: false, .. },
                ..
            })
        ));
        assert!(matches!(
            &theme[1],
            MenuEntry::Item(MenuItem {
                kind: MenuItemKind::Radio { checked: true, .. },
                ..
            })
        ));
    }
}
