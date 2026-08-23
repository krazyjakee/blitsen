//! Synchronous host functions behind the standard `localStorage` object.

use blitsen_js::{JsEngine, JsError};

use super::{argument, json_value};
use crate::storage::LocalStorage;

pub(super) fn install<E: JsEngine + 'static>(
    engine: &mut E,
    storage: Option<LocalStorage>,
) -> Result<(), JsError> {
    let Some(storage) = storage else {
        return Ok(());
    };

    let keys = storage.clone();
    engine.define_global_function(
        "__blitsenStorageKeys",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let keys = keys.keys().map_err(JsError::new)?;
            json_value(&mut engine, &serde_json::json!(keys))
        }),
    )?;

    let reader = storage.clone();
    engine.define_global_function(
        "__blitsenStorageGet",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let key = argument(&mut engine, &call, 0, "localStorage key")?;
            match reader.get(&key).map_err(JsError::new)? {
                Some(value) => engine.string(&value),
                None => Ok(engine.null()),
            }
        }),
    )?;

    let writer = storage.clone();
    engine.define_global_function(
        "__blitsenStorageSet",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let key = argument(&mut engine, &call, 0, "localStorage key")?;
            let value = argument(&mut engine, &call, 1, "localStorage value")?;
            writer.set(&key, &value).map_err(JsError::new)?;
            Ok(call.this)
        }),
    )?;

    let remover = storage.clone();
    engine.define_global_function(
        "__blitsenStorageRemove",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let key = argument(&mut engine, &call, 0, "localStorage key")?;
            remover.remove(&key).map_err(JsError::new)?;
            Ok(call.this)
        }),
    )?;

    engine.define_global_function(
        "__blitsenStorageClear",
        Box::new(move |call| {
            storage.clear().map_err(JsError::new)?;
            Ok(call.this)
        }),
    )
}
