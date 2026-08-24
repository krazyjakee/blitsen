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
        "createElement" => {
            let node = dom
                .create_element(&element_name(
                    HTML_NAMESPACE,
                    bridge_arg(arguments, 0, "element name")?,
                )?)
                .map_err(dom_error)?;
            serialized(dom, Some(node))
        }
        "createElementNS" => {
            let node = dom
                .create_element(&element_name(
                    bridge_arg(arguments, 0, "namespace")?,
                    bridge_arg(arguments, 1, "element name")?,
                )?)
                .map_err(dom_error)?;
            serialized(dom, Some(node))
        }
        "createTextNode" => {
            let node = dom
                .create_text(bridge_arg(arguments, 0, "text")?)
                .map_err(dom_error)?;
            serialized(dom, Some(node))
        }
        "createComment" => {
            let node = create_comment(dom, bridge_arg(arguments, 0, "comment data")?)?;
            serialized(dom, Some(node))
        }
        "createFragment" => {
            let node = dom
                .create_element(&DomName::html(FRAGMENT_TAG))
                .map_err(dom_error)?;
            serialized(dom, Some(node))
        }
        "cloneNode" => {
            let node = handle(runtime, arguments, 0)?;
            let deep = bridge_arg(arguments, 1, "clone depth")? == "true";
            let clone = clone_node(dom, node, deep)?;
            serialized(dom, Some(clone))
        }
        "body" => serialized(dom, dom.body()),
        "documentElement" => serialized(dom, dom.document_element()),
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
        "parentNode" => serialized(
            dom,
            dom.parent(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?,
        ),
        "childNodes" => serialized_all(
            dom,
            dom.children(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?,
        ),
        "childElements" => {
            let children = dom
                .children(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?;
            let mut elements = Vec::new();
            for child in children {
                if dom.node_kind(child).map_err(dom_error)? == NodeKind::Element {
                    elements.push(child);
                }
            }
            serialized_all(dom, elements)
        }
        "firstChild" => serialized(
            dom,
            dom.children(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?
                .first()
                .copied(),
        ),
        "lastChild" => serialized(
            dom,
            dom.children(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?
                .last()
                .copied(),
        ),
        "nextSibling" => serialized(
            dom,
            dom.next_sibling(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?,
        ),
        "previousSibling" => serialized(
            dom,
            dom.previous_sibling(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?,
        ),
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
                        return Ok(Some(serialized(dom, Some(node))?));
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
            serialized_all(dom, parsed)
        }
        _ => return Ok(None),
    }?;
    Ok(Some(value))
}
