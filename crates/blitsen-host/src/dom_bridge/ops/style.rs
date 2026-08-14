//! Inline style, stylesheets, computed style, media queries and animation.

// These are slices of what was one `match`, so they share the helpers and
// imports their parent gathers rather than restating them.
use super::*;

pub(super) fn dispatch(
    runtime: &DomRuntime,
    dom: &mut BlitzDom,
    operation: &str,
    arguments: &[String],
) -> Answer {
    let value = match operation {
        "styleGet" => Ok(Value::String(
            dom.inline_style(
                handle(runtime, arguments, 0)?,
                bridge_arg(arguments, 1, "property")?,
            )
            .map_err(dom_error)?
            .unwrap_or_default(),
        )),
        "styleSet" => {
            let node = handle(runtime, arguments, 0)?;
            dom.set_inline_style(
                node,
                bridge_arg(arguments, 1, "property")?,
                bridge_arg(arguments, 2, "value")?,
            )
            .map_err(dom_error)?;
            Ok(Value::Null)
        }
        "styleRemove" => Ok(Value::String(
            dom.remove_inline_style(
                handle(runtime, arguments, 0)?,
                bridge_arg(arguments, 1, "property")?,
            )
            .map_err(dom_error)?
            .unwrap_or_default(),
        )),
        "styleText" => Ok(Value::String(
            dom.inline_style_text(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?,
        )),
        "setStyleText" => {
            let node = handle(runtime, arguments, 0)?;
            dom.set_inline_style_text(node, bridge_arg(arguments, 1, "CSS text")?)
                .map_err(dom_error)?;
            Ok(Value::Null)
        }
        "styleGetJs" => Ok(Value::String(
            dom.inline_style(
                handle(runtime, arguments, 0)?,
                &js_property_to_css(bridge_arg(arguments, 1, "property")?),
            )
            .map_err(dom_error)?
            .unwrap_or_default(),
        )),
        "styleSetJs" => {
            let node = handle(runtime, arguments, 0)?;
            dom.set_inline_style(
                node,
                &js_property_to_css(bridge_arg(arguments, 1, "property")?),
                bridge_arg(arguments, 2, "value")?,
            )
            .map_err(dom_error)?;
            Ok(Value::Null)
        }
        // The CSSOM stylesheet surface. A sheet has no identity of its own here:
        // it is the `<style>` element that owns it, whose text the cascade is
        // already parsing, so a rule inserted through these operations is in the
        // same stylesheet set Stylo cascades from and cannot be a shadow copy.
        "styleSheets" => Ok(serialized_all(dom.style_sheets().map_err(dom_error)?)),
        "sheetRules" => Ok(json!(
            dom.sheet_rules(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?
        )),
        "insertSheetRule" => {
            let node = handle(runtime, arguments, 0)?;
            let rule = bridge_arg(arguments, 1, "CSS rule")?.to_owned();
            let index = bridge_index(arguments, 2)?;
            dom.insert_sheet_rule(node, &rule, index)
                .map_err(dom_error)?;
            Ok(Value::Null)
        }
        "deleteSheetRule" => {
            let node = handle(runtime, arguments, 0)?;
            let index = bridge_index(arguments, 1)?;
            dom.delete_sheet_rule(node, index).map_err(dom_error)?;
            Ok(Value::Null)
        }
        // The frame's own timestamp, in the seconds the cascade counts animation
        // time in. Nothing below this call reads a clock, so a replayed frame
        // sequence animates exactly as the recorded one did.
        "setAnimationTime" => {
            let milliseconds = bridge_arg(arguments, 0, "timestamp")?
                .parse::<f64>()
                .map_err(|_| JsError::new("invalid animation timestamp"))?;
            dom.set_animation_time(milliseconds / 1_000.0);
            Ok(Value::Null)
        }
        "isAnimating" => Ok(Value::Bool(dom.is_animating())),
        // Layout-dependent like the geometry reads, and gated the same way: the
        // resolved value of a box property is the used value, which is only
        // knowable after style and layout have settled.
        "computedStyle" | "computedStyleJs" => {
            let forced = dom.layout_is_dirty();
            let node = handle(runtime, arguments, 0)?;
            let property = bridge_arg(arguments, 1, "property")?;
            let property = if operation == "computedStyleJs" {
                js_property_to_css(property)
            } else {
                property.to_owned()
            };
            let snapshot = dom.flush_layout().map_err(dom_error)?;
            let value = dom
                .resolved_style(node, &property, snapshot)
                .map_err(dom_error)?;
            Ok(json!({ "forced": forced, "value": value }))
        }
        "matchMedia" => {
            let query = dom
                .media_query(bridge_arg(arguments, 0, "media query")?)
                .map_err(dom_error)?;
            Ok(json!({ "media": query.media, "matches": query.matches }))
        }
        _ => return Ok(None),
    }?;
    Ok(Some(value))
}
