//! The caret, the selection and the editing behind `<input>` and `<textarea>`.
//!
//! All of it goes through Blitz's own editor rather than a store beside it, for
//! the same reason the value does: the editor is what the renderer paints, so a
//! range JavaScript selects is a range the user can see highlighted, and a caret
//! the user moved is one JavaScript reads back. It is also the only thing here
//! that knows where a grapheme, a word or a soft-wrapped line begins — which is
//! why a motion crosses this boundary named rather than as an offset the bridge
//! would have had to guess.
//!
//! The one piece of state that cannot live there is HTML's directionless
//! selection: an anchor and a focus say forward or backward and have no third
//! answer. That bit is kept beside the control and dropped the moment anything
//! moves the caret, so it can never outlive the range it was assigned to.

use blitsen_dom::{SelectionDirection, TextEdit, TextMotion, TextSelection};
use blitz::dom::NodeId;
use blitz::dom::node::TextInputData;

use crate::BlitzDom;

/// Counts the UTF-16 code units before a byte offset in the editor's text.
///
/// The DOM boundary speaks in code units because that is what `value.slice`
/// indexes by; the editor speaks in bytes. A byte offset that is not a
/// character boundary cannot come out of the editor, so one is treated as the
/// end of the text rather than allowed to panic.
fn utf16_offset(text: &str, byte: usize) -> u32 {
    text.get(..byte)
        .unwrap_or(text)
        .encode_utf16()
        .count()
        .try_into()
        .unwrap_or(u32::MAX)
}

/// The byte offset a UTF-16 offset names, clamped to the text.
///
/// An offset that falls inside a surrogate pair rounds down to the start of the
/// character it split, because the editor refuses an index that is not a
/// character boundary and silently doing nothing would be the worse answer.
fn byte_offset(text: &str, offset: u32) -> usize {
    let mut units = 0_u32;
    for (index, character) in text.char_indices() {
        if units >= offset {
            return index;
        }
        units += character.len_utf16() as u32;
        if units > offset {
            return index;
        }
    }
    text.len()
}

impl BlitzDom {
    fn text_input(&self, node: NodeId) -> Option<&TextInputData> {
        self.document
            .get_node(node)?
            .element_data()?
            .text_input_data()
    }

    /// Returns the selection the editor holds, in UTF-16 code units.
    ///
    /// A control that has never been laid out has no editor yet, and answers
    /// with a collapsed selection at the start — where HTML puts one before
    /// anything has moved it.
    pub(crate) fn editor_selection(&self, node: NodeId) -> TextSelection {
        let Some(input) = self.text_input(node) else {
            return TextSelection::default();
        };
        let text = input.editor.raw_text();
        let selection = input.editor.raw_selection();
        let anchor = utf16_offset(text, selection.anchor().index());
        let focus = utf16_offset(text, selection.focus().index());
        let direction = if anchor == focus {
            SelectionDirection::None
        } else if focus < anchor {
            SelectionDirection::Backward
        } else if self.directionless(node) {
            SelectionDirection::None
        } else {
            SelectionDirection::Forward
        };
        TextSelection {
            start: anchor.min(focus),
            end: anchor.max(focus),
            direction,
        }
    }

    /// Replaces the selection, and reports whether there was an editor to take
    /// it. A backward range is made by placing the caret at the end and
    /// extending back to the start, because that is the only way an anchor and
    /// a focus can record which end the user is moving.
    pub(crate) fn write_editor_selection(
        &mut self,
        node: NodeId,
        selection: TextSelection,
    ) -> bool {
        let Some(text) = self
            .text_input(node)
            .map(|input| input.editor.raw_text().to_owned())
        else {
            return false;
        };
        let end = byte_offset(&text, selection.end);
        let start = byte_offset(&text, selection.start).min(end);
        let backward = selection.direction == SelectionDirection::Backward;
        self.document.with_text_input(node, |mut driver| {
            if backward {
                driver.move_to_byte(end);
                driver.extend_selection_to_byte(start);
            } else {
                driver.select_byte_range(start, end);
            }
        });
        self.set_directionless(node, selection.direction == SelectionDirection::None);
        self.mutate(Some(node), None);
        true
    }

    /// Moves the caret, keeping the anchor when `extend` — which is the whole
    /// difference between an arrow key and a shifted one.
    pub(crate) fn move_editor_selection(
        &mut self,
        node: NodeId,
        motion: TextMotion,
        extend: bool,
    ) -> bool {
        let mut moved = false;
        self.document.with_text_input(node, |mut driver| {
            moved = true;
            match (motion, extend) {
                (TextMotion::Left, false) => driver.move_left(),
                (TextMotion::Left, true) => driver.select_left(),
                (TextMotion::Right, false) => driver.move_right(),
                (TextMotion::Right, true) => driver.select_right(),
                (TextMotion::Up, false) => driver.move_up(),
                (TextMotion::Up, true) => driver.select_up(),
                (TextMotion::Down, false) => driver.move_down(),
                (TextMotion::Down, true) => driver.select_down(),
                (TextMotion::WordLeft, false) => driver.move_word_left(),
                (TextMotion::WordLeft, true) => driver.select_word_left(),
                (TextMotion::WordRight, false) => driver.move_word_right(),
                (TextMotion::WordRight, true) => driver.select_word_right(),
                (TextMotion::LineStart, false) => driver.move_to_line_start(),
                (TextMotion::LineStart, true) => driver.select_to_line_start(),
                (TextMotion::LineEnd, false) => driver.move_to_line_end(),
                (TextMotion::LineEnd, true) => driver.select_to_line_end(),
                (TextMotion::TextStart, false) => driver.move_to_text_start(),
                (TextMotion::TextStart, true) => driver.select_to_text_start(),
                (TextMotion::TextEnd, false) => driver.move_to_text_end(),
                (TextMotion::TextEnd, true) => driver.select_to_text_end(),
            }
        });
        if moved {
            self.set_directionless(node, false);
            self.mutate(Some(node), None);
        }
        moved
    }

    /// Puts the caret where a point in the control's border box lands.
    pub(crate) fn move_editor_caret_to_point(
        &mut self,
        node: NodeId,
        offset_x: f32,
        offset_y: f32,
        extend: bool,
    ) -> bool {
        let Some((x, y)) = self.editor_point(node, offset_x, offset_y) else {
            return false;
        };
        self.document.with_text_input(node, |mut driver| {
            if extend {
                driver.extend_selection_to_point(x, y);
            } else {
                driver.move_to_point(x, y);
            }
        });
        self.set_directionless(node, false);
        self.mutate(Some(node), None);
        true
    }

    /// Maps a point in a control's border box into the editor's own space.
    ///
    /// Three corrections stand between the two: the padding and border the text
    /// starts inside, the vertical centring a single-line field gives its one
    /// line, and how far the control has scrolled its text to keep the caret in
    /// view. The result is in device pixels, which is the scale the editor lays
    /// its text out at.
    fn editor_point(&self, node: NodeId, offset_x: f32, offset_y: f32) -> Option<(f32, f32)> {
        let scale = self.document.viewport().scale();
        let node = self.document.get_node(node)?;
        let input = node.element_data()?.text_input_data()?;
        let layout = node.final_layout();
        let origin_x = layout.padding.left + layout.border.left;
        let mut origin_y = layout.padding.top + layout.border.top;
        if !input.is_multiline {
            let text_height = input
                .editor
                .try_layout()
                .map(|layout| layout.height() / layout.scale())
                .unwrap_or_default();
            origin_y += ((layout.content_box_height() - text_height) / 2.0).max(0.0);
        }
        let (scroll_x, scroll_y) = if input.is_multiline {
            (0.0, input.scroll_offset)
        } else {
            (input.scroll_offset, 0.0)
        };
        Some((
            (offset_x - origin_x + scroll_x) * scale,
            (offset_y - origin_y + scroll_y) * scale,
        ))
    }

    /// Applies one editing operation and raises HTML's dirty value flag.
    ///
    /// Typing into a field is an assignment to its value as far as HTML is
    /// concerned: from here on the `value` content attribute is the control's
    /// default and no longer its state, which is what stops the next render
    /// that writes the attribute from undoing what the user typed.
    pub(crate) fn edit_editor_value(&mut self, node: NodeId, edit: TextEdit<'_>) -> bool {
        let mut edited = false;
        self.document.with_text_input(node, |mut driver| {
            edited = true;
            match edit {
                TextEdit::Insert(text) => driver.insert_or_replace_selection(text),
                TextEdit::DeleteBackward => driver.backdelete(),
                TextEdit::DeleteForward => driver.delete(),
                TextEdit::DeleteWordBackward => driver.backdelete_word(),
                TextEdit::DeleteWordForward => driver.delete_word(),
            }
        });
        if !edited {
            return false;
        }
        let value = self.editor_text(node).unwrap_or_default();
        let state = self.form_state.entry(node).or_default();
        state.value = Some(value);
        state.pending = false;
        state.directionless = false;
        self.mutate(Some(node), Some(node));
        true
    }

    /// Tells the renderer which node is focused.
    ///
    /// Focus itself is the bridge's to decide — it runs the focus events and
    /// knows what is focusable — but nothing is painted from that decision
    /// until it arrives here: the caret, the selection highlight and every
    /// `:focus` rule are the renderer's, and it has to be told.
    pub(crate) fn set_focused_node(&mut self, node: Option<NodeId>) {
        let previous = self.document.get_focussed_node_id();
        if previous == node {
            return;
        }
        match node {
            Some(node) => {
                self.document.set_focus_to(node);
            }
            None => self.document.clear_focus(),
        }
        for target in [previous, node].into_iter().flatten() {
            self.mutate(Some(target), None);
        }
    }

    fn directionless(&self, node: NodeId) -> bool {
        self.form_state
            .get(&node)
            .is_some_and(|state| state.directionless)
    }

    fn set_directionless(&mut self, node: NodeId, directionless: bool) {
        self.form_state.entry(node).or_default().directionless = directionless;
    }
}
