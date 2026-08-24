use blitsen_js::{JsEngine, JsError};

use super::super::{input, json_value};

pub(super) fn install<E: JsEngine + 'static>(engine: &mut E) -> Result<(), JsError> {
    engine.define_global_function(
        "__blitsenNativeInputSnapshot",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            json_value(&mut engine, &input::snapshot())
        }),
    )
}
