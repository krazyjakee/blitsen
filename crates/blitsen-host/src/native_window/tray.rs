//! Declarative system tray support owned by one native window session.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use image::GenericImageView;
use winit::event_loop::EventLoopProxy;

use crate::{TrayAction, TrayMenuItem, TrayOptions};

const NONE: u8 = 0;
const SHOW: u8 = 1;
const HIDE: u8 = 2;
const QUIT: u8 = 3;

fn command(action: TrayAction) -> u8 {
    match action {
        TrayAction::Show => SHOW,
        TrayAction::Hide => HIDE,
        TrayAction::Quit => QUIT,
        TrayAction::Separator => NONE,
    }
}

fn queue(action: TrayAction, pending: &AtomicU8, proxy: &EventLoopProxy) {
    let action = command(action);
    if action != NONE {
        pending.store(action, Ordering::Release);
        proxy.wake_up();
    }
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

pub(crate) struct TrayController {
    pending: Arc<AtomicU8>,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    proxy: EventLoopProxy,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    open_on_click: bool,
    close_to_tray: bool,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    options: TrayOptions,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    decoded: Option<DecodedIcon>,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    icon: Option<tray_icon::TrayIcon>,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    menu_actions: std::collections::HashMap<String, TrayAction>,
    #[cfg(target_os = "linux")]
    _service: ksni::Handle<LinuxTray>,
}

impl TrayController {
    pub(crate) fn new(
        options: TrayOptions,
        application_title: &str,
        proxy: EventLoopProxy,
        runtime: &tokio::runtime::Runtime,
    ) -> Result<Self, String> {
        let decoded = decode_icon(&options.icon)?;
        let pending = Arc::new(AtomicU8::new(NONE));

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
                menu: options.context_menu.clone(),
                open_on_click: options.open_on_click,
                pending: Arc::clone(&pending),
                proxy: proxy.clone(),
            };
            runtime
                .block_on(tray.spawn())
                .map_err(|error| format!("could not create tray icon: {error}"))?
        };

        #[cfg(not(target_os = "linux"))]
        let _ = runtime;
        #[cfg(target_os = "android")]
        let _ = (&decoded, &proxy, application_title);

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
            use tray_icon::menu::{Menu, MenuItem, PredefinedMenuItem};

            if self.icon.is_some() {
                return Ok(());
            }
            let menu = Menu::new();
            for (index, item) in self.options.context_menu.iter().enumerate() {
                if item.action == TrayAction::Separator {
                    menu.append(&PredefinedMenuItem::separator())
                        .map_err(|error| format!("could not create tray menu: {error}"))?;
                    continue;
                }
                let id = format!("blitsen-tray-{index}");
                let label = item
                    .label
                    .as_deref()
                    .unwrap_or_else(|| item.action.default_label());
                let entry = MenuItem::with_id(&id, label, item.enabled, None);
                menu.append(&entry)
                    .map_err(|error| format!("could not create tray menu: {error}"))?;
                self.menu_actions.insert(id, item.action);
            }
            let decoded = self.decoded.take().expect("a tray icon is decoded once");
            let icon = tray_icon::Icon::from_rgba(decoded.rgba, decoded.width, decoded.height)
                .map_err(|error| format!("could not create tray icon: {error}"))?;
            let mut builder = tray_icon::TrayIconBuilder::new()
                .with_id("blitsen")
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
                if let Some(action) = self.menu_actions.get(event.id().as_ref()) {
                    queue(*action, &self.pending, &self.proxy);
                }
            }
            while let Ok(event) = TrayIconEvent::receiver().try_recv() {
                if self.open_on_click
                    && matches!(
                        event,
                        TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        }
                    )
                {
                    queue(TrayAction::Show, &self.pending, &self.proxy);
                }
            }
        }
    }

    pub(crate) fn take_action(&self) -> Option<TrayAction> {
        match self.pending.swap(NONE, Ordering::AcqRel) {
            SHOW => Some(TrayAction::Show),
            HIDE => Some(TrayAction::Hide),
            QUIT => Some(TrayAction::Quit),
            _ => None,
        }
    }
}

#[cfg(target_os = "linux")]
struct LinuxTray {
    title: String,
    tooltip: Option<String>,
    icon: ksni::Icon,
    menu: Vec<TrayMenuItem>,
    open_on_click: bool,
    pending: Arc<AtomicU8>,
    proxy: EventLoopProxy,
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
        if self.open_on_click {
            queue(TrayAction::Show, &self.pending, &self.proxy);
        }
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        self.menu
            .iter()
            .map(|item| {
                if item.action == TrayAction::Separator {
                    return ksni::MenuItem::Separator;
                }
                let action = item.action;
                let pending = Arc::clone(&self.pending);
                let proxy = self.proxy.clone();
                ksni::menu::StandardItem {
                    label: item
                        .label
                        .clone()
                        .unwrap_or_else(|| action.default_label().to_owned()),
                    enabled: item.enabled,
                    activate: Box::new(move |_| queue(action, &pending, &proxy)),
                    ..Default::default()
                }
                .into()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_actions_use_distinct_commands() {
        assert_eq!(command(TrayAction::Show), SHOW);
        assert_eq!(command(TrayAction::Hide), HIDE);
        assert_eq!(command(TrayAction::Quit), QUIT);
        assert_eq!(command(TrayAction::Separator), NONE);
    }
}
