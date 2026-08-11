//! The concrete [`blitsen_dom::DomBackend`] implemented over Blitz.
//!
//! [`BlitzDom`] is the only place in Blitsen that translates renderer-neutral
//! DOM operations into Blitz calls. It owns one authoritative `HtmlDocument`;
//! no parallel tree or attribute store is maintained.

pub mod resources;
mod viewport;

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use blitsen_dom::{
    DomBackend, DomError, DomName, FrameInvalidation, HitTest, ImageState, InvalidationMetrics,
    InvalidationMode, InvalidationTracker, LayoutMetrics, LayoutSnapshot, NATIVE_VIEWPORT_TAG,
    Namespace, NodeKind, Rect, ViewportSurface,
};
use blitz::dom::node::ImageData;
use blitz::dom::{DocumentConfig, LocalName, NodeData, NodeId, QualName, ns};
use blitz::html::{HtmlDocument, HtmlProvider};
use kurbo::Point;
use style::computed_values::pointer_events::T as PointerEvents;
use style::computed_values::visibility::T as Visibility;
use style::values::computed::Overflow;

use resources::{ResourceLog, ResourceState};
use viewport::{NATIVE_VIEWPORT_UA_CSS, ViewportState, ViewportWidget};

/// Upper bound on resolve passes one layout flush will spend chasing resources.
///
/// A synchronous provider hands bytes back from inside `resolve`, after the
/// pass that would have consumed them. Without another pass a `background-image`
/// discovered during style resolution would first paint one frame late. Each
/// pass can only uncover resources referenced by the previous one, so the bound
/// caps a chain of `@import`ed stylesheets that each pull in the next.
const RESOURCE_RESOLVE_PASSES: usize = 4;

type HitCandidate = (Vec<i32>, usize, f32, f32);
type RankedHit = (Vec<i32>, usize, usize, NodeId, f32, f32);

fn compare_stacking_paths(left: &[i32], right: &[i32]) -> Ordering {
    (0..left.len().max(right.len()))
        .find_map(|index| {
            let ordering = left
                .get(index)
                .copied()
                .unwrap_or(0)
                .cmp(&right.get(index).copied().unwrap_or(0));
            (ordering != Ordering::Equal).then_some(ordering)
        })
        .unwrap_or(Ordering::Equal)
}

/// A Blitz HTML document exposed only through Blitsen's DOM boundary.
pub struct BlitzDom {
    document: HtmlDocument,
    revision: u64,
    flushed_revision: u64,
    invalidation: InvalidationTracker<NodeId>,
    last_invalidation_metrics: InvalidationMetrics,
    last_frame_was_full_document: bool,
    js_references: HashMap<NodeId, u32>,
    native_viewports: HashMap<NodeId, Rc<RefCell<ViewportState>>>,
    resources: ResourceLog,
}

impl BlitzDom {
    /// Parses an HTML document with the real Blitz fragment parser installed.
    ///
    /// The configured net provider is wrapped so subresource outcomes stay
    /// observable, and a configuration without one gets
    /// [`resources::LocalResources`] rather than Blitz's silent no-op provider.
    pub fn from_html(html: &str, mut config: DocumentConfig) -> Self {
        config.html_parser_provider = Some(Arc::new(HtmlProvider));
        let (provider, log) = resources::track(config.net_provider.take());
        config.net_provider = Some(provider);
        let mut dom = Self::new(HtmlDocument::from_html(html, config));
        dom.resources = log;
        dom
    }

    /// Wraps an existing Blitz document and installs the fragment parser.
    pub fn new(mut document: HtmlDocument) -> Self {
        document.set_html_parser_provider(Arc::new(HtmlProvider));
        document.add_user_agent_stylesheet(NATIVE_VIEWPORT_UA_CSS);
        let invalidation_mode = if document.incremental_layout() {
            InvalidationMode::FineGrained
        } else {
            InvalidationMode::FullDocumentFallback
        };
        Self {
            document,
            revision: 0,
            flushed_revision: u64::MAX,
            invalidation: InvalidationTracker::new(invalidation_mode),
            last_invalidation_metrics: InvalidationMetrics::default(),
            last_frame_was_full_document: false,
            js_references: HashMap::new(),
            native_viewports: HashMap::new(),
            resources: ResourceLog::default(),
        }
    }

    /// Returns the record of every subresource this document has requested.
    pub fn resources(&self) -> &ResourceLog {
        &self.resources
    }

    /// Borrows the authoritative Blitz document for painting or inspection.
    pub fn document_ref(&self) -> &HtmlDocument {
        &self.document
    }

    /// Mutably borrows the authoritative document for renderer integration.
    ///
    /// Callers must not mutate the tree through this escape hatch. DOM bridge
    /// writes go through [`DomBackend`] so revision tracking remains sound.
    pub fn document_mut(&mut self) -> &mut HtmlDocument {
        &mut self.document
    }

    /// Returns the owned Blitz document for transfer to the window renderer.
    pub fn into_document(self) -> HtmlDocument {
        self.document
    }

    /// Retains a node while a JavaScript wrapper is live.
    pub fn retain_for_js(&mut self, node: NodeId) -> Result<(), DomError> {
        self.node(node)?;
        let count = self.js_references.entry(node).or_default();
        *count = count
            .checked_add(1)
            .ok_or_else(|| DomError::Backend("JavaScript node reference count overflow".into()))?;
        Ok(())
    }

    /// Releases a JavaScript wrapper and collects an otherwise-unowned subtree.
    pub fn release_from_js(&mut self, node: NodeId) -> Result<bool, DomError> {
        self.node(node)?;
        let count = self
            .js_references
            .get_mut(&node)
            .ok_or_else(|| DomError::Backend("node has no JavaScript reference".into()))?;
        *count = count
            .checked_sub(1)
            .ok_or_else(|| DomError::Backend("node has no JavaScript reference".into()))?;
        if *count == 0 {
            self.js_references.remove(&node);
        }
        Ok(self.collect_detached_tree(node))
    }

    /// Drains observable invalidation work for the next frame.
    pub fn take_frame_invalidation(&mut self) -> FrameInvalidation<NodeId> {
        let frame = self.invalidation.take_frame(self.document.tree().len());
        self.last_invalidation_metrics = frame.metrics;
        self.last_frame_was_full_document = frame.full_document;
        frame
    }

    /// Returns the restyle/relayout scope consumed by the latest layout flush.
    pub fn last_frame_invalidation(&self) -> (InvalidationMetrics, bool) {
        (
            self.last_invalidation_metrics,
            self.last_frame_was_full_document,
        )
    }

    /// Gives every connected viewport element a surface, and forgets dead ones.
    ///
    /// Attaching is a tree mutation, so it runs before layout resolves: a
    /// surface installed afterwards would first paint against the layout of the
    /// frame that created it. A detached element keeps its surface, because a
    /// reparented viewport is the same viewport.
    fn attach_native_viewports(&mut self) -> Result<(), DomError> {
        for node in self.query_selector_all(self.document(), NATIVE_VIEWPORT_TAG)? {
            if self.native_viewports.contains_key(&node) {
                continue;
            }
            let state = Rc::new(RefCell::new(ViewportState::default()));
            let widget = ViewportWidget::new(Rc::clone(&state));
            self.document
                .mutate()
                .set_custom_widget(node, Box::new(widget));
            self.native_viewports.insert(node, state);
        }
        let dropped: Vec<NodeId> = self
            .native_viewports
            .keys()
            .copied()
            .filter(|node| self.document.get_node(*node).is_none())
            .collect();
        for node in dropped {
            self.native_viewports.remove(&node);
        }
        Ok(())
    }

    /// Propagates the resolved box and display density into each surface.
    fn resize_native_viewports(&mut self) {
        let scale = self.document.viewport().scale_f64();
        for (node, state) in &self.native_viewports {
            let Some(element) = self.document.get_node(*node) else {
                continue;
            };
            // Matches the size Blitz hands the widget when it paints, so the
            // buffer the application allocates is the buffer it is asked for.
            let size = element.final_layout().size;
            state.borrow_mut().resize(
                (f64::from(size.width) * scale) as u32,
                (f64::from(size.height) * scale) as u32,
                scale,
            );
        }
    }

    fn node(&self, node: NodeId) -> Result<&blitz::dom::Node, DomError> {
        self.document.get_node(node).ok_or(DomError::StaleNode)
    }

    fn hit_candidate(
        &self,
        target: NodeId,
        viewport_x: f32,
        viewport_y: f32,
    ) -> Result<Option<HitCandidate>, DomError> {
        let mut chain = vec![target];
        while let Some(parent) = self.parent(*chain.last().expect("target starts chain"))? {
            chain.push(parent);
        }
        chain.reverse();

        let mut x = viewport_x;
        let mut y = viewport_y;
        let mut stacking_path = Vec::new();
        let mut depth = 0;
        for (index, id) in chain.into_iter().enumerate() {
            let node = self.node(id)?;
            let Some(styles) = node.primary_styles() else {
                continue;
            };
            if matches!(
                styles.clone_visibility(),
                Visibility::Hidden | Visibility::Collapse
            ) {
                return Ok(None);
            }
            if index > 0 {
                let layout = node.final_layout();
                x = x - layout.location.x + node.scroll_offset().x as f32;
                y = y - layout.location.y + node.scroll_offset().y as f32;
                if let Some(transform) = *node.transform() {
                    let point = transform.inverse() * Point::new(f64::from(x), f64::from(y));
                    x = point.x as f32;
                    y = point.y as f32;
                }
                depth += 1;
            }
            if node.z_index() != 0 || node.is_stacking_context_root(false) {
                stacking_path.push(node.z_index());
            }

            let layout = node.final_layout();
            let inside_x = x >= 0.0 && x < layout.size.width;
            let inside_y = y >= 0.0 && y < layout.size.height;
            if let Some(styles) = node.primary_styles() {
                let clips_x = styles.clone_overflow_x() != Overflow::Visible;
                let clips_y = styles.clone_overflow_y() != Overflow::Visible;
                if (clips_x && !inside_x) || (clips_y && !inside_y) {
                    return Ok(None);
                }
            }
        }
        let target = self.node(target)?;
        let layout = target.final_layout();
        let inside = x >= 0.0 && x < layout.size.width && y >= 0.0 && y < layout.size.height;
        let interactive = target
            .primary_styles()
            .is_some_and(|styles| styles.clone_pointer_events() != PointerEvents::None);
        Ok((inside && interactive).then_some((stacking_path, depth, x, y)))
    }

    fn ensure_element(&self, node: NodeId) -> Result<(), DomError> {
        if self.node(node)?.element_data().is_some() {
            Ok(())
        } else {
            Err(DomError::InvalidNodeType)
        }
    }

    fn mutate(&mut self, style_node: Option<NodeId>, layout_node: Option<NodeId>) {
        self.revision = self.revision.wrapping_add(1);
        if let Some(node) = style_node {
            self.invalidation.mark_style(node);
        }
        if let Some(node) = layout_node {
            let parents = self.parent_chain(node);
            self.invalidation
                .mark_layout(node, |node| parents.get(&node).copied());
        }
    }

    fn parent_chain(&self, node: NodeId) -> HashMap<NodeId, NodeId> {
        let mut result = HashMap::new();
        let mut current = node;
        while let Some(parent) = self.document.get_node(current).and_then(|node| node.parent) {
            result.insert(current, parent);
            current = parent;
        }
        result
    }

    fn subtree_has_js_reference(&self, root: NodeId) -> bool {
        let Some(root) = self.document.get_node(root) else {
            return false;
        };
        self.js_references.contains_key(&root.id)
            || root
                .children
                .iter()
                .copied()
                .any(|child| self.subtree_has_js_reference(child))
    }

    fn detached_root(&self, mut node: NodeId) -> NodeId {
        while let Some(parent) = self.document.get_node(node).and_then(|node| node.parent) {
            node = parent;
        }
        node
    }

    fn collect_detached_tree(&mut self, node: NodeId) -> bool {
        let root = self.detached_root(node);
        if root == self.document.root_node().id
            || self.subtree_has_js_reference(root)
            || self.document.get_node(root).is_none()
        {
            return false;
        }
        self.document.mutate().remove_and_drop_node(root).is_some()
    }

    fn detach_children(&mut self, parent: NodeId) -> Result<(), DomError> {
        let children = self.node(parent)?.children.clone();
        for child in children {
            self.document.mutate().remove_node(child);
            self.collect_detached_tree(child);
        }
        Ok(())
    }

    fn check_no_cycle(&self, parent: NodeId, child: NodeId) -> Result<(), DomError> {
        let mut current = Some(parent);
        while let Some(node) = current {
            if node == child {
                return Err(DomError::HierarchyRequest);
            }
            current = self.node(node)?.parent;
        }
        Ok(())
    }

    fn qual_name(name: &DomName) -> QualName {
        let namespace = match &name.namespace {
            Namespace::Html => ns!(html),
            Namespace::Svg => ns!(svg),
            Namespace::MathMl => ns!(mathml),
            Namespace::None => ns!(),
            Namespace::Other(value) => value.clone().into(),
        };
        QualName::new(None, namespace, LocalName::from(name.local.clone()))
    }

    fn namespace(name: &QualName) -> Namespace {
        if name.ns == ns!(html) {
            Namespace::Html
        } else if name.ns == ns!(svg) {
            Namespace::Svg
        } else if name.ns == ns!(mathml) {
            Namespace::MathMl
        } else if name.ns == ns!() {
            Namespace::None
        } else {
            Namespace::Other(name.ns.to_string())
        }
    }

    fn style_text(&self, node: NodeId) -> Result<String, DomError> {
        let element = self
            .node(node)?
            .element_data()
            .ok_or(DomError::InvalidNodeType)?;
        let Some(style) = &element.style_attribute else {
            return Ok(String::new());
        };
        let guard = self.document.guard().read();
        let style = style.read_with(&guard);
        let mut css = String::new();
        style
            .to_css(&mut css)
            .map_err(|error| DomError::Backend(error.to_string()))?;
        Ok(css)
    }

    fn declarations(css: &str) -> Vec<(String, String)> {
        css.split(';')
            .filter_map(|declaration| declaration.split_once(':'))
            .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
            .filter(|(name, _)| !name.is_empty())
            .collect()
    }

    fn serialize_node(
        &self,
        node: NodeId,
        output: &mut String,
        raw_text: bool,
    ) -> Result<(), DomError> {
        let node = self.node(node)?;
        match &node.data {
            NodeData::Document(_) | NodeData::AnonymousBlock(_) => {
                for child in &node.children {
                    self.serialize_node(*child, output, false)?;
                }
            }
            NodeData::Text(text) => {
                if raw_text {
                    output.push_str(&text.content);
                } else {
                    output.push_str(&html_escape::encode_text(&text.content));
                }
            }
            NodeData::Comment { contents } => {
                output.push_str("<!--");
                output.push_str(contents);
                output.push_str("-->");
            }
            NodeData::Element(element) => {
                let tag = element.name.local.as_ref();
                output.push('<');
                output.push_str(tag);
                for attribute in element.attrs() {
                    output.push(' ');
                    output.push_str(&attribute.name.local);
                    output.push_str("=\"");
                    output.push_str(&html_escape::encode_double_quoted_attribute(
                        &attribute.value,
                    ));
                    output.push('"');
                }
                output.push('>');
                let is_html = element.name.ns == ns!(html);
                let is_void = is_html
                    && matches!(
                        tag,
                        "area"
                            | "base"
                            | "br"
                            | "col"
                            | "embed"
                            | "hr"
                            | "img"
                            | "input"
                            | "link"
                            | "meta"
                            | "param"
                            | "source"
                            | "track"
                            | "wbr"
                    );
                if !is_void {
                    let children_are_raw = is_html && matches!(tag, "script" | "style");
                    for child in &node.children {
                        self.serialize_node(*child, output, children_are_raw)?;
                    }
                    output.push_str("</");
                    output.push_str(tag);
                    output.push('>');
                }
            }
        }
        Ok(())
    }
}

impl DomBackend for BlitzDom {
    type NodeId = NodeId;

    fn document(&self) -> NodeId {
        self.document.root_node().id
    }

    fn document_element(&self) -> Option<NodeId> {
        self.document.try_root_element().map(|node| node.id)
    }

    fn body(&self) -> Option<NodeId> {
        self.document.query_selector("body").ok().flatten()
    }

    fn node_kind(&self, node: NodeId) -> Result<NodeKind, DomError> {
        match &self.node(node)?.data {
            NodeData::Document(_) => Ok(NodeKind::Document),
            NodeData::Element(_) | NodeData::AnonymousBlock(_) => Ok(NodeKind::Element),
            NodeData::Text(_) => Ok(NodeKind::Text),
            NodeData::Comment { .. } => Ok(NodeKind::Comment),
        }
    }

    fn element_name(&self, node: NodeId) -> Result<DomName, DomError> {
        let name = &self
            .node(node)?
            .element_data()
            .ok_or(DomError::InvalidNodeType)?
            .name;
        Ok(DomName {
            namespace: Self::namespace(name),
            local: name.local.to_string(),
        })
    }

    fn create_element(&mut self, name: &DomName) -> Result<NodeId, DomError> {
        Ok(self
            .document
            .mutate()
            .create_element(Self::qual_name(name), vec![]))
    }

    fn create_text(&mut self, text: &str) -> Result<NodeId, DomError> {
        Ok(self.document.mutate().create_text_node(text))
    }

    fn append_child(&mut self, parent: NodeId, child: NodeId) -> Result<(), DomError> {
        self.ensure_element(parent)?;
        self.node(child)?;
        self.check_no_cycle(parent, child)?;
        self.document.mutate().append_children(parent, &[child]);
        self.mutate(Some(parent), Some(parent));
        Ok(())
    }

    fn insert_before(
        &mut self,
        parent: NodeId,
        child: NodeId,
        reference: Option<NodeId>,
    ) -> Result<(), DomError> {
        self.ensure_element(parent)?;
        self.node(child)?;
        self.check_no_cycle(parent, child)?;
        let Some(reference) = reference else {
            return self.append_child(parent, child);
        };
        if self.node(reference)?.parent != Some(parent) {
            return Err(DomError::NotFound);
        }
        self.document
            .mutate()
            .insert_nodes_before(reference, &[child]);
        self.mutate(Some(parent), Some(parent));
        Ok(())
    }

    fn remove(&mut self, node: NodeId) -> Result<(), DomError> {
        let parent = self.node(node)?.parent.ok_or(DomError::NotFound)?;
        self.document.mutate().remove_node(node);
        self.collect_detached_tree(node);
        self.mutate(Some(parent), Some(parent));
        Ok(())
    }

    fn replace(&mut self, old: NodeId, replacement: NodeId) -> Result<(), DomError> {
        let parent = self.node(old)?.parent.ok_or(DomError::NotFound)?;
        self.node(replacement)?;
        self.check_no_cycle(parent, replacement)?;
        self.document
            .mutate()
            .replace_node_with(old, &[replacement]);
        self.collect_detached_tree(old);
        self.mutate(Some(parent), Some(parent));
        Ok(())
    }

    fn parent(&self, node: NodeId) -> Result<Option<NodeId>, DomError> {
        Ok(self.node(node)?.parent)
    }

    fn children(&self, node: NodeId) -> Result<Vec<NodeId>, DomError> {
        Ok(self.node(node)?.children.to_vec())
    }

    fn previous_sibling(&self, node: NodeId) -> Result<Option<NodeId>, DomError> {
        let node = self.node(node)?;
        let Some(parent) = node.parent else {
            return Ok(None);
        };
        let siblings = &self.node(parent)?.children;
        let index = siblings
            .iter()
            .position(|candidate| *candidate == node.id)
            .ok_or(DomError::NotFound)?;
        Ok(index
            .checked_sub(1)
            .and_then(|index| siblings.get(index))
            .copied())
    }

    fn next_sibling(&self, node: NodeId) -> Result<Option<NodeId>, DomError> {
        let node = self.node(node)?;
        let Some(parent) = node.parent else {
            return Ok(None);
        };
        let siblings = &self.node(parent)?.children;
        let index = siblings
            .iter()
            .position(|candidate| *candidate == node.id)
            .ok_or(DomError::NotFound)?;
        Ok(siblings.get(index + 1).copied())
    }

    fn is_connected(&self, node: NodeId) -> Result<bool, DomError> {
        let root = self.document();
        let mut current = Some(node);
        while let Some(node) = current {
            if node == root {
                return Ok(true);
            }
            current = self.node(node)?.parent;
        }
        Ok(false)
    }

    fn attribute(&self, node: NodeId, name: &DomName) -> Result<Option<String>, DomError> {
        let name = Self::qual_name(name);
        let element = self
            .node(node)?
            .element_data()
            .ok_or(DomError::InvalidNodeType)?;
        Ok(element
            .attrs()
            .iter()
            .find(|attribute| attribute.name == name)
            .map(|attribute| attribute.value.to_string()))
    }

    fn set_attribute(&mut self, node: NodeId, name: &DomName, value: &str) -> Result<(), DomError> {
        self.ensure_element(node)?;
        self.document
            .mutate()
            .set_attribute(node, Self::qual_name(name), value);
        self.mutate(Some(node), Some(node));
        Ok(())
    }

    fn remove_attribute(&mut self, node: NodeId, name: &DomName) -> Result<bool, DomError> {
        let existed = self.attribute(node, name)?.is_some();
        if existed {
            self.document
                .mutate()
                .clear_attribute(node, Self::qual_name(name));
            self.mutate(Some(node), Some(node));
        }
        Ok(existed)
    }

    fn inline_style(&self, node: NodeId, property: &str) -> Result<Option<String>, DomError> {
        Ok(Self::declarations(&self.style_text(node)?)
            .into_iter()
            .find(|(name, _)| name == property)
            .map(|(_, value)| value))
    }

    fn set_inline_style(
        &mut self,
        node: NodeId,
        property: &str,
        value: &str,
    ) -> Result<bool, DomError> {
        self.ensure_element(node)?;
        let original = self.style_text(node)?;
        let mut declarations = Self::declarations(&original);
        declarations.retain(|(name, _)| name != property);
        declarations.push((property.to_owned(), value.to_owned()));
        let candidate = declarations
            .iter()
            .map(|(name, value)| format!("{name}: {value};"))
            .collect::<Vec<_>>()
            .join(" ");
        self.document.mutate().set_attribute(
            node,
            Self::qual_name(&DomName::attribute("style")),
            &candidate,
        );
        let valid = self.inline_style(node, property)?.is_some();
        if valid {
            self.mutate(Some(node), Some(node));
        } else {
            self.document.mutate().set_attribute(
                node,
                Self::qual_name(&DomName::attribute("style")),
                &original,
            );
        }
        Ok(valid)
    }

    fn remove_inline_style(
        &mut self,
        node: NodeId,
        property: &str,
    ) -> Result<Option<String>, DomError> {
        self.ensure_element(node)?;
        let old = self.inline_style(node, property)?;
        if old.is_none() {
            return Ok(None);
        }
        let css = Self::declarations(&self.style_text(node)?)
            .into_iter()
            .filter(|(name, _)| name != property)
            .map(|(name, value)| format!("{name}: {value};"))
            .collect::<Vec<_>>()
            .join(" ");
        self.document.mutate().set_attribute(
            node,
            Self::qual_name(&DomName::attribute("style")),
            &css,
        );
        self.mutate(Some(node), Some(node));
        Ok(old)
    }

    fn inline_style_text(&self, node: NodeId) -> Result<String, DomError> {
        self.style_text(node)
    }

    fn set_inline_style_text(&mut self, node: NodeId, css: &str) -> Result<(), DomError> {
        self.set_attribute(node, &DomName::attribute("style"), css)
    }

    fn text_content(&self, node: NodeId) -> Result<String, DomError> {
        Ok(self.node(node)?.text_content())
    }

    fn set_text_content(&mut self, node: NodeId, text: &str) -> Result<(), DomError> {
        match self.node_kind(node)? {
            NodeKind::Text => self.document.mutate().set_node_text(node, text),
            NodeKind::Element | NodeKind::Document | NodeKind::Fragment => {
                self.detach_children(node)?;
                if !text.is_empty() {
                    let text = self.document.mutate().create_text_node(text);
                    self.document.mutate().append_children(node, &[text]);
                }
            }
            NodeKind::Comment => return Err(DomError::InvalidNodeType),
        }
        self.mutate(Some(node), Some(node));
        Ok(())
    }

    fn parse_fragment(&mut self, context: NodeId, html: &str) -> Result<Vec<NodeId>, DomError> {
        let name = self.element_name(context)?;
        let temporary = self
            .document
            .mutate()
            .create_element(Self::qual_name(&name), vec![]);
        self.document.mutate().set_inner_html(temporary, html);
        let children = self.node(temporary)?.children.clone().to_vec();
        for child in &children {
            self.document.mutate().remove_node(*child);
        }
        self.document.mutate().remove_and_drop_node(temporary);
        Ok(children)
    }

    fn inner_html(&self, node: NodeId) -> Result<String, DomError> {
        let mut html = String::new();
        for child in &self.node(node)?.children {
            self.serialize_node(*child, &mut html, false)?;
        }
        Ok(html)
    }

    fn set_inner_html(&mut self, node: NodeId, html: &str) -> Result<(), DomError> {
        self.ensure_element(node)?;
        self.detach_children(node)?;
        self.document.mutate().set_inner_html(node, html);
        self.mutate(Some(node), Some(node));
        Ok(())
    }

    fn query_selector(&self, root: NodeId, selector: &str) -> Result<Option<NodeId>, DomError> {
        self.node(root)?;
        self.document
            .query_selector_in(root, selector)
            .map_err(|error| DomError::Syntax(format!("{error:?}")))
    }

    fn query_selector_all(&self, root: NodeId, selector: &str) -> Result<Vec<NodeId>, DomError> {
        self.node(root)?;
        self.document
            .query_selector_all_in(root, selector)
            .map(|nodes| nodes.to_vec())
            .map_err(|error| DomError::Syntax(format!("{error:?}")))
    }

    fn get_element_by_id(&self, id: &str) -> Result<Option<NodeId>, DomError> {
        let attribute = DomName::attribute("id");
        Ok(self
            .query_selector_all(self.document(), "*")?
            .into_iter()
            .find(|node| self.attribute(*node, &attribute).ok().flatten().as_deref() == Some(id)))
    }

    fn flush_layout(&mut self) -> Result<LayoutSnapshot, DomError> {
        self.take_frame_invalidation();
        self.attach_native_viewports()?;
        for _ in 0..RESOURCE_RESOLVE_PASSES {
            let settled = self.resources.settlements();
            self.document.resolve(0.0);
            if self.resources.settlements() == settled {
                break;
            }
        }
        self.resize_native_viewports();
        self.flushed_revision = self.revision;
        Ok(LayoutSnapshot::new(self.revision))
    }

    fn layout_is_dirty(&self) -> bool {
        self.flushed_revision != self.revision
    }

    fn bounding_rect(&self, node: NodeId, snapshot: LayoutSnapshot) -> Result<Rect, DomError> {
        if snapshot.revision() != self.revision || self.flushed_revision != self.revision {
            return Err(DomError::LayoutNotFlushed);
        }
        let element = self.node(node)?;
        let rect = if let Some(rect) = self.document.get_client_bounding_rect(node) {
            Rect {
                x: rect.x as f32,
                y: rect.y as f32,
                width: rect.width as f32,
                height: rect.height as f32,
            }
        } else {
            let position = element.absolute_position(0.0, 0.0);
            let layout = element.unrounded_layout();
            Rect {
                x: position.x - self.document.viewport_scroll().x as f32,
                y: position.y - self.document.viewport_scroll().y as f32,
                width: layout.size.width,
                height: layout.size.height,
            }
        };
        Ok(rect)
    }

    fn layout_metrics(
        &self,
        node: NodeId,
        snapshot: LayoutSnapshot,
    ) -> Result<LayoutMetrics, DomError> {
        let rect = self.bounding_rect(node, snapshot)?;
        let element = self.node(node)?;
        let layout = element.unrounded_layout();
        let scroll = if self
            .document
            .try_root_element()
            .is_some_and(|root| root.id == node)
        {
            self.document.viewport_scroll()
        } else {
            *element.scroll_offset()
        };
        Ok(LayoutMetrics {
            rect,
            offset_width: f64::from(layout.size.width.round()),
            offset_height: f64::from(layout.size.height.round()),
            client_width: f64::from(
                (layout.size.width
                    - layout.border.left
                    - layout.border.right
                    - layout.scrollbar_size.width)
                    .max(0.0)
                    .round(),
            ),
            client_height: f64::from(
                (layout.size.height
                    - layout.border.top
                    - layout.border.bottom
                    - layout.scrollbar_size.height)
                    .max(0.0)
                    .round(),
            ),
            scroll_left: scroll.x,
            scroll_top: scroll.y,
        })
    }

    fn set_scroll_offset(
        &mut self,
        node: NodeId,
        left: Option<f64>,
        top: Option<f64>,
        snapshot: LayoutSnapshot,
    ) -> Result<(), DomError> {
        if snapshot.revision() != self.revision || self.flushed_revision != self.revision {
            return Err(DomError::LayoutNotFlushed);
        }
        if self
            .document
            .try_root_element()
            .is_some_and(|root| root.id == node)
        {
            let current = self.document.viewport_scroll();
            let desired_x = left.unwrap_or(current.x);
            let desired_y = top.unwrap_or(current.y);
            self.document
                .scroll_viewport_by(current.x - desired_x, current.y - desired_y);
            return Ok(());
        }

        let element = self
            .document
            .get_node_mut(node)
            .ok_or(DomError::StaleNode)?;
        let layout = element.final_layout();
        let max_x = f64::from(layout.scroll_width());
        let max_y = f64::from(layout.scroll_height());
        let (can_scroll_x, can_scroll_y) = element
            .primary_styles()
            .map(|styles| {
                (
                    matches!(
                        styles.clone_overflow_x(),
                        Overflow::Hidden | Overflow::Scroll | Overflow::Auto
                    ),
                    matches!(
                        styles.clone_overflow_y(),
                        Overflow::Hidden | Overflow::Scroll | Overflow::Auto
                    ),
                )
            })
            .unwrap_or_default();
        if let Some(left) = left {
            element.scroll_offset_mut().x = if can_scroll_x {
                left.clamp(0.0, max_x)
            } else {
                0.0
            };
        }
        if let Some(top) = top {
            element.scroll_offset_mut().y = if can_scroll_y {
                top.clamp(0.0, max_y)
            } else {
                0.0
            };
        }
        Ok(())
    }

    fn hit_test(
        &self,
        x: f32,
        y: f32,
        snapshot: LayoutSnapshot,
    ) -> Result<Option<HitTest<NodeId>>, DomError> {
        if snapshot.revision() != self.revision || self.flushed_revision != self.revision {
            return Err(DomError::LayoutNotFlushed);
        }
        let mut best: Option<RankedHit> = None;
        for (order, node) in self
            .query_selector_all(self.document(), "*")?
            .into_iter()
            .enumerate()
        {
            let Some((stacking_path, depth, offset_x, offset_y)) =
                self.hit_candidate(node, x, y)?
            else {
                continue;
            };
            let candidate = (stacking_path, order, depth, node, offset_x, offset_y);
            if best.as_ref().is_none_or(|current| {
                compare_stacking_paths(&candidate.0, &current.0)
                    .then_with(|| candidate.1.cmp(&current.1))
                    .then_with(|| candidate.2.cmp(&current.2))
                    == Ordering::Greater
            }) {
                best = Some(candidate);
            }
        }
        let Some((_, _, _, target, offset_x, offset_y)) = best else {
            return Ok(None);
        };
        let mut path = vec![target];
        while let Some(parent) = self.parent(*path.last().expect("target starts path"))? {
            path.push(parent);
        }
        path.reverse();
        Ok(Some(HitTest {
            target,
            path,
            offset_x,
            offset_y,
        }))
    }

    fn image_state(&self, node: NodeId, snapshot: LayoutSnapshot) -> Result<ImageState, DomError> {
        if snapshot.revision() != self.revision || self.flushed_revision != self.revision {
            return Err(DomError::LayoutNotFlushed);
        }
        let element = self
            .node(node)?
            .element_data()
            .ok_or(DomError::InvalidNodeType)?;
        if element.name.local.as_ref() != "img" {
            return Err(DomError::InvalidNodeType);
        }
        if let Some(ImageData::Raster(raster)) = element.image_data() {
            return Ok(ImageState::decoded(raster.width, raster.height));
        }
        let Some(source) = self
            .attribute(node, &DomName::attribute("src"))?
            .filter(|source| !source.is_empty())
        else {
            return Ok(ImageState::IDLE);
        };
        let state = self
            .document
            .url()
            .join(&source)
            .map(|url| self.resources.state(url.as_str()));
        Ok(match state {
            // Bytes arrived but no image came out of them, so the decode failed.
            Ok(Some(ResourceState::Loaded | ResourceState::Failed)) => ImageState::FAILED,
            Ok(Some(ResourceState::Loading)) => ImageState::LOADING,
            // A source that cannot become a URL will never be requested, so
            // waiting on it would be waiting forever.
            Err(_) => ImageState::FAILED,
            // Requested by nothing yet: an element whose source was set during
            // the flush that is reading it back.
            Ok(None) => ImageState::LOADING,
        })
    }

    fn native_viewports(&self) -> Result<Vec<NodeId>, DomError> {
        self.query_selector_all(self.document(), NATIVE_VIEWPORT_TAG)
    }

    fn native_viewport_surface(
        &self,
        node: NodeId,
        snapshot: LayoutSnapshot,
    ) -> Result<ViewportSurface, DomError> {
        let rect = self.bounding_rect(node, snapshot)?;
        let state = self
            .native_viewports
            .get(&node)
            .ok_or(DomError::InvalidNodeType)?
            .borrow();
        let (width, height) = state.size();
        Ok(ViewportSurface {
            rect,
            width,
            height,
            device_pixel_ratio: state.device_pixel_ratio(),
            generation: state.generation(),
        })
    }

    fn write_native_viewport(&mut self, node: NodeId, pixels: &[u8]) -> Result<(), DomError> {
        self.native_viewports
            .get(&node)
            .ok_or(DomError::InvalidNodeType)?
            .borrow_mut()
            .write(pixels)
    }
}

#[cfg(test)]
mod tests {
    use anyrender::recording::RenderCommand;
    use anyrender::{Paint, Scene};
    use anyrender_vello_cpu::VelloCpuImageRenderer;
    use std::sync::{Arc, Mutex};

    use blitsen_dom::{DomBackend, DomError, DomName, ImageState, NodeKind};
    use blitz::dom::DocumentConfig;
    use blitz::traits::net::{NetHandler, NetProvider, Request};
    use blitz::traits::shell::{ColorScheme, Viewport};
    use kurbo::{BezPath, Point, Shape as _};

    use super::BlitzDom;
    use super::resources::LocalResources;

    /// Base URL of the checked-in subresource fixtures.
    ///
    /// `file:` keeps the tests on the synchronous provider, which is the same
    /// path a headless harness takes.
    fn fixtures_url() -> String {
        format!("file://{}/fixtures/", env!("CARGO_MANIFEST_DIR"))
    }

    /// Renders a document and returns straight-alpha RGBA8 rows.
    fn render(dom: &mut BlitzDom, width: u32, height: u32) -> Vec<u8> {
        anyrender::render_to_buffer::<VelloCpuImageRenderer, _>(
            |scene| {
                blitz_paint::paint_scene(
                    scene,
                    dom.document_mut().as_mut(),
                    1.0,
                    width,
                    height,
                    0,
                    0,
                );
            },
            width,
            height,
        )
    }

    fn pixel(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let start = ((y * width + x) * 4) as usize;
        pixels[start..start + 4].try_into().expect("rgba8 pixel")
    }

    /// Bounding box `(x, y, width, height)` of everything the frame painted.
    ///
    /// The fixture font draws each letter as a solid em block, so the box of a
    /// text run is the run's exact metrics — which is what makes "did the web
    /// font actually get used" answerable from pixels alone.
    fn inked_bounds(pixels: &[u8], width: u32) -> Option<(u32, u32, u32, u32)> {
        let inked = pixels
            .chunks_exact(4)
            .enumerate()
            .filter(|(_, pixel)| pixel[3] > 0)
            .map(|(index, _)| (index as u32 % width, index as u32 / width));
        inked
            .fold(None, |bounds: Option<[u32; 4]>, (x, y)| {
                Some(match bounds {
                    Some([left, top, right, bottom]) => {
                        [left.min(x), top.min(y), right.max(x), bottom.max(y)]
                    }
                    None => [x, y, x, y],
                })
            })
            .map(|[left, top, right, bottom]| (left, top, right - left + 1, bottom - top + 1))
    }

    /// A provider that answers nothing until it is told to.
    ///
    /// Every other subresource in these tests resolves before `fetch` returns,
    /// which is precisely the case where an in-flight state can never be
    /// observed. Deferring reinstates the asynchrony a real window has.
    type HeldRequest = (Request, Box<dyn NetHandler>);

    #[derive(Clone, Default)]
    struct DeferredResources(Arc<Mutex<Vec<HeldRequest>>>);

    impl NetProvider for DeferredResources {
        fn fetch(&self, _doc_id: usize, request: Request, handler: Box<dyn NetHandler>) {
            self.0
                .lock()
                .expect("deferred requests")
                .push((request, handler));
        }
    }

    impl DeferredResources {
        fn deliver(&self) {
            let held = self
                .0
                .lock()
                .expect("deferred requests")
                .drain(..)
                .collect::<Vec<_>>();
            for (request, handler) in held {
                LocalResources.fetch(0, request, handler);
            }
        }
    }

    /// A document whose relative URLs resolve against the checked-in fixtures.
    fn fixture_document(html: &str, provider: Option<Arc<dyn NetProvider>>) -> BlitzDom {
        BlitzDom::from_html(
            &format!("<style>html, body {{ margin: 0 }}</style>{html}"),
            DocumentConfig {
                base_url: Some(fixtures_url()),
                net_provider: provider,
                viewport: Some(Viewport::new(400, 200, 1.0, ColorScheme::Light)),
                ..Default::default()
            },
        )
    }

    fn backend() -> BlitzDom {
        BlitzDom::from_html(
            r#"<style>.wide { width: 240px }</style><body><main id="host"><p id="x">old</p></main></body>"#,
            DocumentConfig {
                viewport: Some(Viewport::new(800, 600, 1.0, ColorScheme::Light)),
                ..Default::default()
            },
        )
    }

    #[test]
    fn implements_the_complete_boundary_over_one_blitz_tree() {
        let mut dom = backend();
        let document = dom.document();
        let html = dom.document_element().unwrap();
        let body = dom.body().unwrap();
        let host = dom.get_element_by_id("host").unwrap().unwrap();
        let old = dom.get_element_by_id("x").unwrap().unwrap();
        assert_eq!(dom.node_kind(document), Ok(NodeKind::Document));
        assert_eq!(dom.element_name(html).unwrap().local, "html");
        assert!(dom.is_connected(old).unwrap());

        let replacement = dom.create_element(&DomName::html("section")).unwrap();
        dom.set_attribute(replacement, &DomName::attribute("id"), "replacement")
            .unwrap();
        dom.set_text_content(replacement, "hello").unwrap();
        dom.insert_before(host, replacement, Some(old)).unwrap();
        assert_eq!(dom.previous_sibling(old).unwrap(), Some(replacement));
        assert_eq!(dom.next_sibling(replacement).unwrap(), Some(old));
        assert_eq!(dom.parent(replacement).unwrap(), Some(host));

        assert!(dom.set_inline_style(replacement, "width", "120px").unwrap());
        assert!(
            !dom.set_inline_style(replacement, "width", "invalid")
                .unwrap()
        );
        assert_eq!(
            dom.inline_style(replacement, "width").unwrap().as_deref(),
            Some("120px")
        );
        dom.set_inner_html(replacement, "<b>A</b><i>B</i>").unwrap();
        assert_eq!(dom.text_content(replacement).unwrap(), "AB");
        assert!(dom.inner_html(replacement).unwrap().contains("<b>"));
        assert_eq!(
            dom.remove_inline_style(replacement, "width")
                .unwrap()
                .as_deref(),
            Some("120px")
        );

        let snapshot = dom.flush_layout().unwrap();
        assert!(dom.bounding_rect(replacement, snapshot).unwrap().width > 0.0);
        assert!(dom.hit_test(1.0, 1.0, snapshot).unwrap().is_some());
        dom.set_attribute(replacement, &DomName::attribute("class"), "wide")
            .unwrap();
        assert_eq!(
            dom.bounding_rect(replacement, snapshot),
            Err(DomError::LayoutNotFlushed)
        );
        let snapshot = dom.flush_layout().unwrap();
        assert_eq!(
            dom.bounding_rect(replacement, snapshot).unwrap().width,
            240.0
        );
        let (metrics, full_document) = dom.last_frame_invalidation();
        assert!(metrics.restyled_nodes > 0);
        assert!(metrics.relaid_out_nodes >= metrics.restyled_nodes);
        assert!(!full_document);
        dom.flush_layout().unwrap();
        assert_eq!(
            dom.last_frame_invalidation(),
            (blitsen_dom::InvalidationMetrics::default(), false)
        );

        dom.append_child(body, replacement).unwrap();
        assert_eq!(dom.parent(replacement).unwrap(), Some(body));
    }

    #[test]
    fn detached_nodes_follow_javascript_wrapper_lifetime() {
        let mut dom = backend();
        let node = dom.get_element_by_id("x").unwrap().unwrap();
        dom.retain_for_js(node).unwrap();
        dom.remove(node).unwrap();
        assert_eq!(dom.text_content(node).unwrap(), "old");
        assert!(!dom.is_connected(node).unwrap());
        assert!(dom.release_from_js(node).unwrap());
        assert_eq!(dom.text_content(node), Err(DomError::StaleNode));
    }

    #[test]
    fn fragment_parsing_adopts_real_contextual_nodes() {
        let mut dom = backend();
        let host = dom.get_element_by_id("host").unwrap().unwrap();
        let nodes = dom
            .parse_fragment(host, "<span id=one>one</span><span>two</span>")
            .unwrap();
        assert_eq!(nodes.len(), 2);
        dom.append_child(host, nodes[0]).unwrap();
        assert_eq!(dom.get_element_by_id("one").unwrap(), Some(nodes[0]));
    }

    #[test]
    fn reports_the_real_full_document_fallback_mode() {
        let mut dom = BlitzDom::from_html(
            "<body><main id='host'><p>child</p></main></body>",
            DocumentConfig {
                incremental: Some(false),
                ..Default::default()
            },
        );
        let host = dom.get_element_by_id("host").unwrap().unwrap();
        dom.set_attribute(host, &DomName::attribute("class"), "changed")
            .unwrap();
        dom.flush_layout().unwrap();
        let (metrics, full_document) = dom.last_frame_invalidation();
        assert!(full_document);
        assert_eq!(metrics.restyled_nodes, dom.document_ref().tree().len());
        assert_eq!(metrics.relaid_out_nodes, metrics.restyled_nodes);
    }

    #[test]
    fn hit_testing_returns_paint_order_transforms_clipping_and_the_dom_path() {
        let mut dom = BlitzDom::from_html(
            r#"
            <style>
              html, body { margin: 0; width: 400px; height: 300px }
              .box { position: absolute; width: 100px; height: 100px }
              #low { left: 0; top: 0 }
              #high { left: 20px; top: 20px; z-index: 2 }
              #high-child { width: 100%; height: 100% }
              #transparent { left: 20px; top: 20px; z-index: 3; pointer-events: none }
              #transformed { left: 150px; top: 0; transform: translateX(40px) }
              #clip { left: 0; top: 150px; width: 40px; height: 40px; overflow: hidden }
              #outside { position: absolute; left: 60px; top: 0; width: 20px; height: 20px }
              #nested-low { left: 250px; top: 150px; z-index: 1 }
              #nested-child { width: 100%; height: 100%; position: relative; z-index: 100 }
              #nested-high { left: 250px; top: 150px; z-index: 2 }
            </style>
            <body>
              <div id="low" class="box"></div>
              <div id="high" class="box"><div id="high-child"></div></div>
              <div id="transparent" class="box"></div>
              <div id="transformed" class="box"></div>
              <div id="clip" class="box"><div id="outside"></div></div>
              <div id="nested-low" class="box"><div id="nested-child"></div></div>
              <div id="nested-high" class="box"></div>
            </body>
            "#,
            DocumentConfig {
                viewport: Some(Viewport::new(400, 300, 1.0, ColorScheme::Light)),
                ..Default::default()
            },
        );
        let snapshot = dom.flush_layout().unwrap();
        let document = dom.document();
        let body = dom.body().unwrap();
        let high = dom.get_element_by_id("high-child").unwrap().unwrap();
        let transformed = dom.get_element_by_id("transformed").unwrap().unwrap();

        let overlap = dom.hit_test(30.0, 30.0, snapshot).unwrap().unwrap();
        assert_eq!(overlap.target, high);
        assert_eq!(overlap.path.first(), Some(&document));
        assert_eq!(overlap.path.last(), Some(&high));

        let transformed_hit = dom.hit_test(195.0, 10.0, snapshot).unwrap().unwrap();
        assert_eq!(transformed_hit.target, transformed);

        let clipped = dom.hit_test(65.0, 160.0, snapshot).unwrap().unwrap();
        assert_eq!(clipped.target, body);
        assert_eq!(
            clipped.path,
            vec![document, dom.document_element().unwrap(), body]
        );

        let nested_high = dom.get_element_by_id("nested-high").unwrap().unwrap();
        assert_eq!(
            dom.hit_test(260.0, 160.0, snapshot)
                .unwrap()
                .unwrap()
                .target,
            nested_high,
            "a child cannot escape its ancestor's lower stacking context"
        );
    }

    fn viewport_document(body: &str, scale: f32) -> BlitzDom {
        BlitzDom::from_html(
            &format!("<style>html, body {{ margin: 0 }}</style><body>{body}</body>"),
            DocumentConfig {
                viewport: Some(Viewport::new(400, 300, scale, ColorScheme::Light)),
                ..Default::default()
            },
        )
    }

    #[test]
    fn viewport_elements_are_replaced_boxes_with_a_physical_pixel_surface() {
        let mut dom = viewport_document(
            r#"<blitsen-view id="default"></blitsen-view>
               <blitsen-view id="sized" style="width: 80px; height: 40px"><b>fallback</b></blitsen-view>"#,
            2.0,
        );
        let snapshot = dom.flush_layout().unwrap();
        let default = dom.get_element_by_id("default").unwrap().unwrap();
        let sized = dom.get_element_by_id("sized").unwrap().unwrap();
        assert_eq!(dom.native_viewports().unwrap(), vec![default, sized]);

        let unsized_surface = dom.native_viewport_surface(default, snapshot).unwrap();
        assert_eq!(
            (unsized_surface.rect.width, unsized_surface.rect.height),
            (300.0, 150.0),
            "an unsized viewport uses the default object size"
        );
        assert_eq!((unsized_surface.width, unsized_surface.height), (600, 300));
        assert_eq!(unsized_surface.device_pixel_ratio, 2.0);
        assert_eq!(unsized_surface.byte_length(), 600 * 300 * 4);

        let sized_surface = dom.native_viewport_surface(sized, snapshot).unwrap();
        assert_eq!(
            (sized_surface.rect.width, sized_surface.rect.height),
            (80.0, 40.0)
        );
        assert_eq!((sized_surface.width, sized_surface.height), (160, 80));
        assert_eq!(
            sized_surface.rect.y, 150.0,
            "a viewport is a block box that displaces the ones after it"
        );

        let body = dom.body().unwrap();
        assert_eq!(
            dom.native_viewport_surface(body, snapshot),
            Err(DomError::InvalidNodeType),
            "only viewport elements have a surface"
        );

        let created = dom.create_element(&DomName::html("blitsen-view")).unwrap();
        assert_eq!(
            dom.native_viewport_surface(created, snapshot),
            Err(DomError::InvalidNodeType),
            "a detached viewport has no box and therefore no surface"
        );
        dom.append_child(body, created).unwrap();
        let snapshot = dom.flush_layout().unwrap();
        assert_eq!(
            dom.native_viewport_surface(created, snapshot)
                .unwrap()
                .width,
            600,
            "a scripted viewport gets its surface at the next layout flush"
        );
    }

    #[test]
    fn viewport_surfaces_follow_resize_and_display_density() {
        let mut dom = viewport_document(
            r#"<blitsen-view id="view" style="width: 100px; height: 50px"></blitsen-view>"#,
            1.0,
        );
        let snapshot = dom.flush_layout().unwrap();
        let view = dom.get_element_by_id("view").unwrap().unwrap();
        let first = dom.native_viewport_surface(view, snapshot).unwrap();
        assert_eq!((first.width, first.height), (100, 50));

        dom.flush_layout().unwrap();
        let snapshot = dom.flush_layout().unwrap();
        assert_eq!(
            dom.native_viewport_surface(view, snapshot).unwrap(),
            first,
            "a frame that changes nothing does not invalidate the surface"
        );

        dom.set_inline_style(view, "width", "120px").unwrap();
        let snapshot = dom.flush_layout().unwrap();
        let resized = dom.native_viewport_surface(view, snapshot).unwrap();
        assert_eq!((resized.width, resized.height), (120, 50));
        assert_eq!(resized.generation, first.generation + 1);

        let mut viewport = dom.document_ref().viewport().clone();
        viewport.set_hidpi_scale(3.0);
        dom.document_mut().set_viewport(viewport);
        let snapshot = dom.flush_layout().unwrap();
        let dense = dom.native_viewport_surface(view, snapshot).unwrap();
        assert_eq!((dense.width, dense.height), (360, 150));
        assert_eq!(dense.device_pixel_ratio, 3.0);
        assert_eq!(
            dense.rect.width, 120.0,
            "CSS geometry is density-independent"
        );
        assert_eq!(dense.generation, resized.generation + 1);
    }

    #[test]
    fn viewport_writes_must_be_one_complete_frame() {
        let mut dom = viewport_document(
            r#"<blitsen-view id="view" style="width: 4px; height: 2px"></blitsen-view>"#,
            1.0,
        );
        let snapshot = dom.flush_layout().unwrap();
        let view = dom.get_element_by_id("view").unwrap().unwrap();
        let surface = dom.native_viewport_surface(view, snapshot).unwrap();
        assert_eq!(surface.byte_length(), 32);

        assert_eq!(
            dom.write_native_viewport(view, &[0; 16]),
            Err(DomError::Backend(
                "<blitsen-view> surface needs 32 RGBA bytes, received 16".into()
            ))
        );
        assert!(dom.write_native_viewport(view, &[0; 32]).is_ok());

        let body = dom.body().unwrap();
        assert_eq!(
            dom.write_native_viewport(body, &[0; 32]),
            Err(DomError::InvalidNodeType)
        );

        // A resize invalidates the frame the application drew for the old size.
        dom.set_inline_style(view, "width", "8px").unwrap();
        dom.flush_layout().unwrap();
        assert!(dom.write_native_viewport(view, &[0; 32]).is_err());
        assert!(dom.write_native_viewport(view, &[0; 64]).is_ok());
    }

    #[test]
    fn viewport_elements_hit_test_like_any_other_element() {
        let mut dom = viewport_document(
            r#"<div id="backdrop" style="position: absolute; left: 0; top: 0;
                  width: 400px; height: 300px"></div>
               <div id="host" style="position: relative">
                 <blitsen-view id="view" style="position: absolute; left: 20px; top: 10px;
                    width: 100px; height: 50px"></blitsen-view>
               </div>
               <blitsen-view id="transparent" style="position: absolute; left: 20px; top: 10px;
                  width: 100px; height: 50px; pointer-events: none"></blitsen-view>"#,
            1.0,
        );
        let snapshot = dom.flush_layout().unwrap();
        let backdrop = dom.get_element_by_id("backdrop").unwrap().unwrap();
        let host = dom.get_element_by_id("host").unwrap().unwrap();
        let view = dom.get_element_by_id("view").unwrap().unwrap();

        let hit = dom.hit_test(50.0, 25.0, snapshot).unwrap().unwrap();
        assert_eq!(
            hit.target, view,
            "a later viewport with pointer-events: none does not swallow the hit"
        );
        assert_eq!((hit.offset_x, hit.offset_y), (30.0, 15.0));
        assert_eq!(hit.path.last(), Some(&view));
        assert!(
            hit.path.contains(&host),
            "propagation reaches a viewport through its ordinary ancestors"
        );
        assert_eq!(
            dom.hit_test(10.0, 5.0, snapshot).unwrap().unwrap().target,
            backdrop,
            "a viewport claims no more than its own box"
        );
    }

    /// Clip shapes in force where the composited surface is recorded, together
    /// with the paint order of the surrounding solid DOM fills.
    ///
    /// Each clip is returned in scene coordinates so a document-space point can
    /// be tested against every layer that encloses the surface.
    fn composited_surface(scene: &Scene) -> (Vec<&'static str>, Vec<BezPath>) {
        let positioned = |transform: kurbo::Affine, clip: &BezPath| {
            let mut path = clip.clone();
            path.apply_affine(transform);
            // Blitz leaves clip subpaths implicitly closed; `contains` counts
            // windings over explicit segments only.
            if !matches!(path.elements().last(), Some(kurbo::PathEl::ClosePath)) {
                path.close_path();
            }
            path
        };
        let mut clips: Vec<BezPath> = Vec::new();
        let mut active: Vec<BezPath> = Vec::new();
        let mut order = Vec::new();
        for command in &scene.commands {
            match command {
                RenderCommand::PushLayer(layer) => {
                    active.push(positioned(layer.transform, &layer.clip));
                }
                RenderCommand::PushClipLayer(clip) => {
                    active.push(positioned(clip.transform, &clip.clip));
                }
                RenderCommand::PopLayer => {
                    active.pop();
                }
                RenderCommand::Fill(fill) => match &fill.brush {
                    Paint::Solid(color) => {
                        let rgba = color.to_rgba8();
                        match [rgba.r, rgba.g, rgba.b] {
                            [230, 30, 30] => order.push("below"),
                            [30, 60, 230] => order.push("above"),
                            _ => {}
                        }
                    }
                    Paint::Image(_) => {
                        order.push("surface");
                        clips = active.clone();
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        (order, clips)
    }

    #[test]
    fn viewport_contents_composite_between_dom_layers_and_inside_every_clip() {
        let mut dom = viewport_document(
            r#"<style>
                 #stage { position: relative; width: 80px; height: 300px; overflow: hidden }
                 #below { position: absolute; left: 0; top: 0; width: 200px; height: 200px;
                          background: rgb(230, 30, 30); z-index: -1 }
                 #view { position: absolute; left: 10px; top: 10px; width: 100px; height: 50px;
                         border-radius: 12px }
                 #above { position: absolute; left: 0; top: 0; width: 60px; height: 60px;
                          background: rgb(30, 60, 230); z-index: 1 }
               </style>
               <div id="stage">
                 <div id="below"></div>
                 <blitsen-view id="view"></blitsen-view>
                 <div id="above"></div>
               </div>"#,
            1.0,
        );
        let snapshot = dom.flush_layout().unwrap();
        let view = dom.get_element_by_id("view").unwrap().unwrap();
        let surface = dom.native_viewport_surface(view, snapshot).unwrap();
        dom.write_native_viewport(view, &vec![0xff; surface.byte_length()])
            .unwrap();

        let mut scene = Scene::new();
        blitz_paint::paint_scene(&mut scene, dom.document_mut().as_mut(), 1.0, 400, 300, 0, 0);
        let (order, clips) = composited_surface(&scene);

        assert_eq!(order, ["below", "surface", "above"]);
        assert!(!clips.is_empty());
        assert!(
            clips
                .iter()
                .all(|clip| clip.contains(Point::new(50.0, 35.0))),
            "the middle of the surface survives every clip"
        );
        assert!(
            clips
                .iter()
                .any(|clip| !clip.contains(Point::new(11.0, 11.0))),
            "the element's own border-radius rounds the surface"
        );
        assert!(
            clips
                .iter()
                .any(|clip| !clip.contains(Point::new(90.0, 35.0))),
            "the ancestor scrollport clips the surface"
        );
    }

    #[test]
    fn viewport_pixels_reach_the_composited_frame() {
        let mut dom = viewport_document(
            r#"<blitsen-view id="view" style="width: 40px; height: 20px"></blitsen-view>"#,
            1.0,
        );
        let snapshot = dom.flush_layout().unwrap();
        let view = dom.get_element_by_id("view").unwrap().unwrap();
        let surface = dom.native_viewport_surface(view, snapshot).unwrap();
        let frame: Vec<u8> = std::iter::repeat_n([0, 200, 40, 255], surface.byte_length() / 4)
            .flatten()
            .collect();
        dom.write_native_viewport(view, &frame).unwrap();

        let pixels = anyrender::render_to_buffer::<VelloCpuImageRenderer, _>(
            |scene| {
                blitz_paint::paint_scene(scene, dom.document_mut().as_mut(), 1.0, 60, 40, 0, 0);
            },
            60,
            40,
        );
        let pixel = |x: usize, y: usize| {
            let start = (y * 60 + x) * 4;
            [
                pixels[start],
                pixels[start + 1],
                pixels[start + 2],
                pixels[start + 3],
            ]
        };
        assert_eq!(pixel(20, 10), [0, 200, 40, 255]);
        assert_eq!(
            pixel(50, 30),
            [0, 0, 0, 0],
            "the surface does not paint outside its own box"
        );
    }

    /// Guards the `system-fonts` feature on `blitz-dom`.
    ///
    /// Without it Parley has no font sources, every glyph paints nothing, and the
    /// failure is invisible to any assertion that reads the DOM instead of the
    /// frame. That is exactly how it went unnoticed until a demo was recorded.
    #[test]
    fn text_paints_glyphs_rather_than_nothing() {
        let mut dom = viewport_document(
            r#"<div style="font: 48px sans-serif; color: #000">HELLO 12345</div>"#,
            1.0,
        );
        dom.flush_layout().unwrap();
        let pixels = anyrender::render_to_buffer::<VelloCpuImageRenderer, _>(
            |scene| {
                blitz_paint::paint_scene(scene, dom.document_mut().as_mut(), 1.0, 300, 80, 0, 0);
            },
            300,
            80,
        );
        let inked = pixels.chunks_exact(4).filter(|pixel| pixel[3] > 0).count();
        assert!(
            inked > 200,
            "text rendered {inked} non-transparent pixels; system fonts are not loaded"
        );
    }

    /// Guards `@font-face` end to end: fetch, WOFF2 decompression, registration
    /// under the CSS family name, and shaping with the registered face.
    ///
    /// Real framework output almost always ships a web font, so a build that
    /// quietly fell back to the system UI font would not look like itself. Every
    /// letter in the fixture is a solid em block, which no fallback paints, so
    /// the frame says which font was used rather than merely that one was.
    #[test]
    fn web_fonts_load_from_woff2_and_replace_the_fallback() {
        let mut dom = fixture_document(
            r#"<style>
                 @font-face { font-family: "Block"; src: url("block-regular.woff2") format("woff2") }
                 div { font: 50px "Block", sans-serif; color: #000 }
               </style>
               <div>AAAA</div>"#,
            None,
        );
        dom.flush_layout().unwrap();
        let pixels = render(&mut dom, 400, 200);
        let (x, y, width, height) = inked_bounds(&pixels, 400).expect("the run painted nothing");
        assert_eq!(
            (x, width, height),
            (0, 200, 50),
            "four 50px em blocks, so the web font shaped and drew the run"
        );
        assert!(
            (y..y + height)
                .flat_map(|row| (x..x + width).map(move |column| (column, row)))
                .all(|(column, row)| pixel(&pixels, 400, column, row) == [0, 0, 0, 255]),
            "the block glyph is solid, so nothing else contributed to the run"
        );
    }

    /// Faces of one family are told apart by `@font-face` descriptor, not by
    /// the metadata inside the font file.
    ///
    /// The three fixtures are internally indistinguishable — same family name,
    /// same "Regular" style, same weight 400, none of them the family the CSS
    /// declares — so only the descriptors can pick one. They differ only in
    /// block height, which turns a wrong match into a wrong painted height.
    ///
    /// Also covers an uncompressed `truetype` source alongside the WOFF2 above.
    #[test]
    fn font_face_descriptors_select_the_face_within_a_family() {
        let mut dom = fixture_document(
            r#"<style>
                 @font-face { font-family: "Block"; src: url("block-regular.ttf") format("truetype") }
                 @font-face { font-family: "Block"; font-weight: 700;
                              src: url("block-bold.ttf") format("truetype") }
                 @font-face { font-family: "Block"; font-style: italic;
                              src: url("block-italic.ttf") format("truetype") }
                 div { position: absolute; left: 0; font: 50px "Block"; color: #000 }
                 #bold { top: 60px; font-weight: bold }
                 #italic { top: 120px; font-style: italic }
               </style>
               <div id="regular">A</div>
               <div id="bold">A</div>
               <div id="italic">A</div>"#,
            None,
        );
        dom.flush_layout().unwrap();
        let pixels = render(&mut dom, 400, 200);
        let band = |top: usize, bottom: usize| {
            inked_bounds(&pixels[top * 400 * 4..bottom * 400 * 4], 400)
                .expect("a run painted nothing")
        };
        assert_eq!(band(0, 60).3, 50, "the 400 face fills the em box");
        assert_eq!(band(60, 120).3, 25, "the 700 face fills half of it");
        assert_eq!(
            band(120, 200).3,
            13,
            "the italic face fills a quarter of it"
        );
        assert_eq!(band(0, 60).2, band(60, 120).2, "every face advances one em");
    }

    /// Nothing registers a font as a critical resource, so a document paints
    /// while its web fonts are still in flight: Blitsen is FOUT, never FOIT.
    ///
    /// The alternative — withholding the frame until the font arrives — would
    /// trade a restyle for a blank window on every cold start.
    #[test]
    fn text_paints_in_the_fallback_while_a_web_font_is_still_loading() {
        let network = DeferredResources::default();
        let mut dom = fixture_document(
            r#"<style>
                 @font-face { font-family: "Block"; src: url("block-regular.woff2") format("woff2") }
                 div { font: 50px "Block", sans-serif; color: #000 }
               </style>
               <div>AAAA</div>"#,
            Some(Arc::new(network.clone())),
        );
        dom.flush_layout().unwrap();
        let waiting = render(&mut dom, 400, 200);
        let (_, _, _, fallback_height) =
            inked_bounds(&waiting, 400).expect("no text painted while the web font loaded");
        assert_ne!(
            fallback_height, 50,
            "the fallback face painted, not the block glyph"
        );

        network.deliver();
        dom.flush_layout().unwrap();
        let loaded = render(&mut dom, 400, 200);
        assert_eq!(
            inked_bounds(&loaded, 400).map(|bounds| (bounds.2, bounds.3)),
            Some((200, 50)),
            "the arriving font reshapes the already-painted run"
        );
    }

    /// `<img>` end to end: fetch, decode, intrinsic sizing and paint.
    ///
    /// The intrinsic size is what CSS resolves the unspecified dimension
    /// against, so a decode that silently produced nothing would lay the
    /// element out at zero height rather than fail visibly.
    #[test]
    fn images_decode_paint_and_report_their_intrinsic_size() {
        let mut dom = fixture_document(
            r#"<img id="swatch" src="swatch.png" style="display: block; width: 80px">"#,
            None,
        );
        let snapshot = dom.flush_layout().unwrap();
        let swatch = dom.get_element_by_id("swatch").unwrap().unwrap();
        assert_eq!(
            dom.image_state(swatch, snapshot),
            Ok(ImageState::decoded(8, 4))
        );
        let rect = dom.bounding_rect(swatch, snapshot).unwrap();
        assert_eq!(
            (rect.width, rect.height),
            (80.0, 40.0),
            "the intrinsic ratio resolves the dimension CSS left out"
        );

        let pixels = render(&mut dom, 400, 200);
        assert_eq!(pixel(&pixels, 400, 20, 20), [220, 20, 20, 255]);
        assert_eq!(pixel(&pixels, 400, 60, 20), [20, 40, 220, 255]);
        assert_eq!(inked_bounds(&pixels, 400), Some((0, 0, 80, 40)));
    }

    /// Bundlers inline small assets, so a drop-in build's icons arrive as
    /// `data:` URLs rather than files.
    #[test]
    fn images_decode_from_inlined_data_urls() {
        // The `swatch.png` fixture, encoded the way a bundler would inline it.
        let inlined = concat!(
            "data:image/png;base64,",
            "iVBORw0KGgoAAAANSUhEUgAAAAgAAAAECAYAAACzzX7wAAAAF0lEQVR42mO4IyLy",
            "HxmLaNxBwQy0VwAAw8RBoVkySsgAAAAASUVORK5CYII="
        );
        let mut dom = fixture_document(
            &format!(r#"<img id="inlined" src="{inlined}" style="display: block; width: 80px">"#),
            None,
        );
        let snapshot = dom.flush_layout().unwrap();
        let inlined = dom.get_element_by_id("inlined").unwrap().unwrap();
        assert_eq!(
            dom.image_state(inlined, snapshot),
            Ok(ImageState::decoded(8, 4))
        );
        assert_eq!(
            inked_bounds(&render(&mut dom, 400, 200), 400),
            Some((0, 0, 80, 40))
        );
    }

    /// A `background-image` is only discovered once style resolves, which is
    /// after the pass that would have applied it. It still has to be in the
    /// frame that asked for it, or every backdrop flashes empty for one frame.
    #[test]
    fn background_images_paint_in_the_frame_that_asks_for_them() {
        let mut dom = fixture_document(
            r#"<div style="width: 80px; height: 40px; background-image: url('swatch.png');
                 background-size: 80px 40px"></div>"#,
            None,
        );
        dom.flush_layout().unwrap();
        let pixels = render(&mut dom, 400, 200);
        assert_eq!(pixel(&pixels, 400, 20, 20), [220, 20, 20, 255]);
        assert_eq!(pixel(&pixels, 400, 60, 20), [20, 40, 220, 255]);
    }

    /// An image that will never arrive must not be reported as still arriving:
    /// `complete` is what a script polls, and a stuck `false` never resolves.
    #[test]
    fn a_failed_image_is_complete_and_errored_rather_than_loading_forever() {
        let dom = fixture_document(
            r#"<img id="missing" src="does-not-exist.png">
               <img id="remote" src="https://example.com/logo.png">
               <img id="undecodable" src="data:image/png;base64,bm90IGEgcG5n">
               <img id="sourceless">
               <p id="paragraph">not an image</p>"#,
            None,
        );
        let mut dom = dom;
        let snapshot = dom.flush_layout().unwrap();
        let state =
            |id: &str| dom.image_state(dom.get_element_by_id(id).unwrap().unwrap(), snapshot);
        assert_eq!(state("missing"), Ok(ImageState::FAILED));
        assert_eq!(
            state("remote"),
            Ok(ImageState::FAILED),
            "a refused remote fetch is an error, not an unfinished one"
        );
        assert_eq!(
            state("undecodable"),
            Ok(ImageState::FAILED),
            "bytes that arrived but did not decode are an error too"
        );
        assert_eq!(
            state("sourceless"),
            Ok(ImageState::IDLE),
            "an image with nothing to load is already complete"
        );
        assert_eq!(state("paragraph"), Err(DomError::InvalidNodeType));
    }

    /// The state a script actually observes on a cold window: in flight first,
    /// decoded afterwards, with the frame that decodes it also painting it.
    #[test]
    fn an_image_still_in_flight_is_not_complete() {
        let network = DeferredResources::default();
        let mut dom = fixture_document(
            r#"<img id="swatch" src="swatch.png" style="display: block; width: 80px">"#,
            Some(Arc::new(network.clone())),
        );
        let snapshot = dom.flush_layout().unwrap();
        let swatch = dom.get_element_by_id("swatch").unwrap().unwrap();
        assert_eq!(dom.image_state(swatch, snapshot), Ok(ImageState::LOADING));
        assert_eq!(inked_bounds(&render(&mut dom, 400, 200), 400), None);

        network.deliver();
        let snapshot = dom.flush_layout().unwrap();
        assert_eq!(
            dom.image_state(swatch, snapshot),
            Ok(ImageState::decoded(8, 4))
        );
        assert_eq!(
            inked_bounds(&render(&mut dom, 400, 200), 400),
            Some((0, 0, 80, 40))
        );
    }

    /// The `new Image()` path: an element built by script, given a source and
    /// then connected, has to load exactly like a parsed one.
    #[test]
    fn a_scripted_image_loads_when_its_source_is_set() {
        let mut dom = fixture_document(r#"<div id="host"></div>"#, None);
        let host = dom.get_element_by_id("host").unwrap().unwrap();
        let image = dom.create_element(&DomName::html("img")).unwrap();
        let snapshot = dom.flush_layout().unwrap();
        assert_eq!(
            dom.image_state(image, snapshot),
            Ok(ImageState::IDLE),
            "a detached image with no source has nothing to wait for"
        );

        dom.set_attribute(image, &DomName::attribute("src"), "swatch.png")
            .unwrap();
        dom.append_child(host, image).unwrap();
        let snapshot = dom.flush_layout().unwrap();
        assert_eq!(
            dom.image_state(image, snapshot),
            Ok(ImageState::decoded(8, 4))
        );

        dom.set_attribute(image, &DomName::attribute("alt"), "swatch")
            .unwrap();
        assert_eq!(
            dom.image_state(image, snapshot),
            Err(DomError::LayoutNotFlushed),
            "decode state is applied while layout resolves, so it is snapshot gated"
        );
    }
}
