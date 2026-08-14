//! The caret and the selection inside `<input>` and `<textarea>`, and the
//! renderer's idea of which node has focus.

// These are slices of what was one `match`, so they share the helpers and
// imports their parent gathers rather than restating them.
use super::*;

/// Decodes a caret movement from the name the bootstrap sends.
fn motion(name: &str) -> Result<TextMotion, JsError> {
    Ok(match name {
        "left" => TextMotion::Left,
        "right" => TextMotion::Right,
        "up" => TextMotion::Up,
        "down" => TextMotion::Down,
        "wordLeft" => TextMotion::WordLeft,
        "wordRight" => TextMotion::WordRight,
        "lineStart" => TextMotion::LineStart,
        "lineEnd" => TextMotion::LineEnd,
        "textStart" => TextMotion::TextStart,
        "textEnd" => TextMotion::TextEnd,
        _ => return Err(JsError::new(format!("unknown caret motion: {name}"))),
    })
}

/// Decodes an editing operation, which is the mutation behind an `inputType`.
fn edit<'a>(name: &str, data: &'a str) -> Result<TextEdit<'a>, JsError> {
    Ok(match name {
        "insert" => TextEdit::Insert(data),
        "deleteBackward" => TextEdit::DeleteBackward,
        "deleteForward" => TextEdit::DeleteForward,
        "deleteWordBackward" => TextEdit::DeleteWordBackward,
        "deleteWordForward" => TextEdit::DeleteWordForward,
        _ => return Err(JsError::new(format!("unknown text edit: {name}"))),
    })
}

/// Reads an offset the bootstrap has already coerced to a non-negative integer.
fn offset(arguments: &[String], index: usize) -> Result<u32, JsError> {
    bridge_arg(arguments, index, "selection offset")?
        .parse::<u32>()
        .map_err(|_| JsError::new("invalid selection offset"))
}

/// Reads a CSS-pixel coordinate inside a control's border box.
fn coordinate(arguments: &[String], index: usize) -> Result<f32, JsError> {
    bridge_arg(arguments, index, "caret coordinate")?
        .parse::<f32>()
        .map_err(|_| JsError::new("invalid caret coordinate"))
}

pub(super) fn dispatch(
    runtime: &DomRuntime,
    dom: &mut BlitzDom,
    operation: &str,
    arguments: &[String],
) -> Answer {
    let value = match operation {
        "formSelection" => {
            let selection = dom
                .form_selection(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?;
            Ok(json!({
                "start": selection.start,
                "end": selection.end,
                "direction": selection.direction.as_str(),
            }))
        }
        "setFormSelection" => {
            let node = handle(runtime, arguments, 0)?;
            let start = offset(arguments, 1)?;
            let end = offset(arguments, 2)?;
            dom.set_form_selection(
                node,
                TextSelection {
                    start: start.min(end),
                    end,
                    direction: SelectionDirection::from_name(bridge_arg(
                        arguments,
                        3,
                        "selection direction",
                    )?),
                },
            )
            .map_err(dom_error)?;
            Ok(Value::Null)
        }
        "moveFormSelection" => {
            let node = handle(runtime, arguments, 0)?;
            let motion = motion(bridge_arg(arguments, 1, "caret motion")?)?;
            let extend = bridge_arg(arguments, 2, "selection extension")? == "true";
            Ok(Value::Bool(
                dom.move_form_selection(node, motion, extend)
                    .map_err(dom_error)?,
            ))
        }
        // The offsets are the ones a mouse event already carries: CSS pixels
        // from the control's top-left corner. Everything between them and a
        // character index — padding, scroll, the shaped text itself — is the
        // renderer's, so the point crosses the boundary unresolved.
        "moveFormCaret" => {
            let node = handle(runtime, arguments, 0)?;
            let offset_x = coordinate(arguments, 1)?;
            let offset_y = coordinate(arguments, 2)?;
            let extend = bridge_arg(arguments, 3, "selection extension")? == "true";
            Ok(Value::Bool(
                dom.move_form_caret_to_point(node, offset_x, offset_y, extend)
                    .map_err(dom_error)?,
            ))
        }
        "editFormValue" => {
            let node = handle(runtime, arguments, 0)?;
            let data = bridge_arg(arguments, 2, "inserted text")?;
            let edit = edit(bridge_arg(arguments, 1, "text edit")?, data)?;
            Ok(Value::Bool(
                dom.edit_form_value(node, edit).map_err(dom_error)?,
            ))
        }
        // An empty handle is no focus rather than a missing argument: the
        // bootstrap sends it when focus lands on the body, which is where HTML
        // parks it when nothing focusable holds it and is not a node anything
        // should paint a focus ring or a caret on.
        "setFocusedNode" => {
            let node = match bridge_arg(arguments, 0, "node handle")? {
                "" => None,
                _ => Some(handle(runtime, arguments, 0)?),
            };
            dom.set_focused(node).map_err(dom_error)?;
            Ok(Value::Null)
        }
        _ => return Ok(None),
    }?;
    Ok(Some(value))
}
