//! Which cursor the shell shows for a point in the viewport.
//!
//! Blitz answers this from its own hover state, and its hover hit test walks
//! down the box tree, stopping at the first box whose rectangle misses the
//! point. Anything laid out past its parent's box is unreachable that way: an
//! absolutely positioned child of a zero-sized container, a handle translated
//! half outside the node it belongs to. The cursor then never changed over the
//! elements an author is most likely to have styled, which is what a React Flow
//! connection handle is (issue #128).
//!
//! The hit test Blitsen already routes pointer events through ranks every
//! element independently and honours transforms, clipping and paint order, so
//! the cursor is resolved from that one. The two can then never disagree about
//! what the pointer is over — the element that receives the click is the
//! element whose cursor is shown.

use blitsen_dom::DomError;
use blitz::dom::NodeId;
use cursor_icon::CursorIcon;
use style::values::computed::UserSelect;
use style::values::computed::ui::CursorKind;

use crate::BlitzDom;

impl BlitzDom {
    /// The cursor the window should show for a viewport point in CSS pixels.
    ///
    /// `None` is `cursor: none` — the author asked for no pointer at all — and
    /// is deliberately distinct from a point that resolves to the arrow.
    pub fn cursor_at(&self, x: f32, y: f32) -> Result<Option<CursorIcon>, DomError> {
        let Some((_, _, _, target, _, _)) = self.ranked_hit(x, y)? else {
            // Off every box is still inside the window, and a window with
            // nothing under the pointer shows the arrow.
            return Ok(Some(CursorIcon::Default));
        };
        // A hit on a character is a hit on the text node holding it, which is
        // the node an inline `<a>` or `<span>` hangs off. The box that was hit
        // is the block that laid the text out, so stopping there would lose
        // every inline element between the two.
        let text = self.text_at_point(target, x, y)?;
        let hit = text.unwrap_or(target);
        let node = self.node(hit)?;
        // `cursor` and `user-select` inherit, so the value on whatever was hit
        // is already the one the cascade resolved for that point. A text node
        // that stylo never gave styles to falls back to its block.
        let Some(styles) = node
            .primary_styles()
            .or_else(|| self.node(target).ok()?.primary_styles())
        else {
            return Ok(Some(CursorIcon::Default));
        };
        let keyword = styles.clone_cursor().keyword;
        if keyword != CursorKind::Auto {
            return Ok(icon_for(keyword));
        }
        // What `auto` means is decided by what was hit. A control that holds
        // text is editable everywhere inside it, including its padding.
        if self
            .node(target)?
            .element_data()
            .is_some_and(|element| element.text_input_data().is_some())
        {
            return Ok(Some(CursorIcon::Text));
        }
        // A link is a link anywhere inside it, so this is the ancestor walk and
        // not a test of the node that was hit. Up the DOM rather than the
        // layout tree: an inline `<a>` is not the box that laid its own text
        // out, so the layout parent of a hit character is the block around it.
        let mut current = Some(hit);
        while let Some(id) = current {
            if self.is_link(id)? {
                return Ok(Some(CursorIcon::Pointer));
            }
            current = self.node(id)?.parent;
        }
        // Text under the pointer is the caret, unless it cannot be selected.
        if text.is_some() {
            return Ok(Some(match styles.clone_user_select() {
                UserSelect::Auto | UserSelect::Text | UserSelect::All => CursorIcon::Text,
                UserSelect::None => CursorIcon::Default,
            }));
        }
        Ok(Some(CursorIcon::Default))
    }

    /// Whether an element is a hyperlink, which is `:any-link` and not `<a>`.
    ///
    /// Blitz's own `is_link` is the tag alone, so a bare `<a>` used as a
    /// heading anchor would show the hand. The same distinction is drawn in the
    /// user-agent sheet, which only paints an `<a href>` as a link.
    fn is_link(&self, node: NodeId) -> Result<bool, DomError> {
        let node = self.node(node)?;
        let Some(element) = node.element_data() else {
            return Ok(false);
        };
        Ok(matches!(element.name.local.as_ref(), "a" | "area")
            && element
                .attrs()
                .iter()
                .any(|attr| attr.name.local.as_ref() == "href"))
    }
}

/// The shell cursor a resolved `cursor` keyword names.
///
/// Blitz has this mapping too, and keeps it private; it is the CSS keyword list
/// against the `cursor-icon` enum, so there is nothing to share but the table.
fn icon_for(keyword: CursorKind) -> Option<CursorIcon> {
    Some(match keyword {
        // `auto` is resolved by the caller, which knows what was hit; an `auto`
        // arriving here is a caller that did not, and the arrow is what every
        // engine falls back to.
        CursorKind::Auto | CursorKind::Default => CursorIcon::Default,
        CursorKind::None => return None,
        CursorKind::Pointer => CursorIcon::Pointer,
        CursorKind::ContextMenu => CursorIcon::ContextMenu,
        CursorKind::Help => CursorIcon::Help,
        CursorKind::Progress => CursorIcon::Progress,
        CursorKind::Wait => CursorIcon::Wait,
        CursorKind::Cell => CursorIcon::Cell,
        CursorKind::Crosshair => CursorIcon::Crosshair,
        CursorKind::Text => CursorIcon::Text,
        CursorKind::VerticalText => CursorIcon::VerticalText,
        CursorKind::Alias => CursorIcon::Alias,
        CursorKind::Copy => CursorIcon::Copy,
        CursorKind::Move => CursorIcon::Move,
        CursorKind::NoDrop => CursorIcon::NoDrop,
        CursorKind::NotAllowed => CursorIcon::NotAllowed,
        CursorKind::Grab => CursorIcon::Grab,
        CursorKind::Grabbing => CursorIcon::Grabbing,
        CursorKind::EResize => CursorIcon::EResize,
        CursorKind::NResize => CursorIcon::NResize,
        CursorKind::NeResize => CursorIcon::NeResize,
        CursorKind::NwResize => CursorIcon::NwResize,
        CursorKind::SResize => CursorIcon::SResize,
        CursorKind::SeResize => CursorIcon::SeResize,
        CursorKind::SwResize => CursorIcon::SwResize,
        CursorKind::WResize => CursorIcon::WResize,
        CursorKind::EwResize => CursorIcon::EwResize,
        CursorKind::NsResize => CursorIcon::NsResize,
        CursorKind::NeswResize => CursorIcon::NeswResize,
        CursorKind::NwseResize => CursorIcon::NwseResize,
        CursorKind::ColResize => CursorIcon::ColResize,
        CursorKind::RowResize => CursorIcon::RowResize,
        CursorKind::AllScroll => CursorIcon::AllScroll,
        CursorKind::ZoomIn => CursorIcon::ZoomIn,
        CursorKind::ZoomOut => CursorIcon::ZoomOut,
    })
}
