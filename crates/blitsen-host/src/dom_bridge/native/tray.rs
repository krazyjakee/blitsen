#[cfg(not(target_os = "android"))]
use blitsen_js::TypedArrayKind;
use blitsen_js::{JsEngine, JsError};
#[cfg(not(target_os = "android"))]
use serde_json::json;

#[cfg(not(target_os = "android"))]
use super::super::{argument, json_value, tray};

#[cfg(not(target_os = "android"))]
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrayBridgeOptions {
    tooltip: Option<String>,
    open_on_click: bool,
    close_to_tray: bool,
    menu: Vec<TrayBridgeItem>,
}

#[cfg(not(target_os = "android"))]
type TrayBridgeItem = crate::MenuDefinition;

#[cfg(not(target_os = "android"))]
fn parse_tray_menu(
    raw: Vec<TrayBridgeItem>,
    icons: &[Vec<u8>],
) -> Result<(Vec<crate::native_window::menu::MenuEntry>, bool), JsError> {
    crate::native_window::menu::parse_menu(
        raw,
        icons,
        crate::native_window::menu::MenuSurface::Tray,
    )
    .map_err(JsError::new)
}

#[cfg(not(target_os = "android"))]
pub(super) fn install<E: JsEngine + 'static>(engine: &mut E) -> Result<(), JsError> {
    use crate::native_window::tray::TraySpec;

    engine.define_global_function(
        "__blitsenNativeTrayConfigure",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let options: TrayBridgeOptions =
                serde_json::from_str(&argument(&mut engine, &call, 0, "tray options")?)
                    .map_err(|error| JsError::new(format!("malformed tray options: {error}")))?;
            let icon = call
                .arguments
                .get(1)
                .ok_or_else(|| JsError::new("missing tray icon bytes"))?;
            let icon = engine.to_typed_array(icon)?;
            if !matches!(
                icon.kind,
                TypedArrayKind::Uint8 | TypedArrayKind::Uint8Clamped
            ) {
                return Err(JsError::new(
                    "tray icon must be a Uint8Array or Uint8ClampedArray",
                ));
            }

            let menu_icons = call
                .arguments
                .iter()
                .skip(2)
                .map(|value| {
                    let icon = engine.to_typed_array(value)?;
                    if !matches!(
                        icon.kind,
                        TypedArrayKind::Uint8 | TypedArrayKind::Uint8Clamped
                    ) {
                        return Err(JsError::new(
                            "tray menu icons must be Uint8Array or Uint8ClampedArray values",
                        ));
                    }
                    Ok(icon.bytes)
                })
                .collect::<Result<Vec<_>, JsError>>()?;
            let (menu, has_quit) = parse_tray_menu(options.menu, &menu_icons)?;
            if options.close_to_tray && !has_quit {
                return Err(JsError::new(
                    "closeToTray requires a quit action in the tray menu",
                ));
            }
            let id = tray::configure(TraySpec {
                icon: icon.bytes,
                tooltip: options.tooltip,
                open_on_click: options.open_on_click,
                close_to_tray: options.close_to_tray,
                menu,
            });
            engine.string(&id.to_string())
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeTrayRemove",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            engine.string(&tray::remove().to_string())
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeTrayPending",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            Ok(engine.boolean(tray::pending()))
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeTrayTake",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            json_value(&mut engine, &json!(tray::take_messages()))
        }),
    )
}

#[cfg(target_os = "android")]
pub(super) fn install<E: JsEngine + 'static>(_engine: &mut E) -> Result<(), JsError> {
    Ok(())
}

#[cfg(all(test, not(target_os = "android")))]
mod tests {
    use super::*;
    use crate::native_window::menu::{MenuEntry, MenuItemKind, MenuSignal};

    fn action(id: &str) -> TrayBridgeItem {
        TrayBridgeItem {
            id: Some(id.into()),
            label: Some(id.into()),
            ..Default::default()
        }
    }

    fn radio(id: &str, group: &str, checked: bool) -> TrayBridgeItem {
        TrayBridgeItem {
            kind: Some("radio".into()),
            id: Some(id.into()),
            label: Some(id.into()),
            group: Some(group.into()),
            checked: Some(checked),
            ..Default::default()
        }
    }

    #[test]
    fn nested_checkable_menu_keeps_public_identity_and_state() {
        let raw = vec![
            action("open"),
            TrayBridgeItem {
                kind: Some("submenu".into()),
                label: Some("Theme".into()),
                menu: Some(vec![
                    radio("light", "theme", true),
                    radio("dark", "theme", false),
                ]),
                ..Default::default()
            },
            TrayBridgeItem {
                kind: Some("checkbox".into()),
                id: Some("launch".into()),
                label: Some("Launch".into()),
                checked: Some(true),
                ..Default::default()
            },
        ];
        let (menu, has_quit) = parse_tray_menu(raw, &[]).expect("the tree is valid");
        assert!(!has_quit);
        let MenuEntry::Submenu { menu: theme, .. } = &menu[1] else {
            panic!("the second entry is the theme submenu")
        };
        let MenuEntry::Item(dark) = &theme[1] else {
            panic!("the second theme entry is an item")
        };
        assert_eq!(
            dark.signal,
            MenuSignal::Action {
                id: "dark".into(),
                checked: Some(false),
            }
        );
        assert_eq!(
            dark.kind,
            MenuItemKind::Radio {
                group: "theme".into(),
                checked: false,
            }
        );
    }

    #[test]
    fn ids_are_unique_across_submenus() {
        let raw = vec![
            action("open"),
            TrayBridgeItem {
                kind: Some("submenu".into()),
                label: Some("More".into()),
                menu: Some(vec![action("open")]),
                ..Default::default()
            },
        ];
        assert!(parse_tray_menu(raw, &[]).is_err());
    }

    #[test]
    fn radio_groups_are_consecutive_and_have_one_selection() {
        assert!(
            parse_tray_menu(
                vec![
                    radio("light", "theme", false),
                    radio("dark", "theme", false)
                ],
                &[],
            )
            .is_err()
        );
        assert!(
            parse_tray_menu(
                vec![
                    radio("light", "theme", true),
                    action("open"),
                    radio("dark", "theme", false),
                ],
                &[],
            )
            .is_err()
        );
    }

    #[test]
    fn accelerator_has_modifiers_before_one_key() {
        let mut valid = action("open");
        valid.accelerator = Some("CmdOrCtrl+Shift+KeyO".into());
        assert!(parse_tray_menu(vec![valid], &[]).is_ok());

        let mut invalid = action("open");
        invalid.accelerator = Some("KeyO+Control".into());
        assert!(parse_tray_menu(vec![invalid], &[]).is_err());
    }
}
