//! Laid-out geometry: box metrics, hit testing, scrolling and surfaces.

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
        "layoutMetrics" => {
            let forced = dom.layout_is_dirty();
            let node = handle(runtime, arguments, 0)?;
            let snapshot = dom.flush_layout().map_err(dom_error)?;
            let metrics = dom.layout_metrics(node, snapshot).map_err(dom_error)?;
            Ok(json!({
                "forced": forced,
                "x": metrics.rect.x,
                "y": metrics.rect.y,
                "width": metrics.rect.width,
                "height": metrics.rect.height,
                "contentX": metrics.content_rect.x,
                "contentY": metrics.content_rect.y,
                "contentWidth": metrics.content_rect.width,
                "contentHeight": metrics.content_rect.height,
                "offsetWidth": metrics.offset_width,
                "offsetHeight": metrics.offset_height,
                "clientWidth": metrics.client_width,
                "clientHeight": metrics.client_height,
                "scrollLeft": metrics.scroll_left,
                "scrollTop": metrics.scroll_top,
            }))
        }
        "clientRects" => {
            let forced = dom.layout_is_dirty();
            let node = handle(runtime, arguments, 0)?;
            let snapshot = dom.flush_layout().map_err(dom_error)?;
            let rects = dom.client_rects(node, snapshot).map_err(dom_error)?;
            Ok(json!({ "forced": forced, "rects": serialize_rects(rects) }))
        }
        // A run of characters inside one text node, offsets counted the way a
        // `Range` counts them: in UTF-16 code units, which is what a JavaScript
        // string is indexed by.
        "textRects" => {
            let forced = dom.layout_is_dirty();
            let node = handle(runtime, arguments, 0)?;
            let start = bridge_offset(arguments, 1)?;
            let end = bridge_offset(arguments, 2)?;
            let snapshot = dom.flush_layout().map_err(dom_error)?;
            let rects = dom
                .text_rects(node, start, end, snapshot)
                .map_err(dom_error)?;
            Ok(json!({ "forced": forced, "rects": serialize_rects(rects) }))
        }
        "caretPosition" => {
            let x = bridge_arg(arguments, 0, "caret x")?
                .parse::<f32>()
                .map_err(|_| JsError::new("invalid caret x"))?;
            let y = bridge_arg(arguments, 1, "caret y")?
                .parse::<f32>()
                .map_err(|_| JsError::new("invalid caret y"))?;
            let forced = dom.layout_is_dirty();
            let snapshot = dom.flush_layout().map_err(dom_error)?;
            let caret = dom.caret_position(x, y, snapshot).map_err(dom_error)?;
            Ok(json!({
                "forced": forced,
                "node": caret.map(|caret| DomRuntime::serialize_handle(caret.node)),
                "offset": caret.map_or(0, |caret| caret.offset),
            }))
        }
        "imageState" => {
            let forced = dom.layout_is_dirty();
            let node = handle(runtime, arguments, 0)?;
            let snapshot = dom.flush_layout().map_err(dom_error)?;
            let state = dom.image_state(node, snapshot).map_err(dom_error)?;
            Ok(json!({
                "forced": forced,
                "naturalWidth": state.natural_width,
                "naturalHeight": state.natural_height,
                "complete": state.complete,
                "errored": state.errored,
            }))
        }
        "linkState" => {
            let forced = dom.layout_is_dirty();
            let node = handle(runtime, arguments, 0)?;
            let snapshot = dom.flush_layout().map_err(dom_error)?;
            let state = dom.link_state(node, snapshot).map_err(dom_error)?;
            Ok(json!({
                "forced": forced,
                "pending": state.pending,
                "complete": state.complete,
                "errored": state.errored,
            }))
        }
        "viewportSurface" => {
            let forced = dom.layout_is_dirty();
            let node = handle(runtime, arguments, 0)?;
            let snapshot = dom.flush_layout().map_err(dom_error)?;
            let surface = dom
                .native_viewport_surface(node, snapshot)
                .map_err(dom_error)?;
            Ok(json!({
                "forced": forced,
                "width": surface.width,
                "height": surface.height,
                "devicePixelRatio": surface.device_pixel_ratio,
                "generation": surface.generation,
                "byteLength": surface.byte_length(),
            }))
        }
        "hitTest" => {
            let x = bridge_arg(arguments, 0, "hit-test x")?
                .parse::<f32>()
                .map_err(|_| JsError::new("invalid hit-test x"))?;
            let y = bridge_arg(arguments, 1, "hit-test y")?
                .parse::<f32>()
                .map_err(|_| JsError::new("invalid hit-test y"))?;
            let snapshot = dom.flush_layout().map_err(dom_error)?;
            Ok(match dom.hit_test(x, y, snapshot).map_err(dom_error)? {
                None => Value::Null,
                Some(hit) => json!({
                    "target": DomRuntime::serialize_handle(hit.target),
                    "path": hit.path.into_iter()
                        .map(DomRuntime::serialize_handle)
                        .collect::<Vec<_>>(),
                    "offsetX": hit.offset_x,
                    "offsetY": hit.offset_y,
                }),
            })
        }
        "setScroll" => {
            let forced = dom.layout_is_dirty();
            let node = handle(runtime, arguments, 0)?;
            let axis = bridge_arg(arguments, 1, "scroll axis")?;
            let value = bridge_arg(arguments, 2, "scroll value")?
                .parse::<f64>()
                .map_err(|_| JsError::new("invalid scroll value"))?;
            let snapshot = dom.flush_layout().map_err(dom_error)?;
            match axis {
                "left" => dom
                    .set_scroll_offset(node, Some(value), None, snapshot)
                    .map_err(dom_error)?,
                "top" => dom
                    .set_scroll_offset(node, None, Some(value), snapshot)
                    .map_err(dom_error)?,
                _ => return Err(JsError::new("invalid scroll axis")),
            }
            Ok(json!({ "forced": forced }))
        }
        _ => return Ok(None),
    }?;
    Ok(Some(value))
}

/// Reads a text offset, which JavaScript has already clamped to the node.
fn bridge_offset(arguments: &[String], index: usize) -> Result<u32, JsError> {
    bridge_arg(arguments, index, "text offset")?
        .parse::<u32>()
        .map_err(|_| JsError::new("invalid text offset"))
}

fn serialize_rects(rects: Vec<Rect>) -> Value {
    rects
        .into_iter()
        .map(|rect| {
            json!({
                "x": rect.x,
                "y": rect.y,
                "width": rect.width,
                "height": rect.height,
            })
        })
        .collect()
}
