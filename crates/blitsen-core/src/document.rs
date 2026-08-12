//! Document-level queries, creation, and the static node lists they return.

use blitsen_dom::{DomBackend, DomError, DomName};

/// DOM operations needed by the JavaScript `document` object.
///
/// The blanket implementation delegates directly to [`DomBackend`], ensuring
/// selector matching remains the renderer's implementation (Stylo for Blitz).
pub trait DocumentBackend {
    /// Stable node handle returned to wrappers.
    type NodeId: Copy;

    /// Queries the document for the first selector match.
    fn document_query_selector(&self, selector: &str) -> Result<Option<Self::NodeId>, DomError>;
    /// Queries the document for every selector match in tree order.
    fn document_query_selector_all(&self, selector: &str) -> Result<Vec<Self::NodeId>, DomError>;
    /// Looks up an exact element ID through the backend's maintained index.
    fn document_get_element_by_id(&self, id: &str) -> Result<Option<Self::NodeId>, DomError>;
    /// Creates a detached HTML element.
    fn document_create_element(&mut self, local_name: &str) -> Result<Self::NodeId, DomError>;
    /// Creates a detached text node.
    fn document_create_text(&mut self, text: &str) -> Result<Self::NodeId, DomError>;
    /// Returns the body element.
    fn document_body(&self) -> Option<Self::NodeId>;
    /// Returns the document element.
    fn document_element(&self) -> Option<Self::NodeId>;
}

impl<D: DomBackend> DocumentBackend for D {
    type NodeId = D::NodeId;

    fn document_query_selector(&self, selector: &str) -> Result<Option<Self::NodeId>, DomError> {
        self.query_selector(self.document(), selector)
    }

    fn document_query_selector_all(&self, selector: &str) -> Result<Vec<Self::NodeId>, DomError> {
        self.query_selector_all(self.document(), selector)
    }

    fn document_get_element_by_id(&self, id: &str) -> Result<Option<Self::NodeId>, DomError> {
        self.get_element_by_id(id)
    }

    fn document_create_element(&mut self, local_name: &str) -> Result<Self::NodeId, DomError> {
        self.create_element(&DomName::html(local_name.to_ascii_lowercase()))
    }

    fn document_create_text(&mut self, text: &str) -> Result<Self::NodeId, DomError> {
        self.create_text(text)
    }

    fn document_body(&self) -> Option<Self::NodeId> {
        self.body()
    }

    fn document_element(&self) -> Option<Self::NodeId> {
        self.document_element()
    }
}

/// Static `NodeList` snapshot returned by `querySelectorAll` in v0.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StaticNodeList<N> {
    nodes: Vec<N>,
}

impl<N> StaticNodeList<N> {
    /// Creates a snapshot from nodes already in tree order.
    pub fn new(nodes: Vec<N>) -> Self {
        Self { nodes }
    }

    /// Returns the number of nodes in the snapshot.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Reports whether the snapshot contains no nodes.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Returns a node by zero-based index.
    pub fn item(&self, index: usize) -> Option<&N> {
        self.nodes.get(index)
    }

    /// Iterates over the snapshot in tree order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &N> {
        self.nodes.iter()
    }

    /// Consumes the snapshot and returns its handles.
    pub fn into_vec(self) -> Vec<N> {
        self.nodes
    }
}

/// Runtime-neutral implementation of the v0 JavaScript `document` surface.
pub struct DocumentApi<'a, D> {
    backend: &'a mut D,
}

impl<'a, D: DocumentBackend> DocumentApi<'a, D> {
    /// Borrows the authoritative DOM backend as a document object.
    pub fn new(backend: &'a mut D) -> Self {
        Self { backend }
    }

    /// Implements `document.querySelector`.
    pub fn query_selector(&self, selector: &str) -> Result<Option<D::NodeId>, DomError> {
        self.backend.document_query_selector(selector)
    }

    /// Implements `document.querySelectorAll` as a static v0 snapshot.
    pub fn query_selector_all(
        &self,
        selector: &str,
    ) -> Result<StaticNodeList<D::NodeId>, DomError> {
        self.backend
            .document_query_selector_all(selector)
            .map(StaticNodeList::new)
    }

    /// Implements `document.getElementById` without rebuilding an ID index.
    pub fn get_element_by_id(&self, id: &str) -> Result<Option<D::NodeId>, DomError> {
        self.backend.document_get_element_by_id(id)
    }

    /// Implements HTML `document.createElement`.
    pub fn create_element(&mut self, local_name: &str) -> Result<D::NodeId, DomError> {
        self.backend.document_create_element(local_name)
    }

    /// Implements `document.createTextNode`.
    pub fn create_text_node(&mut self, text: &str) -> Result<D::NodeId, DomError> {
        self.backend.document_create_text(text)
    }

    /// Implements `document.body`.
    pub fn body(&self) -> Option<D::NodeId> {
        self.backend.document_body()
    }

    /// Implements `document.documentElement`.
    pub fn document_element(&self) -> Option<D::NodeId> {
        self.backend.document_element()
    }
}
