//! The concrete [`blitsen_dom::DomBackend`] implemented over Blitz.
//!
//! [`BlitzDom`] is the only place in Blitsen that translates renderer-neutral
//! DOM operations into Blitz calls. It owns one authoritative `HtmlDocument`;
//! no parallel tree or attribute store is maintained.

use std::collections::HashMap;
use std::sync::Arc;

use blitsen_dom::{
    DomBackend, DomError, DomName, FrameInvalidation, InvalidationMetrics, InvalidationMode,
    InvalidationTracker, LayoutSnapshot, Namespace, NodeKind, Rect,
};
use blitz::dom::{DocumentConfig, LocalName, NodeData, NodeId, QualName, ns};
use blitz::html::{HtmlDocument, HtmlProvider};

/// A Blitz HTML document exposed only through Blitsen's DOM boundary.
pub struct BlitzDom {
    document: HtmlDocument,
    revision: u64,
    flushed_revision: u64,
    invalidation: InvalidationTracker<NodeId>,
    last_invalidation_metrics: InvalidationMetrics,
    last_frame_was_full_document: bool,
    js_references: HashMap<NodeId, u32>,
}

impl BlitzDom {
    /// Parses an HTML document with the real Blitz fragment parser installed.
    pub fn from_html(html: &str, mut config: DocumentConfig) -> Self {
        config.html_parser_provider = Some(Arc::new(HtmlProvider));
        Self::new(HtmlDocument::from_html(html, config))
    }

    /// Wraps an existing Blitz document and installs the fragment parser.
    pub fn new(mut document: HtmlDocument) -> Self {
        document.set_html_parser_provider(Arc::new(HtmlProvider));
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
        }
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

    fn node(&self, node: NodeId) -> Result<&blitz::dom::Node, DomError> {
        self.document.get_node(node).ok_or(DomError::StaleNode)
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
        self.document.resolve(0.0);
        self.flushed_revision = self.revision;
        Ok(LayoutSnapshot::new(self.revision))
    }

    fn bounding_rect(&self, node: NodeId, snapshot: LayoutSnapshot) -> Result<Rect, DomError> {
        if snapshot.revision() != self.revision || self.flushed_revision != self.revision {
            return Err(DomError::LayoutNotFlushed);
        }
        let layout = self.node(node)?.final_layout();
        Ok(Rect {
            x: layout.location.x,
            y: layout.location.y,
            width: layout.size.width,
            height: layout.size.height,
        })
    }

    fn hit_test(
        &self,
        x: f32,
        y: f32,
        snapshot: LayoutSnapshot,
    ) -> Result<Option<NodeId>, DomError> {
        if snapshot.revision() != self.revision || self.flushed_revision != self.revision {
            return Err(DomError::LayoutNotFlushed);
        }
        Ok(self.document.hit(x, y).map(|hit| hit.node_id))
    }
}

#[cfg(test)]
mod tests {
    use blitsen_dom::{DomBackend, DomError, DomName, NodeKind};
    use blitz::dom::DocumentConfig;
    use blitz::traits::shell::{ColorScheme, Viewport};

    use super::BlitzDom;

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
}
