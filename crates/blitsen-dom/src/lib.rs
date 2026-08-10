//! Renderer-independent DOM interfaces.
//!
//! Blitz owns the live tree in the first backend.  This crate describes the
//! operations the bridge may perform without exposing Blitz types or keeping a
//! second, shadow DOM.

use std::error::Error;
use std::fmt;
use std::hash::Hash;

/// Namespace of an element or attribute name.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Namespace {
    /// The HTML namespace.
    Html,
    /// The SVG namespace.
    Svg,
    /// The MathML namespace.
    MathMl,
    /// No namespace, used by ordinary HTML attributes.
    None,
    /// A namespace not known to the v0 bridge.
    Other(String),
}

/// A namespace-aware DOM name.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DomName {
    /// Namespace containing the name.
    pub namespace: Namespace,
    /// Namespace-local name.
    pub local: String,
}

impl DomName {
    /// Creates an HTML element name.
    pub fn html(local: impl Into<String>) -> Self {
        Self {
            namespace: Namespace::Html,
            local: local.into(),
        }
    }

    /// Creates a non-namespaced attribute name.
    pub fn attribute(local: impl Into<String>) -> Self {
        Self {
            namespace: Namespace::None,
            local: local.into(),
        }
    }
}

/// Kind of a node in the authoritative backend tree.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeKind {
    /// The document root.
    Document,
    /// An element.
    Element,
    /// A text node.
    Text,
    /// A comment node.
    Comment,
    /// A document fragment.
    Fragment,
}

/// A CSS-pixel rectangle returned by layout.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    /// Horizontal position relative to the viewport.
    pub x: f32,
    /// Vertical position relative to the viewport.
    pub y: f32,
    /// Rectangle width.
    pub width: f32,
    /// Rectangle height.
    pub height: f32,
}

impl Rect {
    /// Reports whether a viewport point lies inside the rectangle.
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
    }
}

/// Proof that style and layout were flushed at a particular tree revision.
///
/// Layout-dependent backend reads accept this token so an accidental stale
/// read cannot silently return geometry from a previous mutation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LayoutSnapshot {
    revision: u64,
}

impl LayoutSnapshot {
    /// Creates a snapshot token for a backend revision.
    pub fn new(revision: u64) -> Self {
        Self { revision }
    }

    /// Returns the tree revision represented by this snapshot.
    pub fn revision(self) -> u64 {
        self.revision
    }
}

/// Failure produced while accessing or mutating the DOM backend.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DomError {
    /// A node handle did not resolve to a live node.
    StaleNode,
    /// The requested operation is not valid for the node's kind.
    InvalidNodeType,
    /// A tree mutation would create an invalid hierarchy.
    HierarchyRequest,
    /// A reference child was not a child of the supplied parent.
    NotFound,
    /// A selector or HTML fragment could not be parsed.
    Syntax(String),
    /// Layout was read without a snapshot for the current revision.
    LayoutNotFlushed,
    /// The concrete renderer reported another failure.
    Backend(String),
}

impl fmt::Display for DomError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StaleNode => formatter.write_str("node handle is stale"),
            Self::InvalidNodeType => formatter.write_str("operation is invalid for this node type"),
            Self::HierarchyRequest => formatter.write_str("mutation would create an invalid tree"),
            Self::NotFound => formatter.write_str("reference node was not found"),
            Self::Syntax(message) => write!(formatter, "invalid DOM syntax: {message}"),
            Self::LayoutNotFlushed => formatter.write_str("layout has not been flushed"),
            Self::Backend(message) => formatter.write_str(message),
        }
    }
}

impl Error for DomError {}

/// Boundary implemented by every DOM and renderer backend.
///
/// A backend's [`DomBackend::NodeId`] values are handles into its own tree, not
/// copies of nodes.  Every method must validate a handle before using it.
pub trait DomBackend {
    /// Opaque stable handle into the backend's authoritative tree.
    type NodeId: Copy + fmt::Debug + Eq + Hash;

    /// Returns the document root node.
    fn document(&self) -> Self::NodeId;
    /// Returns the document element, when one exists.
    fn document_element(&self) -> Option<Self::NodeId>;
    /// Returns the body element, when one exists.
    fn body(&self) -> Option<Self::NodeId>;
    /// Returns a node's kind, validating the handle.
    fn node_kind(&self, node: Self::NodeId) -> Result<NodeKind, DomError>;
    /// Returns an element's namespace-aware name.
    fn element_name(&self, node: Self::NodeId) -> Result<DomName, DomError>;

    /// Creates a detached element owned by this backend.
    fn create_element(&mut self, name: &DomName) -> Result<Self::NodeId, DomError>;
    /// Creates a detached text node owned by this backend.
    fn create_text(&mut self, text: &str) -> Result<Self::NodeId, DomError>;
    /// Appends a node, first detaching it from any existing parent.
    fn append_child(&mut self, parent: Self::NodeId, child: Self::NodeId) -> Result<(), DomError>;
    /// Inserts a node before an optional reference child.
    ///
    /// `None` has the same semantics as [`DomBackend::append_child`].
    fn insert_before(
        &mut self,
        parent: Self::NodeId,
        child: Self::NodeId,
        reference: Option<Self::NodeId>,
    ) -> Result<(), DomError>;
    /// Detaches a node without invalidating its handle.
    fn remove(&mut self, node: Self::NodeId) -> Result<(), DomError>;
    /// Replaces a node with another, detaching the replacement first.
    fn replace(&mut self, old: Self::NodeId, replacement: Self::NodeId) -> Result<(), DomError>;

    /// Returns a node's parent.
    fn parent(&self, node: Self::NodeId) -> Result<Option<Self::NodeId>, DomError>;
    /// Returns a snapshot of a node's children in tree order.
    fn children(&self, node: Self::NodeId) -> Result<Vec<Self::NodeId>, DomError>;
    /// Returns a node's previous sibling.
    fn previous_sibling(&self, node: Self::NodeId) -> Result<Option<Self::NodeId>, DomError>;
    /// Returns a node's next sibling.
    fn next_sibling(&self, node: Self::NodeId) -> Result<Option<Self::NodeId>, DomError>;
    /// Reports whether the node is currently connected to the document.
    fn is_connected(&self, node: Self::NodeId) -> Result<bool, DomError>;

    /// Returns an attribute value.
    fn attribute(&self, node: Self::NodeId, name: &DomName) -> Result<Option<String>, DomError>;
    /// Sets an attribute value and invalidates selector-dependent style.
    fn set_attribute(
        &mut self,
        node: Self::NodeId,
        name: &DomName,
        value: &str,
    ) -> Result<(), DomError>;
    /// Removes an attribute and returns whether it was present.
    fn remove_attribute(&mut self, node: Self::NodeId, name: &DomName) -> Result<bool, DomError>;

    /// Returns one inline CSS declaration by kebab-case property name.
    fn inline_style(&self, node: Self::NodeId, property: &str) -> Result<Option<String>, DomError>;
    /// Sets one inline CSS declaration, returning whether the value was valid.
    fn set_inline_style(
        &mut self,
        node: Self::NodeId,
        property: &str,
        value: &str,
    ) -> Result<bool, DomError>;
    /// Removes one inline CSS declaration and returns its previous value.
    fn remove_inline_style(
        &mut self,
        node: Self::NodeId,
        property: &str,
    ) -> Result<Option<String>, DomError>;

    /// Returns concatenated descendant text using DOM `textContent` semantics.
    fn text_content(&self, node: Self::NodeId) -> Result<String, DomError>;
    /// Replaces a node's children with text and invalidates layout.
    fn set_text_content(&mut self, node: Self::NodeId, text: &str) -> Result<(), DomError>;
    /// Parses an HTML fragment in the supplied element's context.
    ///
    /// Returned nodes are detached but adopted by this backend and may be
    /// inserted using the normal mutation methods.
    fn parse_fragment(
        &mut self,
        context: Self::NodeId,
        html: &str,
    ) -> Result<Vec<Self::NodeId>, DomError>;

    /// Returns the first matching descendant, or `None`.
    fn query_selector(
        &self,
        root: Self::NodeId,
        selector: &str,
    ) -> Result<Option<Self::NodeId>, DomError>;
    /// Returns a static, tree-ordered snapshot of matching descendants.
    fn query_selector_all(
        &self,
        root: Self::NodeId,
        selector: &str,
    ) -> Result<Vec<Self::NodeId>, DomError>;

    /// Resolves pending style and layout work and returns a current snapshot.
    fn flush_layout(&mut self) -> Result<LayoutSnapshot, DomError>;
    /// Returns border-box geometry after validating a layout snapshot.
    fn bounding_rect(&self, node: Self::NodeId, snapshot: LayoutSnapshot)
    -> Result<Rect, DomError>;
    /// Returns the topmost node at a viewport point after validating layout.
    fn hit_test(
        &self,
        x: f32,
        y: f32,
        snapshot: LayoutSnapshot,
    ) -> Result<Option<Self::NodeId>, DomError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangles_use_half_open_edges() {
        let rect = Rect {
            x: 10.0,
            y: 20.0,
            width: 30.0,
            height: 40.0,
        };
        assert!(rect.contains(10.0, 20.0));
        assert!(rect.contains(39.99, 59.99));
        assert!(!rect.contains(40.0, 60.0));
    }

    #[test]
    fn names_make_namespace_choice_explicit() {
        assert_eq!(DomName::html("div").namespace, Namespace::Html);
        assert_eq!(DomName::attribute("class").namespace, Namespace::None);
    }
}
