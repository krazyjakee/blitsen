//! Finding nodes: selectors, class and id lookup, identity and containment.

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
        "kind" => Ok(Value::String(
            match dom
                .node_kind(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?
            {
                NodeKind::Element => "element",
                NodeKind::Document => "document",
                NodeKind::Text => "text",
                NodeKind::Comment => "comment",
                NodeKind::Fragment => "fragment",
            }
            .into(),
        )),
        "tagName" => Ok(Value::String(
            dom.element_name(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?
                .local,
        )),
        "namespaceUri" => Ok(namespace_uri(
            &dom.element_name(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?
                .namespace,
        )
        .map(|uri| Value::String(uri.to_owned()))
        .unwrap_or(Value::Null)),
        "querySelector" => Ok(serialized(
            dom.query_selector(dom.document(), bridge_arg(arguments, 0, "selector")?)
                .map_err(dom_error)?,
        )),
        "querySelectorAll" => Ok(json!(
            dom.query_selector_all(dom.document(), bridge_arg(arguments, 0, "selector")?)
                .map_err(dom_error)?
                .into_iter()
                .map(DomRuntime::serialize_handle)
                .collect::<Vec<_>>()
        )),
        "querySelectorIn" => {
            let node = handle(runtime, arguments, 0)?;
            Ok(serialized(
                dom.query_selector(node, bridge_arg(arguments, 1, "selector")?)
                    .map_err(dom_error)?,
            ))
        }
        "querySelectorAllIn" => {
            let node = handle(runtime, arguments, 0)?;
            Ok(json!(
                dom.query_selector_all(node, bridge_arg(arguments, 1, "selector")?)
                    .map_err(dom_error)?
                    .into_iter()
                    .map(DomRuntime::serialize_handle)
                    .collect::<Vec<_>>()
            ))
        }
        "elementsByClassName" => {
            let root = dom.document();
            Ok(json!(
                elements_by_class_name(dom, root, bridge_arg(arguments, 0, "class names")?)?
                    .into_iter()
                    .map(DomRuntime::serialize_handle)
                    .collect::<Vec<_>>()
            ))
        }
        "elementsByClassNameIn" => {
            let node = handle(runtime, arguments, 0)?;
            Ok(json!(
                elements_by_class_name(dom, node, bridge_arg(arguments, 1, "class names")?)?
                    .into_iter()
                    .map(DomRuntime::serialize_handle)
                    .collect::<Vec<_>>()
            ))
        }
        // Selector matching against a single element is the renderer's own, not
        // an emulation over `querySelectorAll`: a detached element has no scope
        // to search, and an ancestor walk would rescan the subtree per level.
        "matches" => {
            let node = handle(runtime, arguments, 0)?;
            dom.node_kind(node).map_err(dom_error)?;
            Ok(Value::Bool(
                dom.document_ref()
                    .matches_selector(node, bridge_arg(arguments, 1, "selector")?)
                    .map_err(|error| dom_error(DomError::Syntax(format!("{error:?}"))))?,
            ))
        }
        "closest" => {
            let node = handle(runtime, arguments, 0)?;
            dom.node_kind(node).map_err(dom_error)?;
            Ok(serialized(
                dom.document_ref()
                    .closest(node, bridge_arg(arguments, 1, "selector")?)
                    .map_err(|error| dom_error(DomError::Syntax(format!("{error:?}"))))?,
            ))
        }
        "contains" => {
            let node = handle(runtime, arguments, 0)?;
            let mut candidate = Some(handle(runtime, arguments, 1)?);
            while let Some(current) = candidate {
                if current == node {
                    return Ok(Some(Value::Bool(true)));
                }
                candidate = dom.parent(current).map_err(dom_error)?;
            }
            Ok(Value::Bool(false))
        }
        "getElementById" => Ok(serialized(
            dom.get_element_by_id(bridge_arg(arguments, 0, "id")?)
                .map_err(dom_error)?,
        )),
        _ => return Ok(None),
    }?;
    Ok(Some(value))
}
