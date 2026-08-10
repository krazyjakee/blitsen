//! Renderer-independent DOM interfaces.
//!
//! Blitz owns the live tree in the first backend.  This crate describes the
//! operations the bridge may perform without exposing Blitz types or keeping a
//! second, shadow DOM.

use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::hash::Hash;

/// Strategy used to turn dirty marks into frame work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InvalidationMode {
    /// Restyle and relayout only explicitly dirty nodes/subtrees.
    FineGrained,
    /// Documented v0 fallback when the backend cannot expose incremental work.
    FullDocumentFallback,
}

/// Observable style/layout work performed for one frame.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct InvalidationMetrics {
    /// Number of nodes restyled.
    pub restyled_nodes: usize,
    /// Number of nodes relaid out.
    pub relaid_out_nodes: usize,
}

/// Dirty-node plan consumed by a backend at the next frame boundary.
#[derive(Debug)]
pub struct FrameInvalidation<N> {
    /// Nodes requiring selector/cascade recomputation.
    pub style_nodes: HashSet<N>,
    /// Nodes whose layout subtrees may have changed.
    pub layout_nodes: HashSet<N>,
    /// Whether the backend must recompute the complete document.
    pub full_document: bool,
    /// Work counters for regression instrumentation.
    pub metrics: InvalidationMetrics,
}

/// Tracks mutation invalidation without maintaining a shadow DOM.
#[derive(Debug)]
pub struct InvalidationTracker<N> {
    mode: InvalidationMode,
    style_dirty: HashSet<N>,
    layout_dirty: HashSet<N>,
}

impl<N: Copy + Eq + Hash> InvalidationTracker<N> {
    /// Creates an empty tracker in the selected backend mode.
    pub fn new(mode: InvalidationMode) -> Self {
        Self {
            mode,
            style_dirty: HashSet::new(),
            layout_dirty: HashSet::new(),
        }
    }

    /// Marks selector/cascade data dirty without assuming layout changed.
    pub fn mark_style(&mut self, node: N) {
        self.style_dirty.insert(node);
    }

    /// Marks layout dirty and propagates the mark through every ancestor.
    pub fn mark_layout(&mut self, node: N, mut parent: impl FnMut(N) -> Option<N>) {
        let mut current = Some(node);
        while let Some(node) = current {
            if !self.layout_dirty.insert(node) {
                break;
            }
            current = parent(node);
        }
    }

    /// Marks a mutation that can affect both cascade and geometry.
    pub fn mark_mutation(&mut self, node: N, parent: impl FnMut(N) -> Option<N>) {
        self.mark_style(node);
        self.mark_layout(node, parent);
    }

    /// Drains dirty state into the next frame's work plan.
    ///
    /// `document_nodes` is used only by the full-document fallback to make its
    /// cost visible in metrics.
    pub fn take_frame(&mut self, document_nodes: usize) -> FrameInvalidation<N> {
        let dirty = !self.style_dirty.is_empty() || !self.layout_dirty.is_empty();
        let full_document = dirty && self.mode == InvalidationMode::FullDocumentFallback;
        let metrics = if full_document {
            InvalidationMetrics {
                restyled_nodes: document_nodes,
                relaid_out_nodes: document_nodes,
            }
        } else {
            InvalidationMetrics {
                restyled_nodes: self.style_dirty.len(),
                relaid_out_nodes: self.layout_dirty.len(),
            }
        };
        FrameInvalidation {
            style_nodes: std::mem::take(&mut self.style_dirty),
            layout_nodes: std::mem::take(&mut self.layout_dirty),
            full_document,
            metrics,
        }
    }

    /// Reports whether any work is pending.
    pub fn is_dirty(&self) -> bool {
        !self.style_dirty.is_empty() || !self.layout_dirty.is_empty()
    }
}

/// Generational handle into a [`NodeArena`].
///
/// The slot selects storage and the generation prevents a stale handle from
/// resolving to an unrelated node after that storage is reused.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct NodeId {
    slot: u32,
    generation: u32,
}

impl NodeId {
    /// Creates a handle from its stable wire representation.
    pub const fn new(slot: u32, generation: u32) -> Self {
        Self { slot, generation }
    }

    /// Returns the arena slot.
    pub const fn slot(self) -> u32 {
        self.slot
    }

    /// Returns the slot generation.
    pub const fn generation(self) -> u32 {
        self.generation
    }

    /// Packs the handle for opaque storage in a JavaScript wrapper.
    pub const fn to_u64(self) -> u64 {
        (self.generation as u64) << 32 | self.slot as u64
    }

    /// Restores a handle previously produced by [`NodeId::to_u64`].
    pub const fn from_u64(value: u64) -> Self {
        Self {
            slot: value as u32,
            generation: (value >> 32) as u32,
        }
    }
}

#[derive(Debug)]
struct NodeSlot<T> {
    generation: u32,
    value: Option<T>,
    tree_owned: bool,
    js_references: u32,
}

/// Owns backend node handles and coordinates tree and JavaScript lifetimes.
///
/// A connected tree node is tree-owned. Detaching it releases that ownership;
/// it remains live only while one or more JavaScript wrappers retain it.
#[derive(Debug)]
pub struct NodeArena<T> {
    slots: Vec<NodeSlot<T>>,
    free: Vec<u32>,
    len: usize,
}

impl<T> Default for NodeArena<T> {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
            len: 0,
        }
    }
}

impl<T> NodeArena<T> {
    /// Creates an empty arena.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts a node initially owned by the document tree.
    pub fn insert(&mut self, value: T) -> NodeId {
        self.len += 1;
        if let Some(slot_index) = self.free.pop() {
            let slot = &mut self.slots[slot_index as usize];
            debug_assert!(slot.value.is_none());
            slot.generation = slot.generation.wrapping_add(1);
            slot.value = Some(value);
            slot.tree_owned = true;
            slot.js_references = 0;
            NodeId::new(slot_index, slot.generation)
        } else {
            let slot = u32::try_from(self.slots.len()).expect("node arena exhausted u32 slots");
            self.slots.push(NodeSlot {
                generation: 0,
                value: Some(value),
                tree_owned: true,
                js_references: 0,
            });
            NodeId::new(slot, 0)
        }
    }

    /// Returns the number of live nodes.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Reports whether the arena contains no live nodes.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Resolves a handle after validating both slot and generation.
    pub fn get(&self, node: NodeId) -> Result<&T, DomError> {
        self.valid_slot(node)?
            .value
            .as_ref()
            .ok_or(DomError::StaleNode)
    }

    /// Mutably resolves a handle after validating both slot and generation.
    pub fn get_mut(&mut self, node: NodeId) -> Result<&mut T, DomError> {
        self.valid_slot_mut(node)?
            .value
            .as_mut()
            .ok_or(DomError::StaleNode)
    }

    /// Adds one JavaScript wrapper reference to a live node.
    pub fn retain_for_js(&mut self, node: NodeId) -> Result<(), DomError> {
        let slot = self.valid_slot_mut(node)?;
        slot.js_references = slot
            .js_references
            .checked_add(1)
            .ok_or_else(|| DomError::Backend("JavaScript node reference count overflow".into()))?;
        Ok(())
    }

    /// Releases one JavaScript wrapper reference.
    ///
    /// Returns `true` when this release collected a detached node.
    pub fn release_from_js(&mut self, node: NodeId) -> Result<bool, DomError> {
        let slot = self.valid_slot_mut(node)?;
        slot.js_references = slot.js_references.checked_sub(1).ok_or_else(|| {
            DomError::Backend("node has no JavaScript reference to release".into())
        })?;
        Ok(self.collect_if_unowned(node))
    }

    /// Marks a node as attached and therefore owned by the tree.
    pub fn attach_to_tree(&mut self, node: NodeId) -> Result<(), DomError> {
        self.valid_slot_mut(node)?.tree_owned = true;
        Ok(())
    }

    /// Releases tree ownership after a node is detached.
    ///
    /// Returns `true` when no JavaScript wrapper retained the node and it was
    /// collected immediately.
    pub fn detach_from_tree(&mut self, node: NodeId) -> Result<bool, DomError> {
        self.valid_slot_mut(node)?.tree_owned = false;
        Ok(self.collect_if_unowned(node))
    }

    /// Explicitly destroys a node even if a stale JavaScript wrapper remains.
    ///
    /// This is used only by backend operations whose semantics drop storage;
    /// ordinary DOM removal must use [`NodeArena::detach_from_tree`].
    pub fn destroy(&mut self, node: NodeId) -> Result<T, DomError> {
        self.valid_slot(node)?;
        self.take_slot(node).ok_or(DomError::StaleNode)
    }

    /// Reports whether the tree currently owns a node.
    pub fn is_tree_owned(&self, node: NodeId) -> Result<bool, DomError> {
        Ok(self.valid_slot(node)?.tree_owned)
    }

    /// Returns the number of JavaScript wrappers retaining a node.
    pub fn js_reference_count(&self, node: NodeId) -> Result<u32, DomError> {
        Ok(self.valid_slot(node)?.js_references)
    }

    fn valid_slot(&self, node: NodeId) -> Result<&NodeSlot<T>, DomError> {
        let slot = self
            .slots
            .get(node.slot as usize)
            .ok_or(DomError::StaleNode)?;
        if slot.generation != node.generation || slot.value.is_none() {
            return Err(DomError::StaleNode);
        }
        Ok(slot)
    }

    fn valid_slot_mut(&mut self, node: NodeId) -> Result<&mut NodeSlot<T>, DomError> {
        let slot = self
            .slots
            .get_mut(node.slot as usize)
            .ok_or(DomError::StaleNode)?;
        if slot.generation != node.generation || slot.value.is_none() {
            return Err(DomError::StaleNode);
        }
        Ok(slot)
    }

    fn collect_if_unowned(&mut self, node: NodeId) -> bool {
        let slot = &self.slots[node.slot as usize];
        if slot.tree_owned || slot.js_references != 0 {
            false
        } else {
            self.take_slot(node);
            true
        }
    }

    fn take_slot(&mut self, node: NodeId) -> Option<T> {
        let slot = &mut self.slots[node.slot as usize];
        let value = slot.value.take()?;
        slot.tree_owned = false;
        slot.js_references = 0;
        self.free.push(node.slot);
        self.len -= 1;
        Some(value)
    }
}

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

/// CSSOM box and scroll measurements from one validated layout snapshot.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LayoutMetrics {
    /// Viewport-relative border box.
    pub rect: Rect,
    /// Rounded border-box width.
    pub offset_width: f64,
    /// Rounded border-box height.
    pub offset_height: f64,
    /// Rounded padding-box width excluding any reserved scrollbar gutter.
    pub client_width: f64,
    /// Rounded padding-box height excluding any reserved scrollbar gutter.
    pub client_height: f64,
    /// Current element scroll offsets.
    pub scroll_left: f64,
    /// Current vertical element scroll offset.
    pub scroll_top: f64,
}

/// Result of resolving a viewport point against the laid-out document.
#[derive(Clone, Debug, PartialEq)]
pub struct HitTest<N> {
    /// Deepest interactive DOM node at the point.
    pub target: N,
    /// Connected propagation path in root-to-target order.
    pub path: Vec<N>,
    /// Horizontal CSS-pixel coordinate within the target border box.
    pub offset_x: f32,
    /// Vertical CSS-pixel coordinate within the target border box.
    pub offset_y: f32,
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
    /// Serializes the complete inline declaration block.
    fn inline_style_text(&self, node: Self::NodeId) -> Result<String, DomError>;
    /// Parses and replaces the complete inline declaration block. Invalid
    /// declarations are ignored according to CSS parsing rules.
    fn set_inline_style_text(&mut self, node: Self::NodeId, css: &str) -> Result<(), DomError>;

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
    /// Serializes a node's children as HTML.
    fn inner_html(&self, node: Self::NodeId) -> Result<String, DomError>;
    /// Contextually parses HTML and replaces a node's children with the
    /// adopted fragment.
    fn set_inner_html(&mut self, node: Self::NodeId, html: &str) -> Result<(), DomError>;

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
    /// Returns the first element with the exact `id` attribute value.
    fn get_element_by_id(&self, id: &str) -> Result<Option<Self::NodeId>, DomError>;

    /// Resolves pending style and layout work and returns a current snapshot.
    fn flush_layout(&mut self) -> Result<LayoutSnapshot, DomError>;
    /// Reports whether a layout-dependent read would force synchronous work.
    fn layout_is_dirty(&self) -> bool;
    /// Returns border-box geometry after validating a layout snapshot.
    fn bounding_rect(&self, node: Self::NodeId, snapshot: LayoutSnapshot)
    -> Result<Rect, DomError>;
    /// Returns CSSOM box and scroll measurements from a validated snapshot.
    fn layout_metrics(
        &self,
        node: Self::NodeId,
        snapshot: LayoutSnapshot,
    ) -> Result<LayoutMetrics, DomError>;
    /// Sets one or both scroll axes without bubbling into an ancestor scroller.
    fn set_scroll_offset(
        &mut self,
        node: Self::NodeId,
        left: Option<f64>,
        top: Option<f64>,
        snapshot: LayoutSnapshot,
    ) -> Result<(), DomError>;
    /// Returns the topmost node and its propagation path after validating layout.
    fn hit_test(
        &self,
        x: f32,
        y: f32,
        snapshot: LayoutSnapshot,
    ) -> Result<Option<HitTest<Self::NodeId>>, DomError>;
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

    #[test]
    fn reused_slots_reject_the_previous_generation() {
        let mut arena = NodeArena::new();
        let old = arena.insert("old");
        assert_eq!(arena.destroy(old), Ok("old"));

        let replacement = arena.insert("replacement");
        assert_eq!(old.slot(), replacement.slot());
        assert_ne!(old.generation(), replacement.generation());
        assert_eq!(arena.get(old), Err(DomError::StaleNode));
        assert_eq!(arena.get(replacement), Ok(&"replacement"));
    }

    #[test]
    fn detached_nodes_live_only_while_javascript_retains_them() {
        let mut arena = NodeArena::new();
        let node = arena.insert(String::from("detached"));
        arena.retain_for_js(node).unwrap();

        assert!(!arena.detach_from_tree(node).unwrap());
        assert_eq!(arena.get(node).unwrap(), "detached");
        assert!(!arena.is_tree_owned(node).unwrap());
        assert_eq!(arena.js_reference_count(node).unwrap(), 1);

        assert!(arena.release_from_js(node).unwrap());
        assert_eq!(arena.get(node), Err(DomError::StaleNode));
    }

    #[test]
    fn explicitly_dropped_nodes_fail_cleanly_through_retained_handles() {
        let mut arena = NodeArena::new();
        let node = arena.insert(7);
        arena.retain_for_js(node).unwrap();

        assert_eq!(arena.destroy(node), Ok(7));
        assert_eq!(arena.get(node), Err(DomError::StaleNode));
        assert_eq!(arena.get_mut(node), Err(DomError::StaleNode));
        assert_eq!(arena.detach_from_tree(node), Err(DomError::StaleNode));
    }

    #[test]
    fn node_handles_have_a_stable_external_representation() {
        let node = NodeId::new(123, 456);
        assert_eq!(NodeId::from_u64(node.to_u64()), node);
    }

    #[test]
    fn invalidation_separates_style_and_propagated_layout_work() {
        let parents = [(3, 2), (2, 1)]
            .into_iter()
            .collect::<std::collections::HashMap<_, _>>();
        let mut dirty = InvalidationTracker::new(InvalidationMode::FineGrained);
        dirty.mark_style(4);
        dirty.mark_mutation(3, |node| parents.get(&node).copied());
        let frame = dirty.take_frame(100);

        assert_eq!(frame.style_nodes, HashSet::from([3, 4]));
        assert_eq!(frame.layout_nodes, HashSet::from([1, 2, 3]));
        assert_eq!(
            frame.metrics,
            InvalidationMetrics {
                restyled_nodes: 2,
                relaid_out_nodes: 3,
            }
        );
        assert!(!frame.full_document);
        assert!(!dirty.is_dirty());
    }

    #[test]
    fn full_layout_fallback_reports_its_true_frame_cost() {
        let mut dirty = InvalidationTracker::new(InvalidationMode::FullDocumentFallback);
        dirty.mark_style(9);
        let frame = dirty.take_frame(250);
        assert!(frame.full_document);
        assert_eq!(frame.metrics.restyled_nodes, 250);
        assert_eq!(frame.metrics.relaid_out_nodes, 250);
    }
}
