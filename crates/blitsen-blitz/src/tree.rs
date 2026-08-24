//! Tree internals: node access, detachment bookkeeping, names and serialization.

use std::cell::RefCell;
use std::rc::Rc;

use blitsen_dom::{DomError, DomName, LayoutSnapshot, NATIVE_VIEWPORT_TAG, Namespace};
use blitz::dom::{LocalName, NodeData, NodeId, QualName, ns};

use crate::BlitzDom;
use crate::surface::attach_widgets;
use crate::viewport::{ViewportState, ViewportWidget};

impl BlitzDom {
    /// Gives every connected viewport element a surface, and forgets dead ones.
    ///
    /// Attaching is a tree mutation, so it runs before layout resolves: a
    /// surface installed afterwards would first paint against the layout of the
    /// frame that created it. A detached element keeps its surface, because a
    /// reparented viewport is the same viewport.
    pub(crate) fn attach_native_viewports(&mut self) -> Result<(), DomError> {
        attach_widgets(
            self,
            NATIVE_VIEWPORT_TAG,
            |dom| &mut dom.native_viewports,
            |_dom, _node| Ok(Rc::new(RefCell::new(ViewportState::default()))),
            |state| Box::new(ViewportWidget::new(state)),
        )
    }

    /// Propagates the resolved box and display density into each surface.
    pub(crate) fn resize_native_viewports(&mut self) {
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

    pub(crate) fn node(&self, node: NodeId) -> Result<&blitz::dom::Node, DomError> {
        self.document.get_node(node).ok_or(DomError::StaleNode)
    }

    /// Rejects layout state unless both the token and the last flush describe
    /// the tree's current revision.
    pub(crate) fn ensure_layout_fresh(&self, snapshot: LayoutSnapshot) -> Result<(), DomError> {
        if snapshot.revision() == self.revision && self.flushed_revision == self.revision {
            Ok(())
        } else {
            Err(DomError::LayoutNotFlushed)
        }
    }

    /// The box that laid this one out, which is not always the DOM parent.
    ///
    /// Blitz sets `layout_parent` while it builds boxes, so it names the
    /// anonymous block an inline run was wrapped in as well as the ordinary
    /// containers. Anything reading `final_layout().location` — an offset
    /// relative to that box — has to walk this chain rather than the DOM one.
    /// Before boxes have been built it is unset, and the DOM parent is the only
    /// answer available.
    pub(crate) fn layout_parent(&self, node: NodeId) -> Result<Option<NodeId>, DomError> {
        let node = self.node(node)?;
        Ok(node.layout_parent.get().or(node.parent))
    }

    pub(crate) fn ensure_element(&self, node: NodeId) -> Result<(), DomError> {
        if self.node(node)?.element_data().is_some() {
            Ok(())
        } else {
            Err(DomError::InvalidNodeType)
        }
    }

    /// Reports whether a node is an HTML element with the given local name.
    pub(crate) fn is_tag(&self, node: NodeId, tag: &str) -> bool {
        self.document
            .get_node(node)
            .and_then(|node| node.element_data())
            .is_some_and(|element| {
                element.name.local.as_ref() == tag && element.name.ns == ns!(html)
            })
    }

    pub(crate) fn mutate(&mut self, style_node: Option<NodeId>, layout_node: Option<NodeId>) {
        self.revision = self.revision.wrapping_add(1);
        if let Some(node) = style_node {
            self.invalidation.mark_style(node);
        }
        if let Some(node) = layout_node {
            let document = &self.document;
            self.invalidation.mark_layout(node, |node| {
                document.get_node(node).and_then(|node| node.parent)
            });
        }
    }

    pub(crate) fn subtree_has_js_reference(&self, root: NodeId) -> bool {
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

    pub(crate) fn detached_root(&self, mut node: NodeId) -> NodeId {
        while let Some(parent) = self.document.get_node(node).and_then(|node| node.parent) {
            node = parent;
        }
        node
    }

    pub(crate) fn collect_detached_tree(&mut self, node: NodeId) -> bool {
        let root = self.detached_root(node);
        if root == self.document.root_node().id
            || self.subtree_has_js_reference(root)
            || self.document.get_node(root).is_none()
        {
            return false;
        }
        self.document.mutate().remove_and_drop_node(root).is_some()
    }

    pub(crate) fn detach_children(&mut self, parent: NodeId) -> Result<(), DomError> {
        let children = self.node(parent)?.children.clone();
        for child in children {
            self.document.mutate().remove_node(child);
            self.collect_detached_tree(child);
        }
        Ok(())
    }

    pub(crate) fn check_no_cycle(&self, parent: NodeId, child: NodeId) -> Result<(), DomError> {
        let mut current = Some(parent);
        while let Some(node) = current {
            if node == child {
                return Err(DomError::HierarchyRequest);
            }
            current = self.node(node)?.parent;
        }
        Ok(())
    }

    pub(crate) fn qual_name(name: &DomName) -> QualName {
        let namespace = match &name.namespace {
            Namespace::Html => ns!(html),
            Namespace::Svg => ns!(svg),
            Namespace::MathMl => ns!(mathml),
            Namespace::None => ns!(),
            Namespace::Other(value) => value.clone().into(),
        };
        QualName::new(None, namespace, LocalName::from(name.local.clone()))
    }

    pub(crate) fn namespace(name: &QualName) -> Namespace {
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

    pub(crate) fn serialize_node(
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
