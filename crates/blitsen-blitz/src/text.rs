//! Text geometry: the box a run of characters occupies, and the character a
//! point lands on.
//!
//! Both answers come out of one Parley layout. Blitz does not lay a text node
//! out on its own: every text node inside a block is shaped into that block's
//! single inline layout, so the geometry of "characters 3 to 7 of this text
//! node" is a byte range in the inline root's laid-out text, not a box the tree
//! holds anywhere.
//!
//! Finding that byte range is the whole difficulty. The laid-out text is not
//! the DOM's text: whitespace has been collapsed across node boundaries, a
//! case transform has rewritten letters, a `<br>` has contributed a newline no
//! node owns, and a list marker text that no node owns either. Parley records
//! which *element* styled each run — not which text node — so the mapping is
//! rebuilt here: the source characters are collected in the order Blitz pushed
//! them, and then aligned against the text Parley actually laid out. The
//! alignment is what a collapse is: every laid-out character consumes the
//! source character it came from, and a laid-out space consumes the whole run
//! of collapsible whitespace that collapsed into it.

use blitsen_dom::{CaretPosition, DomError, Rect};
use blitz::dom::node::NodeData;
use blitz::dom::{Node, NodeId};
use parley::{Affinity, Cluster, ClusterSide, Cursor, Selection};
use style::values::computed::TextTransform;
use style::values::specified::box_::{DisplayInside, DisplayOutside};

use crate::BlitzDom;

/// Whitespace CSS collapses a run of into one space.
///
/// A no-break space is deliberately not here: it is whitespace to Unicode and
/// to `char::is_whitespace`, and CSS keeps every one of them.
fn is_collapsible(character: char) -> bool {
    matches!(character, ' ' | '\t' | '\n' | '\r' | '\u{c}')
}

/// One character of the text an inline layout was built from.
#[derive(Clone, Copy)]
struct SourceChar {
    /// The text node it came from, or `None` for text no node owns.
    node: Option<NodeId>,
    /// Its offset in that node's data, in UTF-16 code units.
    offset: u32,
    /// Its length there, or zero for the tail of one a case transform expanded.
    width: u32,
    /// The character as it was pushed, after any case transform.
    character: char,
}

/// Where every character of an inline root's laid-out text came from.
struct Alignment {
    /// One entry per laid-out character: its byte offset in the laid-out text,
    /// and the source character it was laid out from.
    anchors: Vec<(usize, SourceChar)>,
    /// Byte length of the laid-out text.
    length: usize,
}

impl Alignment {
    /// The byte the characters of `node` at or after `offset` start at.
    ///
    /// `None` when the node laid out no text at all. An offset past everything
    /// the node laid out answers with the byte after its last character, which
    /// is what makes the end of a range measure the end of the run.
    fn byte_at(&self, node: NodeId, offset: u32) -> Option<usize> {
        let mut end = None;
        for (byte, source) in &self.anchors {
            if source.node != Some(node) {
                continue;
            }
            if source.offset >= offset {
                return Some(*byte);
            }
            end = Some(*byte + source.character.len_utf8());
        }
        end
    }

    /// The text node the character at a byte offset came from.
    ///
    /// Text no node owns — a `<br>`'s newline — belongs to whichever node
    /// laid out the next character, because that is where a caret at it goes.
    fn node_at(&self, byte: usize) -> Option<NodeId> {
        self.anchors
            .iter()
            .find(|(anchor, source)| *anchor >= byte && source.node.is_some())
            .and_then(|(_, source)| source.node)
            .or_else(|| {
                // Past the last character: the last node that laid one out,
                // which is what a click to the right of a line lands on.
                self.anchors
                    .iter()
                    .rev()
                    .find_map(|(_, source)| source.node)
            })
    }

    /// The offset in one node's data that a byte in the laid-out text names.
    ///
    /// Read against a node rather than against the text, because a boundary
    /// between two nodes is a position in both of them: the end of `AB` and the
    /// start of `CD` are the same byte, and which one the answer is depends on
    /// the character the caller was pointing at.
    fn offset_in(&self, node: NodeId, byte: usize) -> u32 {
        let mut end = 0;
        for (anchor, source) in &self.anchors {
            if source.node != Some(node) {
                continue;
            }
            if *anchor >= byte {
                return source.offset;
            }
            end = source.offset + source.width;
        }
        end
    }
}

impl BlitzDom {
    /// The inline root laying a node out, when one is doing so.
    ///
    /// A node in a `display: none` subtree has none: nothing laid it out, so
    /// there is no geometry to report rather than an empty box at the origin.
    fn inline_root(&self, node: NodeId) -> Result<Option<NodeId>, DomError> {
        Ok(self
            .node(node)?
            .inline_root_ancestor()
            .map(|root| root.id)
            .filter(|root| {
                self.node(*root)
                    .ok()
                    .and_then(Node::element_data)
                    .is_some_and(|element| element.inline_layout_data.is_some())
            }))
    }

    /// The viewport position of an inline root's content box, and its scale.
    ///
    /// Parley's coordinates are relative to that origin and in device pixels,
    /// which is the pair every rectangle out of a layout has to be resolved
    /// against. Taken the way Blitz takes it when it paints the same layout.
    fn inline_origin(&self, root: NodeId) -> Result<(f32, f32, f32), DomError> {
        let node = self.node(root)?;
        let layout = node.final_layout();
        let position = node.absolute_position(0.0, 0.0);
        let scale = node
            .element_data()
            .and_then(|element| element.inline_layout_data.as_ref())
            .map_or(1.0, |inline| inline.layout.scale());
        Ok((
            position.x + layout.border.left + layout.padding.left
                - self.document.viewport_scroll().x as f32,
            position.y + layout.border.top + layout.padding.top
                - self.document.viewport_scroll().y as f32,
            scale,
        ))
    }

    /// Rebuilds the mapping from an inline root's source text to its laid-out
    /// text.
    fn alignment(&self, root: NodeId) -> Result<Alignment, DomError> {
        let node = self.node(root)?;
        let inline = node
            .element_data()
            .and_then(|element| element.inline_layout_data.as_ref())
            .ok_or(DomError::InvalidNodeType)?;
        let mut source = Vec::new();
        self.collect_source(root, TextTransform::NONE, &mut source, true)?;
        Ok(align(&source, &inline.text))
    }

    /// Collects the text of an inline root in the order Blitz pushes it.
    ///
    /// This mirrors Blitz's inline construction, and has to: a character
    /// collected that Blitz did not push — the content of a `<button>`, which
    /// is laid out in an inline context of its own — would shift everything
    /// after it onto the wrong node.
    fn collect_source(
        &self,
        node: NodeId,
        inherited_transform: TextTransform,
        source: &mut Vec<SourceChar>,
        is_root: bool,
    ) -> Result<(), DomError> {
        let node = self.node(node)?;
        let transform = node
            .primary_styles()
            .map(|styles| styles.clone_text_transform() & TextTransform::CASE_TRANSFORMS)
            .unwrap_or(inherited_transform);
        let descend = |parent: &Node, source: &mut Vec<SourceChar>| -> Result<(), DomError> {
            for child in parent
                .before()
                .into_iter()
                .chain(parent.children.iter().copied())
                .chain(parent.after())
            {
                self.collect_source(child, transform, source, false)?;
            }
            Ok(())
        };
        match &node.data {
            NodeData::Text(text) => push_text(&text.content, inherited_transform, node.id, source),
            NodeData::Element(element) | NodeData::AnonymousBlock(element) => {
                if is_root {
                    // A list marker rendered inside the principal box is text in
                    // the same layout, and no node owns it.
                    if let Some(marker) = self.inside_list_marker(node) {
                        push_filler(&marker, source);
                    }
                    return descend(node, source);
                }
                let local = element.name.local.as_ref();
                let hidden = local == "input"
                    && element.attrs().iter().any(|attribute| {
                        attribute.name.local.as_ref() == "type" && &*attribute.value == "hidden"
                    });
                if hidden {
                    return Ok(());
                }
                let Some(display) = node.primary_styles().map(|styles| styles.clone_display())
                else {
                    return Ok(());
                };
                match (display.outside(), display.inside()) {
                    (DisplayOutside::None, DisplayInside::None) => {}
                    (DisplayOutside::None, DisplayInside::Contents) => descend(node, source)?,
                    (DisplayOutside::Inline, DisplayInside::Flow) => {
                        if local == "br" {
                            // Preserved rather than collapsed, which is why it
                            // is a source character and not a boundary.
                            push_filler("\n", source);
                        } else if !lays_out_as_a_box(local) {
                            descend(node, source)?;
                        }
                    }
                    // Anything else is an inline box: it has an inline context
                    // of its own, and none of its text is in this layout.
                    _ => {}
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// The text of a list marker rendered inside the element's principal box.
    fn inside_list_marker(&self, node: &Node) -> Option<String> {
        use blitz::dom::node::{ListItemLayoutPosition, Marker};

        let list_item = node.element_data()?.list_item_data.as_deref()?;
        if !matches!(list_item.position, ListItemLayoutPosition::Inside) {
            return None;
        }
        Some(match &list_item.marker {
            Marker::Char(character) => format!("{character} "),
            Marker::String(text) => text.clone(),
        })
    }

    /// Rectangles a run of a text node's characters occupies, one per line.
    pub(crate) fn text_node_rects(
        &self,
        node: NodeId,
        start: u32,
        end: u32,
    ) -> Result<Vec<Rect>, DomError> {
        let content = self
            .node(node)?
            .text_data()
            .ok_or(DomError::InvalidNodeType)?
            .content
            .clone();
        let length = content.encode_utf16().count() as u32;
        let (start, end) = (start.min(length), end.min(length));
        if start >= end {
            return Ok(Vec::new());
        }
        let Some(root) = self.inline_root(node)? else {
            return Ok(Vec::new());
        };
        let alignment = self.alignment(root)?;
        let (Some(from), Some(to)) = (
            alignment.byte_at(node, start),
            alignment
                .byte_at(node, end)
                .map(|byte| byte.min(alignment.length)),
        ) else {
            return Ok(Vec::new());
        };
        if from >= to {
            return Ok(Vec::new());
        }
        let (origin_x, origin_y, scale) = self.inline_origin(root)?;
        let root_node = self.node(root)?;
        let layout = &root_node
            .element_data()
            .and_then(|element| element.inline_layout_data.as_ref())
            .ok_or(DomError::InvalidNodeType)?
            .layout;
        let selection = Selection::new(
            Cursor::from_byte_index(layout, from, Affinity::Downstream),
            Cursor::from_byte_index(layout, to, Affinity::Downstream),
        );
        Ok(selection
            .geometry(layout)
            .into_iter()
            .map(|(box_, _)| Rect {
                x: origin_x + (box_.x0 as f32) / scale,
                y: origin_y + (box_.y0 as f32) / scale,
                width: ((box_.x1 - box_.x0) as f32) / scale,
                height: ((box_.y1 - box_.y0) as f32) / scale,
            })
            .filter(|rect| rect.width > 0.0 || rect.height > 0.0)
            .collect())
    }

    /// The character boundary a viewport point lands on.
    pub(crate) fn caret_at_point(
        &self,
        x: f32,
        y: f32,
    ) -> Result<Option<CaretPosition<NodeId>>, DomError> {
        let Some((_, _, _, target, _, _)) = self.ranked_hit(x, y)? else {
            return Ok(None);
        };
        // The box under the point is not the one holding the text: an inline
        // layout belongs to the block that established it, and that is however
        // many boxes up from whatever the hit landed on.
        let Some(root) = self.inline_root(target)? else {
            return Ok(None);
        };
        let (origin_x, origin_y, scale) = self.inline_origin(root)?;
        let node = self.node(root)?;
        let layout = &node
            .element_data()
            .and_then(|element| element.inline_layout_data.as_ref())
            .ok_or(DomError::InvalidNodeType)?
            .layout;
        let Some((cluster, side)) =
            Cluster::from_point(layout, (x - origin_x) * scale, (y - origin_y) * scale)
        else {
            return Ok(None);
        };
        // Which edge of the character the point fell on is which side of it the
        // caret goes, and the two swap over in right-to-left text.
        let leading = side == ClusterSide::Left;
        let byte = if cluster.is_rtl() == leading {
            cluster.text_range().end
        } else {
            cluster.text_range().start
        };
        // The character under the point decides the node, and the boundary
        // decides the offset within it. Reading the node off the boundary
        // instead would put a click on the right of `AB` into the `CD` beside
        // it, which is the same position in the text and the wrong node.
        let alignment = self.alignment(root)?;
        Ok(alignment
            .node_at(cluster.text_range().start)
            .map(|node| CaretPosition {
                node,
                offset: alignment.offset_in(node, byte),
            }))
    }
}

/// Whether an inline element is laid out as a box rather than as styled text.
///
/// Restated from Blitz's own list, which is private to it: a replaced element
/// and a form control have an inline context of their own, so the text inside
/// one is not in the layout being walked.
fn lays_out_as_a_box(local: &str) -> bool {
    matches!(
        local,
        "img" | "svg" | "canvas" | "video" | "embed" | "iframe" | "input" | "textarea" | "button"
    )
}

/// Appends a text node's characters, applying the case transform in force.
fn push_text(content: &str, transform: TextTransform, node: NodeId, source: &mut Vec<SourceChar>) {
    let mut offset = 0;
    for character in content.chars() {
        let width = character.len_utf16() as u32;
        let mut pushed = 0;
        let mut push = |character: char, source: &mut Vec<SourceChar>| {
            source.push(SourceChar {
                node: Some(node),
                offset,
                // Only the first stands for the DOM character: a transform that
                // expanded one letter into two did not lengthen the node's data.
                width: if pushed == 0 { width } else { 0 },
                character,
            });
            pushed += 1;
        };
        match transform {
            TextTransform::UPPERCASE => character.to_uppercase().for_each(|c| push(c, source)),
            TextTransform::LOWERCASE => character.to_lowercase().for_each(|c| push(c, source)),
            _ => push(character, source),
        }
        offset += width;
    }
}

/// Appends text that no DOM node owns.
fn push_filler(text: &str, source: &mut Vec<SourceChar>) {
    source.extend(text.chars().map(|character| SourceChar {
        node: None,
        offset: 0,
        width: 0,
        character,
    }));
}

/// Aligns the characters that were pushed against the ones that were laid out.
fn align(source: &[SourceChar], laid_out: &str) -> Alignment {
    let mut anchors = Vec::with_capacity(laid_out.len());
    let mut cursor = 0;
    // A laid-out space stands for whatever whitespace collapsed into it, so it
    // matches any of it rather than only itself.
    let matches = |source: char, laid_out: char| {
        source == laid_out || (laid_out == ' ' && is_collapsible(source))
    };
    for (byte, character) in laid_out.char_indices() {
        // Whitespace that collapsing removed outright — the space before a line
        // break, the indentation in front of a paragraph — was pushed and never
        // laid out, so it is passed over here.
        while cursor < source.len()
            && !matches(source[cursor].character, character)
            && is_collapsible(source[cursor].character)
        {
            cursor += 1;
        }
        let Some(&anchor) = source.get(cursor) else {
            break;
        };
        anchors.push((byte, anchor));
        cursor += 1;
        if character == ' ' && is_collapsible(anchor.character) {
            while cursor < source.len() && is_collapsible(source[cursor].character) {
                cursor += 1;
            }
        }
    }
    Alignment {
        anchors,
        length: laid_out.len(),
    }
}
