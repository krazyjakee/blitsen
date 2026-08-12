//! Form-control state: the value and checkedness HTML keeps beside the attributes.

use blitsen_dom::{DomBackend, DomName, Namespace};
use blitz::dom::NodeId;
use blitz::dom::node::SpecialElementData;

use crate::BlitzDom;

/// The `<option>` and `<textarea>` state Blitz will not settle by itself.
///
/// Walked at the end of a flush that had layout work to do, which is when a
/// control can have appeared or its defaults changed.
const UNSETTLED_CONTROLS: &str = "option, textarea";

/// Form-control state HTML keeps beside the content attributes.
///
/// An entry exists only for a node something has written, so a document nobody
/// scripts carries none. The flags are HTML's: once `value` or `checked` has
/// been assigned the matching attribute is only the default, and Blitz — which
/// writes the `value` attribute straight through to its editor — must not be
/// allowed to overwrite the state with it.
#[derive(Clone, Debug, Default)]
pub(crate) struct FormState {
    /// Assigned value; `Some` is HTML's dirty value flag.
    pub(crate) value: Option<String>,
    /// Assigned checkedness or selectedness; `Some` is the dirty checkedness flag.
    pub(crate) checked: Option<bool>,
    /// Whether the writes above still have to reach Blitz's own control state,
    /// which does not exist until the control has been laid out once.
    pub(crate) pending: bool,
}

impl BlitzDom {
    /// Returns the text Blitz's editor holds for a control, if it has one.
    ///
    /// The editor is built while layout resolves, so a control the renderer has
    /// never laid out has no state here yet and the caller falls back to the
    /// defaults the editor would have been built from.
    pub(crate) fn editor_text(&self, node: NodeId) -> Option<String> {
        Some(
            self.document
                .get_node(node)?
                .element_data()?
                .text_input_data()?
                .editor
                .text()
                .to_string(),
        )
    }

    /// Puts a value into Blitz's editor, and reports whether there was one.
    ///
    /// Writing here rather than into a store beside it is the point: the editor
    /// is what the renderer paints, so a value JavaScript sets is a value the
    /// user can see.
    pub(crate) fn write_editor_text(&mut self, node: NodeId, value: &str) -> bool {
        let Some(input) = self
            .document
            .get_node_mut(node)
            .and_then(|node| node.element_data_mut())
            .and_then(|element| element.text_input_data_mut())
        else {
            return false;
        };
        if input.editor.text() == value {
            return true;
        }
        input.editor.set_text(value);
        self.document
            .with_text_input(node, |mut driver| driver.refresh_layout());
        true
    }

    /// Returns the checkedness Blitz holds, if the control has any.
    pub(crate) fn checked_state(&self, node: NodeId) -> Option<bool> {
        self.document
            .get_node(node)?
            .element_data()?
            .checkbox_input_checked()
    }

    /// Writes checkedness into Blitz, and reports whether it took.
    ///
    /// An `<option>` is given the flag it does not otherwise have, because that
    /// is the flag `:checked` matches against — an option selected from script
    /// is then found by `select :checked` the way a browser finds it. Blitz
    /// paints this flag only on an `<input>`, so an option gains no appearance
    /// from carrying it.
    pub(crate) fn write_checked_state(&mut self, node: NodeId, checked: bool) -> bool {
        let selectable = self.is_tag(node, "option");
        let Some(element) = self
            .document
            .get_node_mut(node)
            .and_then(|node| node.element_data_mut())
        else {
            return false;
        };
        if let Some(state) = element.checkbox_input_checked_mut() {
            *state = checked;
            return true;
        }
        if selectable {
            element.special_data = SpecialElementData::CheckboxInput(checked);
            return true;
        }
        false
    }

    /// Settles the control state a resolved layout has just made writable.
    ///
    /// Two things land here. State assigned before the control existed is
    /// pushed into the editor or flag Blitz has now built, and every option and
    /// textarea is given the default Blitz does not derive for itself: an
    /// option's selectedness so `:checked` can match it, and a textarea's child
    /// text, which is its default value where an input has a `value` attribute.
    /// Returns whether anything changed enough to need laying out again.
    pub(crate) fn settle_form_controls(&mut self) -> bool {
        let mut relayout = false;
        for node in self
            .form_state
            .iter()
            .filter(|(_, state)| state.pending)
            .map(|(node, _)| *node)
            .collect::<Vec<_>>()
        {
            let state = self.form_state[&node].clone();
            let mut settled = false;
            if let Some(value) = &state.value {
                settled |= self.write_editor_text(node, value);
                relayout |= settled;
            }
            if let Some(checked) = state.checked {
                settled |= self.write_checked_state(node, checked);
            }
            if settled && let Some(state) = self.form_state.get_mut(&node) {
                state.pending = false;
            }
        }
        for node in self
            .query_selector_all(self.document(), UNSETTLED_CONTROLS)
            .unwrap_or_default()
        {
            // A control JavaScript has written keeps what it was given: that is
            // the dirty flag, and the default no longer reaches it.
            let state = self.form_state.get(&node);
            if self.is_tag(node, "option") {
                if state.and_then(|state| state.checked).is_some() {
                    continue;
                }
                let selected = self
                    .attribute(node, &DomName::attribute("selected"))
                    .is_ok_and(|value| value.is_some());
                if self.checked_state(node) != Some(selected) {
                    self.write_checked_state(node, selected);
                }
            } else if state.and_then(|state| state.value.as_ref()).is_none()
                && let Ok(text) = self.text_content(node)
                && self.editor_text(node).is_some_and(|value| value != text)
            {
                relayout |= self.write_editor_text(node, &text);
            }
        }
        relayout
    }

    /// Puts back the control state a content attribute must not have written.
    ///
    /// Blitz writes the `value` attribute straight through to its editor, which
    /// is right until the value has been assigned: HTML's dirty value flag
    /// makes the attribute the default from then on, and only the default. A
    /// control nothing has written still follows its attribute, here as in a
    /// browser, which is also how the `checked` attribute reaches a checkbox
    /// Blitz only reads it for while parsing.
    pub(crate) fn restore_form_state(&mut self, node: NodeId, name: &DomName) {
        // Named before looked up: every attribute write in a framework render
        // reaches this, and only these three have anything to put back.
        if !matches!(name.local.as_str(), "value" | "checked" | "selected")
            || !matches!(name.namespace, Namespace::None | Namespace::Html)
        {
            return;
        }
        let state = self.form_state.get(&node);
        match name.local.as_str() {
            "value" => {
                if let Some(value) = state.and_then(|state| state.value.clone()) {
                    self.write_editor_text(node, &value);
                }
            }
            "checked" | "selected" => match state.and_then(|state| state.checked) {
                Some(checked) => {
                    self.write_checked_state(node, checked);
                }
                None => {
                    let present = self
                        .attribute(node, name)
                        .is_ok_and(|value| value.is_some());
                    self.write_checked_state(node, present);
                }
            },
            _ => {}
        }
    }
}
