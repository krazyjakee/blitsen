//! The [`DomBackend`] implementation: the whole renderer-neutral surface.
//!
//! One `impl` block, because a trait implementation cannot be split across
//! modules; the helpers it leans on live beside it in sibling files.

use blitsen_dom::{
    CaretPosition, DomBackend, DomError, DomName, HitTest, ImageState, LayoutMetrics,
    LayoutSnapshot, LinkState, MediaQueryMatch, NATIVE_VIEWPORT_TAG, NodeKind, Rect, TextEdit,
    TextMotion, TextSelection, ViewportSurface,
};
use blitz::dom::node::ImageData;
use blitz::dom::{NodeData, NodeId};
use style::context::QuirksMode;
use style::properties::{PropertyDeclaration, PropertyId};
use style::stylesheets::{CssRule, CustomMediaEvaluator};
use style::values::computed::Overflow;

use crate::pointer_events;
use crate::resources::ResourceState;
use crate::{BlitzDom, RESOURCE_RESOLVE_PASSES, css_pixels};

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
        // A `style` attribute is CSS arriving at the cascade like any other.
        let normalized = (name.local == "style")
            .then(|| pointer_events::normalize_css(value))
            .flatten();
        let value = normalized.as_deref().unwrap_or(value);
        self.document
            .mutate()
            .set_attribute(node, Self::qual_name(name), value);
        self.restore_form_state(node, name);
        self.mutate(Some(node), Some(node));
        Ok(())
    }

    fn remove_attribute(&mut self, node: NodeId, name: &DomName) -> Result<bool, DomError> {
        let existed = self.attribute(node, name)?.is_some();
        if existed {
            self.document
                .mutate()
                .clear_attribute(node, Self::qual_name(name));
            self.restore_form_state(node, name);
            self.mutate(Some(node), Some(node));
        }
        Ok(existed)
    }

    fn form_value(&self, node: NodeId) -> Result<String, DomError> {
        self.ensure_element(node)?;
        if let Some(text) = self.editor_text(node) {
            return Ok(text);
        }
        if let Some(value) = self
            .form_state
            .get(&node)
            .and_then(|state| state.value.clone())
        {
            return Ok(value);
        }
        // Nothing has been laid out yet, so answer with what the control will
        // start from: a textarea's child text, an input's `value` attribute.
        if self.is_tag(node, "textarea") {
            self.text_content(node)
        } else {
            Ok(self
                .attribute(node, &DomName::attribute("value"))?
                .unwrap_or_default())
        }
    }

    fn set_form_value(&mut self, node: NodeId, value: &str) -> Result<(), DomError> {
        self.ensure_element(node)?;
        let settled = self.write_editor_text(node, value);
        let state = self.form_state.entry(node).or_default();
        state.value = Some(value.to_owned());
        state.pending = !settled;
        self.mutate(Some(node), Some(node));
        Ok(())
    }

    fn form_selection(&self, node: NodeId) -> Result<TextSelection, DomError> {
        self.ensure_element(node)?;
        Ok(self.editor_selection(node))
    }

    fn set_form_selection(
        &mut self,
        node: NodeId,
        selection: TextSelection,
    ) -> Result<(), DomError> {
        self.ensure_element(node)?;
        self.write_editor_selection(node, selection);
        Ok(())
    }

    fn move_form_selection(
        &mut self,
        node: NodeId,
        motion: TextMotion,
        extend: bool,
    ) -> Result<bool, DomError> {
        self.ensure_element(node)?;
        Ok(self.move_editor_selection(node, motion, extend))
    }

    fn move_form_caret_to_point(
        &mut self,
        node: NodeId,
        offset_x: f32,
        offset_y: f32,
        extend: bool,
    ) -> Result<bool, DomError> {
        self.ensure_element(node)?;
        Ok(self.move_editor_caret_to_point(node, offset_x, offset_y, extend))
    }

    fn edit_form_value(&mut self, node: NodeId, edit: TextEdit<'_>) -> Result<bool, DomError> {
        self.ensure_element(node)?;
        Ok(self.edit_editor_value(node, edit))
    }

    fn set_focused(&mut self, node: Option<NodeId>) -> Result<(), DomError> {
        if let Some(node) = node {
            self.ensure_element(node)?;
        }
        self.set_focused_node(node);
        Ok(())
    }

    fn form_checked(&self, node: NodeId) -> Result<bool, DomError> {
        self.ensure_element(node)?;
        if let Some(checked) = self.checked_state(node) {
            return Ok(checked);
        }
        if let Some(checked) = self.form_state.get(&node).and_then(|state| state.checked) {
            return Ok(checked);
        }
        let attribute = if self.is_tag(node, "option") {
            "selected"
        } else {
            "checked"
        };
        Ok(self
            .attribute(node, &DomName::attribute(attribute))?
            .is_some())
    }

    fn set_form_checked(&mut self, node: NodeId, checked: bool) -> Result<(), DomError> {
        self.ensure_element(node)?;
        let settled = self.write_checked_state(node, checked);
        let state = self.form_state.entry(node).or_default();
        state.checked = Some(checked);
        state.pending = !settled;
        self.mutate(Some(node), Some(node));
        Ok(())
    }

    fn inline_style(&self, node: NodeId, property: &str) -> Result<Option<String>, DomError> {
        self.style_property_value(node, property)
    }

    fn set_inline_style(
        &mut self,
        node: NodeId,
        property: &str,
        value: &str,
    ) -> Result<bool, DomError> {
        self.ensure_element(node)?;
        // `element.style.pointerEvents = "all"` arrives with the property and
        // the value already apart, so there is no declaration text to scan.
        let value = if pointer_events::is_property(property) {
            pointer_events::normalize_value(value).unwrap_or(value)
        } else {
            value
        };
        let original = self.style_text(node)?;
        // Replace rather than append so a previous `!important` declaration
        // does not silently win over a CSSOM assignment. Both operations go
        // through Blitz's Stylo-backed declaration block; serializing it back
        // into the attribute keeps DOM reads and the cascade on one value.
        self.document.remove_style_property(node, property);
        self.document.set_style_property(node, property, value);
        let valid = self.inline_style(node, property)?.is_some();
        let candidate = if valid {
            self.style_text(node)?
        } else {
            original
        };
        self.document.mutate().set_attribute(
            node,
            Self::qual_name(&DomName::attribute("style")),
            &candidate,
        );
        if valid {
            self.mutate(Some(node), Some(node));
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
        self.document.remove_style_property(node, property);
        let css = self.style_text(node)?;
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

    fn style_sheets(&self) -> Result<Vec<NodeId>, DomError> {
        self.query_selector_all(self.document(), r#"style, link[rel~="stylesheet"]"#)
    }

    fn sheet_rules(&self, node: NodeId) -> Result<Vec<String>, DomError> {
        self.sheet_owner(node)?;
        Ok(Self::split_css_rules(&self.text_content(node)?))
    }

    fn insert_sheet_rule(
        &mut self,
        node: NodeId,
        rule: &str,
        index: usize,
    ) -> Result<(), DomError> {
        self.sheet_owner(node)?;
        let mut rules = Self::split_css_rules(&self.text_content(node)?);
        if index > rules.len() {
            return Err(DomError::NotFound);
        }
        // Refused on two counts, because the two catch different mistakes: one
        // rule structurally, and one rule the cascade's own parser recognizes.
        // Anything else would be written into the sheet and silently ignored.
        let split = Self::split_css_rules(rule);
        if split.len() != 1 || self.parsed_rule_count(rule) != 1 {
            return Err(DomError::Syntax(format!(
                "not a single CSS rule: {}",
                rule.trim()
            )));
        }
        rules.insert(index, split.into_iter().next().unwrap_or_default());
        self.write_sheet_rules(node, &rules)
    }

    fn delete_sheet_rule(&mut self, node: NodeId, index: usize) -> Result<(), DomError> {
        self.sheet_owner(node)?;
        let mut rules = Self::split_css_rules(&self.text_content(node)?);
        if index >= rules.len() {
            return Err(DomError::NotFound);
        }
        rules.remove(index);
        self.write_sheet_rules(node, &rules)
    }

    fn text_content(&self, node: NodeId) -> Result<String, DomError> {
        Ok(self.node(node)?.text_content())
    }

    fn set_text_content(&mut self, node: NodeId, text: &str) -> Result<(), DomError> {
        // The text of a `<style>` is a stylesheet, and this is how one written
        // by a bundler's CSS-in-JS shim reaches the cascade.
        let normalized = self
            .is_tag(node, "style")
            .then(|| pointer_events::normalize_css(text))
            .flatten();
        let text = normalized.as_deref().unwrap_or(text);
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

    fn outer_html(&self, node: NodeId) -> Result<String, DomError> {
        let mut html = String::new();
        self.serialize_node(node, &mut html, false)?;
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

    fn set_animation_time(&mut self, seconds: f64) {
        // A clock that ran backwards would restart every animation that had
        // already started, so it only ever moves forward.
        if seconds.is_finite() && seconds > self.animation_time {
            self.animation_time = seconds;
        }
    }

    fn is_animating(&self) -> bool {
        self.document.is_animating()
    }

    fn flush_layout(&mut self) -> Result<LayoutSnapshot, DomError> {
        let settle_controls = self.layout_is_dirty();
        let now = self.animation_time;
        self.take_frame_invalidation();
        self.attach_native_viewports()?;
        self.attach_canvases()?;
        for _ in 0..RESOURCE_RESOLVE_PASSES {
            let settled = self.resources.settlements();
            self.document.resolve(now);
            if self.resources.settlements() == settled {
                break;
            }
        }
        // Only after the resolve above: a control's editor is built while layout
        // resolves, so this is the first moment there is anything to write into.
        if settle_controls && self.settle_form_controls() {
            self.document.resolve(now);
        }
        self.resize_native_viewports();
        self.flushed_revision = self.revision;
        Ok(LayoutSnapshot::new(self.revision))
    }

    fn layout_is_dirty(&self) -> bool {
        self.flushed_revision != self.revision
    }

    fn bounding_rect(&self, node: NodeId, snapshot: LayoutSnapshot) -> Result<Rect, DomError> {
        self.ensure_layout_fresh(snapshot)?;
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

    fn client_rects(&self, node: NodeId, snapshot: LayoutSnapshot) -> Result<Vec<Rect>, DomError> {
        self.ensure_layout_fresh(snapshot)?;
        // Validates the handle before asking Blitz, which answers a stale one
        // with an empty list rather than an error.
        self.node(node)?;
        // Nothing laid out is no rectangles, rather than the empty box at the
        // origin `bounding_rect` reports: a `display: none` element and a `<br>`
        // both have geometry to describe only if you invent it.
        Ok(self
            .document
            .node_client_rects(node)
            .into_iter()
            .map(|rect| Rect {
                x: rect.x as f32,
                y: rect.y as f32,
                width: rect.width as f32,
                height: rect.height as f32,
            })
            .collect())
    }

    fn text_rects(
        &self,
        node: NodeId,
        start: u32,
        end: u32,
        snapshot: LayoutSnapshot,
    ) -> Result<Vec<Rect>, DomError> {
        self.ensure_layout_fresh(snapshot)?;
        self.text_node_rects(node, start, end)
    }

    fn caret_position(
        &self,
        x: f32,
        y: f32,
        snapshot: LayoutSnapshot,
    ) -> Result<Option<CaretPosition<NodeId>>, DomError> {
        self.ensure_layout_fresh(snapshot)?;
        self.caret_at_point(x, y)
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
        let content_rect = Rect {
            x: layout.border.left + layout.padding.left,
            y: layout.border.top + layout.padding.top,
            width: (layout.size.width
                - layout.border.left
                - layout.border.right
                - layout.padding.left
                - layout.padding.right
                - layout.scrollbar_size.width)
                .max(0.0),
            height: (layout.size.height
                - layout.border.top
                - layout.border.bottom
                - layout.padding.top
                - layout.padding.bottom
                - layout.scrollbar_size.height)
                .max(0.0),
        };
        Ok(LayoutMetrics {
            rect,
            content_rect,
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

    fn resolved_style(
        &self,
        node: NodeId,
        property: &str,
        snapshot: LayoutSnapshot,
    ) -> Result<Option<String>, DomError> {
        let element = self.node(node)?;
        element.element_data().ok_or(DomError::InvalidNodeType)?;
        // A node the cascade never reached — one that has never been connected —
        // has no resolved style at all, rather than a default one.
        let Some(styles) = element.primary_styles() else {
            return Ok(None);
        };
        // CSSOM resolves `width` and `height` to the used value, which is the
        // content box layout produced and not the declaration that asked for it.
        // A box that was never generated has no used value to report.
        if matches!(property, "width" | "height") && !styles.clone_display().is_none() {
            let content = self.layout_metrics(node, snapshot)?.content_rect;
            let used = if property == "width" {
                content.width
            } else {
                content.height
            };
            return Ok(Some(css_pixels(used)));
        }
        self.ensure_layout_fresh(snapshot)?;
        let Ok(id) = PropertyId::parse_enabled_for_all_content(property) else {
            return Ok(None);
        };
        Ok(match id.as_shorthand() {
            Err(declaration) => Some(styles.computed_value_to_string(declaration)),
            // A shorthand has no computed value of its own: it is the
            // serialization of its longhands, and only some sets of longhand
            // values are expressible as one.
            Ok(shorthand) => {
                let longhands: Vec<PropertyDeclaration> = shorthand
                    .longhands()
                    .map(|longhand| styles.computed_or_resolved_declaration(longhand, None))
                    .collect();
                let mut text = String::new();
                shorthand
                    .longhands_to_css(&longhands.iter().collect::<Vec<_>>(), &mut text)
                    .ok()
                    .filter(|()| !text.is_empty())
                    .map(|()| text)
            }
        })
    }

    fn media_query(&mut self, query: &str) -> Result<MediaQueryMatch, DomError> {
        let guard = self.document.guard().clone();
        // Parsed as the stylesheet rule it is, so a query reaches JavaScript
        // through the same parser and the same error handling the cascade uses.
        // The rule needs a body a parser will not discard.
        let sheet =
            self.parse_stylesheet(&format!("@media {query} {{ x {{ color: red }} }}"), &guard);
        let read = guard.read();
        let media = sheet
            .rules
            .read_with(&read)
            .0
            .iter()
            .find_map(|rule| match rule {
                CssRule::Media(rule) => Some(rule.media_queries.read_with(&read)),
                _ => None,
            })
            .ok_or_else(|| DomError::Syntax(format!("not a media query: {query}")))?;
        // `MediaList` derives its `Debug` from its CSS serialization, which is
        // what CSSOM asks `MediaQueryList.media` to report.
        let text = format!("{media:?}");
        let matches = media.evaluate(
            self.document.stylist_device(),
            QuirksMode::NoQuirks,
            &mut CustomMediaEvaluator::none(),
        );
        Ok(MediaQueryMatch {
            media: text,
            matches,
        })
    }

    fn set_scroll_offset(
        &mut self,
        node: NodeId,
        left: Option<f64>,
        top: Option<f64>,
        snapshot: LayoutSnapshot,
    ) -> Result<(), DomError> {
        self.ensure_layout_fresh(snapshot)?;
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
        self.ensure_layout_fresh(snapshot)?;
        let Some((_, _, _, target, offset_x, offset_y)) = self.ranked_hit(x, y)? else {
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
        self.ensure_layout_fresh(snapshot)?;
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

    fn link_state(&self, node: NodeId, snapshot: LayoutSnapshot) -> Result<LinkState, DomError> {
        self.ensure_layout_fresh(snapshot)?;
        let element = self
            .node(node)?
            .element_data()
            .ok_or(DomError::InvalidNodeType)?;
        if element.name.local.as_ref() != "link" {
            return Err(DomError::InvalidNodeType);
        }
        // The same test Blitz applies before it requests anything, down to the
        // case sensitivity: a `rel` this agrees is a stylesheet but Blitz does
        // not would leave a caller waiting on a request nobody made.
        let stylesheet = self
            .attribute(node, &DomName::attribute("rel"))?
            .is_some_and(|rel| rel.split_ascii_whitespace().any(|rel| rel == "stylesheet"));
        let href = self
            .attribute(node, &DomName::attribute("href"))?
            .filter(|href| !href.is_empty());
        let (true, Some(href)) = (stylesheet, href) else {
            return Ok(LinkState::IDLE);
        };
        Ok(
            match self
                .document
                .url()
                .join(&href)
                .map(|url| self.resources.state(url.as_str()))
            {
                Ok(Some(ResourceState::Loaded)) => LinkState::LOADED,
                // An answer with no bytes. The renderer cannot tell a refused
                // request from a sheet that really is empty, and reports the
                // one that matters: an empty sheet contributes nothing either
                // way, and a missing one has to be able to say so.
                Ok(Some(ResourceState::Failed)) => LinkState::FAILED,
                Ok(Some(ResourceState::Loading)) => LinkState::LOADING,
                // An `href` that cannot become a URL will never be requested,
                // so waiting on it would be waiting forever.
                Err(_) => LinkState::FAILED,
                // Requested by nothing yet: an element connected during the
                // flush that is reading it back, or one still detached.
                Ok(None) => LinkState::LOADING,
            },
        )
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
