//! Building and reshaping the tree: creation, insertion, removal, markup.

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
        "createElement" => Ok(serialized(Some(
            dom.create_element(&element_name(
                HTML_NAMESPACE,
                bridge_arg(arguments, 0, "element name")?,
            )?)
            .map_err(dom_error)?,
        ))),
        "createElementNS" => Ok(serialized(Some(
            dom.create_element(&element_name(
                bridge_arg(arguments, 0, "namespace")?,
                bridge_arg(arguments, 1, "element name")?,
            )?)
            .map_err(dom_error)?,
        ))),
        "createTextNode" => Ok(serialized(Some(
            dom.create_text(bridge_arg(arguments, 0, "text")?)
                .map_err(dom_error)?,
        ))),
        "createComment" => Ok(serialized(Some(create_comment(
            dom,
            bridge_arg(arguments, 0, "comment data")?,
        )?))),
        "createFragment" => Ok(serialized(Some(
            dom.create_element(&DomName::html(FRAGMENT_TAG))
                .map_err(dom_error)?,
        ))),
        "cloneNode" => {
            let node = handle(runtime, arguments, 0)?;
            let deep = bridge_arg(arguments, 1, "clone depth")? == "true";
            Ok(serialized(Some(clone_node(dom, node, deep)?)))
        }
        "body" => Ok(serialized(dom.body())),
        "documentElement" => Ok(serialized(dom.document_element())),
        "appendChild" => {
            let parent = handle(runtime, arguments, 0)?;
            let child = handle(runtime, arguments, 1)?;
            dom.append_child(parent, child).map_err(dom_error)?;
            Ok(Value::Null)
        }
        "insertBefore" => {
            let parent = handle(runtime, arguments, 0)?;
            let child = handle(runtime, arguments, 1)?;
            let reference = if bridge_arg(arguments, 2, "reference")?.is_empty() {
                None
            } else {
                Some(handle(runtime, arguments, 2)?)
            };
            dom.insert_before(parent, child, reference)
                .map_err(dom_error)?;
            Ok(Value::Null)
        }
        "removeChild" => {
            let parent = handle(runtime, arguments, 0)?;
            let child = handle(runtime, arguments, 1)?;
            if dom.parent(child).map_err(dom_error)? != Some(parent) {
                return Err(dom_error(DomError::NotFound));
            }
            dom.remove(child).map_err(dom_error)?;
            Ok(Value::Null)
        }
        "remove" => {
            let node = handle(runtime, arguments, 0)?;
            if dom.parent(node).map_err(dom_error)?.is_some() {
                dom.remove(node).map_err(dom_error)?;
            }
            Ok(Value::Null)
        }
        "replaceWith" => {
            let node = handle(runtime, arguments, 0)?;
            let replacement = handle(runtime, arguments, 1)?;
            dom.replace(node, replacement).map_err(dom_error)?;
            Ok(Value::Null)
        }
        "parentNode" => Ok(serialized(
            dom.parent(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?,
        )),
        "childNodes" => Ok(json!(
            dom.children(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?
                .into_iter()
                .map(DomRuntime::serialize_handle)
                .collect::<Vec<_>>()
        )),
        "childElements" => {
            let children = dom
                .children(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?;
            let mut elements = Vec::new();
            for child in children {
                if dom.node_kind(child).map_err(dom_error)? == NodeKind::Element {
                    elements.push(DomRuntime::serialize_handle(child));
                }
            }
            Ok(json!(elements))
        }
        "firstChild" => Ok(serialized(
            dom.children(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?
                .first()
                .copied(),
        )),
        "lastChild" => Ok(serialized(
            dom.children(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?
                .last()
                .copied(),
        )),
        "nextSibling" => Ok(serialized(
            dom.next_sibling(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?,
        )),
        "previousSibling" => Ok(serialized(
            dom.previous_sibling(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?,
        )),
        // Walked in the backend rather than hop by hop from JavaScript: a text
        // node between two elements is ordinary in rendered markup, and every
        // one skipped would otherwise be a call of its own.
        "nextElementSibling" | "previousElementSibling" => {
            let forward = operation == "nextElementSibling";
            let mut sibling = handle(runtime, arguments, 0)?;
            loop {
                let next = if forward {
                    dom.next_sibling(sibling).map_err(dom_error)?
                } else {
                    dom.previous_sibling(sibling).map_err(dom_error)?
                };
                match next {
                    None => return Ok(Some(Value::Null)),
                    Some(node) if dom.node_kind(node).map_err(dom_error)? == NodeKind::Element => {
                        return Ok(Some(serialized(Some(node))));
                    }
                    Some(node) => sibling = node,
                }
            }
        }
        "isConnected" => Ok(Value::Bool(
            dom.is_connected(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?,
        )),
        // A comment's data is its `textContent`; the renderer's own text
        // collection skips comments, as it must for an element's.
        "textContent" => {
            let node = handle(runtime, arguments, 0)?;
            Ok(Value::String(
                match dom.node_kind(node).map_err(dom_error)? {
                    NodeKind::Comment => comment_data(dom, node)?,
                    _ => dom.text_content(node).map_err(dom_error)?,
                },
            ))
        }
        "setTextContent" => {
            let node = handle(runtime, arguments, 0)?;
            dom.set_text_content(node, bridge_arg(arguments, 1, "text")?)
                .map_err(dom_error)?;
            Ok(Value::Null)
        }
        "innerHTML" => Ok(Value::String(
            dom.inner_html(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?,
        )),
        "outerHTML" => Ok(Value::String(
            dom.outer_html(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?,
        )),
        "setInnerHTML" => {
            let node = handle(runtime, arguments, 0)?;
            dom.set_inner_html(node, bridge_arg(arguments, 1, "HTML")?)
                .map_err(dom_error)?;
            Ok(Value::Null)
        }
        // Parsed in the element the result lands in, which is what makes a `<td>`
        // survive `beforeend` on a `<tr>` and be discarded anywhere else.
        "insertAdjacentHTML" => {
            let node = handle(runtime, arguments, 0)?;
            let position = bridge_arg(arguments, 1, "position")?.to_ascii_lowercase();
            let sibling = matches!(position.as_str(), "beforebegin" | "afterend");
            let parent = if sibling {
                dom.parent(node)
                    .map_err(dom_error)?
                    .ok_or_else(|| dom_error(DomError::NotFound))?
            } else {
                node
            };
            let reference = match position.as_str() {
                "beforebegin" => Some(node),
                "afterend" => dom.next_sibling(node).map_err(dom_error)?,
                "afterbegin" => dom.children(node).map_err(dom_error)?.first().copied(),
                "beforeend" => None,
                _ => return Err(JsError::new("invalid insertAdjacentHTML position")),
            };
            let parsed = dom
                .parse_fragment(parent, bridge_arg(arguments, 2, "HTML")?)
                .map_err(dom_error)?;
            for child in &parsed {
                dom.insert_before(parent, *child, reference)
                    .map_err(dom_error)?;
            }
            Ok(json!(
                parsed
                    .into_iter()
                    .map(DomRuntime::serialize_handle)
                    .collect::<Vec<_>>()
            ))
        }
        _ => return Ok(None),
    }?;
    Ok(Some(value))
}
