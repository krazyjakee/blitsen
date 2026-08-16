//! Finding nodes: selectors, class and id lookup, identity and containment.

// These are slices of what was one `match`, so they share the helpers and
// imports their parent gathers rather than restating them.
use super::*;

const FOCUSABLE_CANDIDATES: &str = "[tabindex], button, input, select, textarea, a[href]";

fn has_attribute(dom: &BlitzDom, node: NodeId, name: &str) -> Result<bool, DomError> {
    dom.attribute(node, &DomName::attribute(name))
        .map(|value| value.is_some())
}

/// Matches the focus predicate the JavaScript bridge exposes.
///
/// Kept here so a sequential focus move can evaluate the live tree without one
/// JSON bridge round trip per attribute of every element. The `tabindex`
/// conversion follows the numeric test the old JavaScript predicate used. That
/// predicate did not special-case the `hidden` attribute or hidden inputs, so
/// neither does this one.
fn is_focusable(dom: &BlitzDom, node: NodeId) -> Result<bool, DomError> {
    if !matches!(dom.node_kind(node)?, NodeKind::Element) || has_attribute(dom, node, "disabled")? {
        return Ok(false);
    }

    let tag = dom.element_name(node)?.local;
    if let Some(tabindex) = dom.attribute(node, &DomName::attribute("tabindex"))? {
        return Ok(javascript_number(&tabindex).is_some_and(|tabindex| tabindex >= 0.0));
    }

    Ok(
        matches!(tag.as_str(), "button" | "input" | "select" | "textarea")
            || (tag == "a" && has_attribute(dom, node, "href")?),
    )
}

fn javascript_number(value: &str) -> Option<f64> {
    let value = value.trim_matches(|character| {
        matches!(
            character,
            '\u{0009}'
                | '\u{000a}'
                | '\u{000b}'
                | '\u{000c}'
                | '\u{000d}'
                | '\u{0020}'
                | '\u{00a0}'
                | '\u{1680}'
                | '\u{2000}'
                ..='\u{200a}'
                    | '\u{2028}'
                    | '\u{2029}'
                    | '\u{202f}'
                    | '\u{205f}'
                    | '\u{3000}'
                    | '\u{feff}'
        )
    });
    if value.is_empty() {
        return Some(0.0);
    }
    match value {
        "Infinity" | "+Infinity" => return Some(f64::INFINITY),
        "-Infinity" => return Some(f64::NEG_INFINITY),
        _ => {}
    }
    for (prefix, radix) in [
        ("0x", 16),
        ("0X", 16),
        ("0o", 8),
        ("0O", 8),
        ("0b", 2),
        ("0B", 2),
    ] {
        if let Some(digits) = value.strip_prefix(prefix) {
            if digits.is_empty() {
                return None;
            }
            return digits.chars().try_fold(0.0, |number, digit| {
                digit
                    .to_digit(radix)
                    .map(|digit| number * f64::from(radix) + f64::from(digit))
            });
        }
    }
    if value.eq_ignore_ascii_case("inf") || value.eq_ignore_ascii_case("infinity") {
        return None;
    }
    value.parse().ok()
}

fn focusable_nodes(dom: &BlitzDom) -> Result<Vec<NodeId>, DomError> {
    dom.query_selector_all(dom.document(), FOCUSABLE_CANDIDATES)?
        .into_iter()
        .filter_map(|node| match is_focusable(dom, node) {
            Ok(true) => Some(Ok(node)),
            Ok(false) => None,
            Err(error) => Some(Err(error)),
        })
        .collect()
}

fn next_focusable(
    dom: &BlitzDom,
    current: Option<NodeId>,
    backwards: bool,
) -> Result<Option<NodeId>, DomError> {
    let focusables = focusable_nodes(dom)?;
    if focusables.is_empty() {
        return Ok(None);
    }
    let current = current.and_then(|current| focusables.iter().position(|node| *node == current));
    let index = if backwards {
        current
            .filter(|index| *index > 0)
            .map_or(focusables.len() - 1, |index| index - 1)
    } else {
        current.map_or(0, |index| (index + 1) % focusables.len())
    };
    Ok(Some(focusables[index]))
}

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
        "isFocusable" => Ok(Value::Bool(
            is_focusable(dom, handle(runtime, arguments, 0)?).map_err(dom_error)?,
        )),
        "nextFocusable" => {
            let current = match bridge_arg(arguments, 0, "current focus handle")? {
                "" => None,
                _ => Some(handle(runtime, arguments, 0)?),
            };
            Ok(serialized(
                next_focusable(
                    dom,
                    current,
                    bridge_arg(arguments, 1, "focus direction")? == "true",
                )
                .map_err(dom_error)?,
            ))
        }
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
        "querySelectorAll" => Ok(serialized_all(
            dom.query_selector_all(dom.document(), bridge_arg(arguments, 0, "selector")?)
                .map_err(dom_error)?,
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
            Ok(serialized_all(
                dom.query_selector_all(node, bridge_arg(arguments, 1, "selector")?)
                    .map_err(dom_error)?,
            ))
        }
        "elementsByClassName" => {
            let root = dom.document();
            Ok(serialized_all(elements_by_class_name(
                dom,
                root,
                bridge_arg(arguments, 0, "class names")?,
            )?))
        }
        "elementsByClassNameIn" => {
            let node = handle(runtime, arguments, 0)?;
            Ok(serialized_all(elements_by_class_name(
                dom,
                node,
                bridge_arg(arguments, 1, "class names")?,
            )?))
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
