use blitsen_js::{JsEngine, JsError};
#[cfg(any(target_os = "windows", target_os = "macos"))]
use serde_json::json;

#[cfg(any(target_os = "windows", target_os = "macos"))]
use super::super::{argument, json_value, menu};

// The application menu, installed only where a platform has one to install.
// macOS has NSApp's main menu, and Windows has a per-window menu bar; on Linux
// muda's only backend is a `gtk::MenuBar` added to a `gtk::Window`, and Blitsen
// windows are winit's. That is the whole argument, and `native-modules.mjs`
// carries it: a menu that appeared here would be a tray menu wearing the name.
#[cfg(any(target_os = "windows", target_os = "macos"))]
pub(super) fn install<E: JsEngine + 'static>(engine: &mut E) -> Result<(), JsError> {
    engine.define_global_function(
        "__blitsenNativeMenuConfigure",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let entries: Vec<crate::MenuDefinition> =
                serde_json::from_str(&argument(&mut engine, &call, 0, "menu entries")?).map_err(
                    |error| JsError::new(format!("malformed application menu: {error}")),
                )?;
            // Parsed here rather than when the request is applied, so a tree
            // the platform cannot install is a rejected promise naming what is
            // wrong with it rather than a menu that half appeared.
            crate::native_window::menu::parse_menu(
                entries.clone(),
                &[],
                crate::native_window::menu::MenuSurface::Application,
            )
            .map_err(JsError::new)?;
            engine.string(&menu::configure(entries).to_string())
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeMenuRemove",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            engine.string(&menu::remove().to_string())
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeMenuPending",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            Ok(engine.boolean(menu::pending()))
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeMenuTake",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            json_value(&mut engine, &json!(menu::take_messages()))
        }),
    )
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub(super) fn install<E: JsEngine + 'static>(_engine: &mut E) -> Result<(), JsError> {
    Ok(())
}
