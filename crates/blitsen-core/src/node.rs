//! The node tree and node content surfaces.

use blitsen_dom::{DomBackend, DomError};

use crate::StaticNodeList;

/// Authoritative-tree operations needed by JavaScript `Node` wrappers.
pub trait NodeTreeBackend {
    /// Stable node handle.
    type NodeId: Copy + Eq;

    /// Appends a node, moving it from an existing parent first.
    fn node_append(&mut self, parent: Self::NodeId, child: Self::NodeId) -> Result<(), DomError>;
    /// Inserts a node before an optional child of `parent`.
    fn node_insert_before(
        &mut self,
        parent: Self::NodeId,
        child: Self::NodeId,
        reference: Option<Self::NodeId>,
    ) -> Result<(), DomError>;
    /// Detaches a node.
    fn node_remove(&mut self, node: Self::NodeId) -> Result<(), DomError>;
    /// Replaces a node in its current parent.
    fn node_replace(
        &mut self,
        old: Self::NodeId,
        replacement: Self::NodeId,
    ) -> Result<(), DomError>;
    /// Returns a node's parent.
    fn node_parent(&self, node: Self::NodeId) -> Result<Option<Self::NodeId>, DomError>;
    /// Returns a static snapshot of a node's children.
    fn node_children(&self, node: Self::NodeId) -> Result<Vec<Self::NodeId>, DomError>;
    /// Returns a node's next sibling.
    fn node_next_sibling(&self, node: Self::NodeId) -> Result<Option<Self::NodeId>, DomError>;
}

impl<D: DomBackend> NodeTreeBackend for D {
    type NodeId = D::NodeId;

    fn node_append(&mut self, parent: Self::NodeId, child: Self::NodeId) -> Result<(), DomError> {
        self.append_child(parent, child)
    }

    fn node_insert_before(
        &mut self,
        parent: Self::NodeId,
        child: Self::NodeId,
        reference: Option<Self::NodeId>,
    ) -> Result<(), DomError> {
        self.insert_before(parent, child, reference)
    }

    fn node_remove(&mut self, node: Self::NodeId) -> Result<(), DomError> {
        self.remove(node)
    }

    fn node_replace(
        &mut self,
        old: Self::NodeId,
        replacement: Self::NodeId,
    ) -> Result<(), DomError> {
        self.replace(old, replacement)
    }

    fn node_parent(&self, node: Self::NodeId) -> Result<Option<Self::NodeId>, DomError> {
        self.parent(node)
    }

    fn node_children(&self, node: Self::NodeId) -> Result<Vec<Self::NodeId>, DomError> {
        self.children(node)
    }

    fn node_next_sibling(&self, node: Self::NodeId) -> Result<Option<Self::NodeId>, DomError> {
        self.next_sibling(node)
    }
}

/// Runtime-neutral implementation of JavaScript node mutation and traversal.
pub struct NodeTreeApi<'a, D: NodeTreeBackend> {
    backend: &'a mut D,
    node: D::NodeId,
}

/// Text and HTML operations required by JavaScript node wrappers.
pub trait NodeContentBackend {
    /// Stable node handle.
    type NodeId: Copy;

    /// Reads concatenated descendant text.
    fn content_text(&self, node: Self::NodeId) -> Result<String, DomError>;
    /// Replaces children with one text node (or none for an empty string).
    fn content_set_text(&mut self, node: Self::NodeId, text: &str) -> Result<(), DomError>;
    /// Serializes child nodes to HTML.
    fn content_inner_html(&self, node: Self::NodeId) -> Result<String, DomError>;
    /// Contextually parses and adopts replacement child nodes.
    fn content_set_inner_html(&mut self, node: Self::NodeId, html: &str) -> Result<(), DomError>;
}

impl<D: DomBackend> NodeContentBackend for D {
    type NodeId = D::NodeId;

    fn content_text(&self, node: Self::NodeId) -> Result<String, DomError> {
        self.text_content(node)
    }

    fn content_set_text(&mut self, node: Self::NodeId, text: &str) -> Result<(), DomError> {
        self.set_text_content(node, text)
    }

    fn content_inner_html(&self, node: Self::NodeId) -> Result<String, DomError> {
        self.inner_html(node)
    }

    fn content_set_inner_html(&mut self, node: Self::NodeId, html: &str) -> Result<(), DomError> {
        self.set_inner_html(node, html)
    }
}

/// Runtime-neutral `textContent` and `innerHTML` implementation.
pub struct NodeContentApi<'a, D: NodeContentBackend> {
    backend: &'a mut D,
    node: D::NodeId,
}

impl<'a, D: NodeContentBackend> NodeContentApi<'a, D> {
    /// Wraps a node from the authoritative backend.
    pub fn new(backend: &'a mut D, node: D::NodeId) -> Self {
        Self { backend, node }
    }

    /// Implements the `textContent` getter.
    pub fn text_content(&self) -> Result<String, DomError> {
        self.backend.content_text(self.node)
    }

    /// Implements the `textContent` setter.
    pub fn set_text_content(&mut self, text: &str) -> Result<(), DomError> {
        self.backend.content_set_text(self.node, text)
    }

    /// Implements the `innerHTML` getter.
    pub fn inner_html(&self) -> Result<String, DomError> {
        self.backend.content_inner_html(self.node)
    }

    /// Implements the `innerHTML` setter through the backend fragment parser.
    pub fn set_inner_html(&mut self, html: &str) -> Result<(), DomError> {
        self.backend.content_set_inner_html(self.node, html)
    }
}

impl<'a, D: NodeTreeBackend> NodeTreeApi<'a, D> {
    /// Wraps one handle from the authoritative backend tree.
    pub fn new(backend: &'a mut D, node: D::NodeId) -> Self {
        Self { backend, node }
    }

    /// Implements `appendChild` and returns the appended node.
    pub fn append_child(&mut self, child: D::NodeId) -> Result<D::NodeId, DomError> {
        self.backend.node_append(self.node, child)?;
        Ok(child)
    }

    /// Implements `insertBefore` and returns the inserted node.
    pub fn insert_before(
        &mut self,
        child: D::NodeId,
        reference: Option<D::NodeId>,
    ) -> Result<D::NodeId, DomError> {
        self.backend
            .node_insert_before(self.node, child, reference)?;
        Ok(child)
    }

    /// Implements `removeChild`, rejecting a node owned by another parent.
    pub fn remove_child(&mut self, child: D::NodeId) -> Result<D::NodeId, DomError> {
        if self.backend.node_parent(child)? != Some(self.node) {
            return Err(DomError::NotFound);
        }
        self.backend.node_remove(child)?;
        Ok(child)
    }

    /// Implements `Node.remove()`.
    pub fn remove(&mut self) -> Result<(), DomError> {
        self.backend.node_remove(self.node)
    }

    /// Implements the one-node v0 form of `replaceWith`.
    pub fn replace_with(&mut self, replacement: D::NodeId) -> Result<(), DomError> {
        self.backend.node_replace(self.node, replacement)
    }

    /// Implements `childNodes` as a snapshot for the current bridge turn.
    pub fn child_nodes(&self) -> Result<StaticNodeList<D::NodeId>, DomError> {
        self.backend
            .node_children(self.node)
            .map(StaticNodeList::new)
    }

    /// Implements `firstChild`.
    pub fn first_child(&self) -> Result<Option<D::NodeId>, DomError> {
        Ok(self.backend.node_children(self.node)?.first().copied())
    }

    /// Implements `nextSibling`.
    pub fn next_sibling(&self) -> Result<Option<D::NodeId>, DomError> {
        self.backend.node_next_sibling(self.node)
    }
}
