//! The DOM bridge operations, grouped by what each one acts on.
//!
//! Every operation the bootstrap can name arrives here as a string and a list
//! of string arguments. A group answers `Ok(None)` for a name it does not own,
//! so `dispatch` moves on to the next; the order of the groups carries no
//! meaning, because no name appears in two of them.

mod attributes;
mod document;
mod layout;
mod query;
mod style;
mod text_input;
mod tree;

use blitsen_blitz::BlitzDom;
use blitsen_core::js_property_to_css;
use blitsen_dom::{
    DomBackend, DomError, DomName, Namespace, NodeKind, Rect, SelectionDirection, TextEdit,
    TextMotion, TextSelection,
};
use blitsen_js::JsError;
use blitz::dom::NodeId;
use serde_json::{Value, json};

use super::{DomRuntime, web_url};

/// A group's answer: `None` when the operation belongs to another group.
type Answer = Result<Option<Value>, JsError>;

type Group = fn(&DomRuntime, &mut BlitzDom, &str, &[String]) -> Answer;

const GROUPS: [Group; 7] = [
    query::dispatch,
    layout::dispatch,
    tree::dispatch,
    attributes::dispatch,
    style::dispatch,
    text_input::dispatch,
    document::dispatch,
];

pub(super) fn dispatch(
    runtime: &DomRuntime,
    operation: &str,
    arguments: &[String],
) -> Result<Value, JsError> {
    let shared = runtime.document();
    let mut dom = shared.borrow_mut();
    for group in GROUPS {
        if let Some(value) = group(runtime, &mut dom, operation, arguments)? {
            return Ok(value);
        }
    }
    Err(JsError::new(format!(
        "unknown DOM bridge operation: {operation}"
    )))
}

fn bridge_arg<'a>(arguments: &'a [String], index: usize, name: &str) -> Result<&'a str, JsError> {
    arguments
        .get(index)
        .map(String::as_str)
        .ok_or_else(|| JsError::new(format!("missing {name}")))
}

/// Reads a rule index, which JavaScript has already range-checked.
fn bridge_index(arguments: &[String], index: usize) -> Result<usize, JsError> {
    bridge_arg(arguments, index, "rule index")?
        .parse::<usize>()
        .map_err(|_| JsError::new("invalid CSS rule index"))
}

fn handle(_runtime: &DomRuntime, arguments: &[String], index: usize) -> Result<NodeId, JsError> {
    bridge_arg(arguments, index, "node handle")?
        .parse::<u64>()
        .map(NodeId::from_u64)
        .map_err(|_| JsError::new("invalid DOM node handle"))
}

fn serialized(dom: &BlitzDom, node: Option<NodeId>) -> Result<Value, JsError> {
    node.map(|node| described(dom, node))
        .transpose()
        .map(|node| node.unwrap_or(Value::Null))
}

/// The shape every operation answering with more than one node returns.
fn serialized_all(
    dom: &BlitzDom,
    nodes: impl IntoIterator<Item = NodeId>,
) -> Result<Value, JsError> {
    nodes
        .into_iter()
        .map(|node| described(dom, node))
        .collect::<Result<Vec<_>, _>>()
        .map(Value::Array)
}

/// A node result carries the information JavaScript needs to select its
/// interface. This is deliberately only a description, not an owning wrapper:
/// `__blitsenWrap` remains the single identity and lifetime boundary.
fn described(dom: &BlitzDom, node: NodeId) -> Result<Value, JsError> {
    let kind = dom.node_kind(node).map_err(dom_error)?;
    let kind = match kind {
        NodeKind::Element => "element",
        NodeKind::Document => "document",
        NodeKind::Text => "text",
        NodeKind::Comment => "comment",
        NodeKind::Fragment => "fragment",
    };
    let mut description = json!({
        "handle": DomRuntime::serialize_handle(node),
        "kind": kind,
    });
    if kind == "element" {
        description["tagName"] = Value::String(dom.element_name(node).map_err(dom_error)?.local);
    }
    Ok(description)
}

/// An ordinary HTML attribute's name, from the argument at `index`.
///
/// Lower-cased because the null namespace is the one HTML attributes live in and
/// it folds case; the namespaced trio goes through [`attribute_name`] instead.
fn attribute_arg(arguments: &[String], index: usize) -> Result<DomName, JsError> {
    Ok(DomName::attribute(
        bridge_arg(arguments, index, "attribute name")?.to_ascii_lowercase(),
    ))
}

/// A namespaced attribute's name, from the pair the `*AttributeNS` trio passes.
fn attribute_arg_ns(arguments: &[String], index: usize) -> Result<DomName, JsError> {
    attribute_name(
        bridge_arg(arguments, index, "namespace")?,
        bridge_arg(arguments, index + 1, "attribute name")?,
    )
}

fn dom_error(error: DomError) -> JsError {
    JsError::new(error.to_string())
}

const HTML_NAMESPACE: &str = "http://www.w3.org/1999/xhtml";
const SVG_NAMESPACE: &str = "http://www.w3.org/2000/svg";
const MATHML_NAMESPACE: &str = "http://www.w3.org/1998/Math/MathML";

/// The element a fragment's children are parked under.
///
/// A `DocumentFragment` is a real detached element in the backend: that is what
/// gives its children a parent to be parsed, serialized and cloned against, and
/// it is never connected, so it is never styled, laid out or painted. The name
/// is `template` because template contents are the one parsing context that
/// accepts every kind of child, including the table rows an ordinary element
/// would discard.
const FRAGMENT_TAG: &str = "template";

fn namespace_uri(namespace: &Namespace) -> Option<&str> {
    match namespace {
        Namespace::Html => Some(HTML_NAMESPACE),
        Namespace::Svg => Some(SVG_NAMESPACE),
        Namespace::MathMl => Some(MATHML_NAMESPACE),
        Namespace::None => None,
        Namespace::Other(uri) => Some(uri),
    }
}

fn namespace_from_uri(uri: &str) -> Namespace {
    match uri {
        "" => Namespace::None,
        HTML_NAMESPACE => Namespace::Html,
        SVG_NAMESPACE => Namespace::Svg,
        MATHML_NAMESPACE => Namespace::MathMl,
        other => Namespace::Other(other.to_owned()),
    }
}

fn element_name(namespace: &str, name: &str) -> Result<DomName, JsError> {
    if name.is_empty()
        || name.chars().any(|character| {
            character.is_whitespace() || matches!(character, '<' | '>' | '/' | '\0')
        })
    {
        return Err(JsError::new("invalid element name"));
    }
    let namespace = namespace_from_uri(namespace);
    // Only HTML folds case; SVG has `linearGradient` and `clipPath`.
    let local = if namespace == Namespace::Html {
        name.to_ascii_lowercase()
    } else {
        name.to_owned()
    };
    Ok(DomName { namespace, local })
}

/// Builds the attribute name behind the `*AttributeNS` trio.
///
/// A qualified name's prefix is not kept: Blitz keys an attribute by namespace
/// and local name, which is the pair `getAttributeNS` asks back for. Case folds
/// only in the null namespace — the one ordinary HTML attributes live in, which
/// the rest of the bridge already lower-cases — so `xlink:href` and `xml:space`
/// keep theirs, as `createElementNS` keeps an element's.
fn attribute_name(namespace: &str, qualified: &str) -> Result<DomName, JsError> {
    let local = qualified.rsplit(':').next().unwrap_or_default();
    if local.is_empty()
        || local.chars().any(|character| {
            character.is_whitespace() || matches!(character, '<' | '>' | '/' | '=' | '"' | '\0')
        })
    {
        return Err(JsError::new("invalid attribute name"));
    }
    let namespace = namespace_from_uri(namespace);
    let local = if namespace == Namespace::None {
        local.to_ascii_lowercase()
    } else {
        local.to_owned()
    };
    Ok(DomName { namespace, local })
}

/// Returns the descendants of `root` carrying every one of `names` as a class.
///
/// Matched against the class attribute's tokens rather than through a selector:
/// a class a bundler invents contains characters (`w-1/2`, `md:flex`) that only
/// survive a selector escaped, and the escaping is what would be guessed at.
fn elements_by_class_name(
    dom: &BlitzDom,
    root: NodeId,
    names: &str,
) -> Result<Vec<NodeId>, JsError> {
    let tokens = names.split_ascii_whitespace().collect::<Vec<_>>();
    let mut found = Vec::new();
    if tokens.is_empty() {
        return Ok(found);
    }
    let mut pending = dom.children(root).map_err(dom_error)?;
    pending.reverse();
    while let Some(node) = pending.pop() {
        if dom.node_kind(node).map_err(dom_error)? != NodeKind::Element {
            continue;
        }
        let classes = dom
            .attribute(node, &DomName::attribute("class"))
            .map_err(dom_error)?
            .unwrap_or_default();
        if tokens
            .iter()
            .all(|token| classes.split_ascii_whitespace().any(|name| name == *token))
        {
            found.push(node);
        }
        let mut children = dom.children(node).map_err(dom_error)?;
        children.reverse();
        pending.extend(children);
    }
    Ok(found)
}

/// Returns an element's attribute names in document order.
///
/// Read through the renderer's own view of the node: the DOM boundary can read
/// one attribute by name but cannot enumerate them, and `dataset` has to know
/// which `data-` attributes exist before it can answer for them. Namespaced,
/// because a clone reads its attributes back through this and `xlink:href`
/// copied into the null namespace would be a different attribute.
fn attribute_names(dom: &BlitzDom, node: NodeId) -> Result<Vec<DomName>, JsError> {
    Ok(dom
        .document_ref()
        .get_node(node)
        .ok_or_else(|| dom_error(DomError::StaleNode))?
        .element_data()
        .ok_or_else(|| dom_error(DomError::InvalidNodeType))?
        .attrs()
        .iter()
        .map(|attribute| DomName {
            namespace: namespace_from_uri(&attribute.name.ns),
            local: attribute.name.local.to_string(),
        })
        .collect())
}

/// Copies a node, deeply when asked, the way `cloneNode` defines it.
///
/// A clone carries the tree and nothing else: no listeners, no wrapper identity
/// and no JavaScript state, which is what the DOM specifies. Depth is served by
/// serializing and reparsing, because that is the only complete copy the DOM
/// boundary offers.
fn clone_node(dom: &mut BlitzDom, node: NodeId, deep: bool) -> Result<NodeId, JsError> {
    match dom.node_kind(node).map_err(dom_error)? {
        NodeKind::Element => {
            let name = dom.element_name(node).map_err(dom_error)?;
            let clone = dom.create_element(&name).map_err(dom_error)?;
            for attribute in attribute_names(dom, node)? {
                if let Some(value) = dom.attribute(node, &attribute).map_err(dom_error)? {
                    dom.set_attribute(clone, &attribute, &value)
                        .map_err(dom_error)?;
                }
            }
            if deep {
                let html = dom.inner_html(node).map_err(dom_error)?;
                dom.set_inner_html(clone, &html).map_err(dom_error)?;
            }
            Ok(clone)
        }
        NodeKind::Text => {
            let text = dom.text_content(node).map_err(dom_error)?;
            dom.create_text(&text).map_err(dom_error)
        }
        NodeKind::Comment => {
            let data = comment_data(dom, node)?;
            create_comment(dom, &data)
        }
        NodeKind::Document | NodeKind::Fragment => Err(dom_error(DomError::InvalidNodeType)),
    }
}

fn comment_data(dom: &BlitzDom, node: NodeId) -> Result<String, JsError> {
    match &dom
        .document_ref()
        .get_node(node)
        .ok_or_else(|| dom_error(DomError::StaleNode))?
        .data
    {
        blitz::dom::NodeData::Comment { contents } => Ok(contents.clone()),
        _ => Err(dom_error(DomError::InvalidNodeType)),
    }
}

/// Creates a detached comment node by parsing one.
///
/// The DOM boundary has no comment constructor, so the fragment parser is the
/// way to reach the node kind. Data that would close the comment early is
/// refused rather than silently truncated.
fn create_comment(dom: &mut BlitzDom, data: &str) -> Result<NodeId, JsError> {
    if data.contains("-->")
        || data.contains("--!>")
        || data.starts_with('>')
        || data.starts_with("->")
    {
        return Err(JsError::new(
            "comment data cannot contain a comment terminator",
        ));
    }
    let context = dom
        .body()
        .or_else(|| dom.document_element())
        .ok_or_else(|| dom_error(DomError::NotFound))?;
    let nodes = dom
        .parse_fragment(context, &format!("<!--{data}-->"))
        .map_err(dom_error)?;
    match nodes.first() {
        Some(node) if nodes.len() == 1 && dom.node_kind(*node) == Ok(NodeKind::Comment) => {
            Ok(*node)
        }
        _ => Err(JsError::new("comment data could not be represented")),
    }
}
