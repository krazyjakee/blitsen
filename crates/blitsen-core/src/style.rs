//! Inline style, and the property-name mapping JavaScript uses.

use blitsen_dom::{DomBackend, DomError};

/// Inline-style operations required by `CSSStyleDeclaration` wrappers.
pub trait InlineStyleBackend {
    /// Stable node handle.
    type NodeId: Copy;

    /// Reads one kebab-case inline property.
    fn style_property(
        &self,
        node: Self::NodeId,
        property: &str,
    ) -> Result<Option<String>, DomError>;
    /// Attempts to set one property, returning false for an invalid value.
    fn style_set_property(
        &mut self,
        node: Self::NodeId,
        property: &str,
        value: &str,
    ) -> Result<bool, DomError>;
    /// Removes one property and returns its old value.
    fn style_remove_property(
        &mut self,
        node: Self::NodeId,
        property: &str,
    ) -> Result<Option<String>, DomError>;
    /// Serializes the declaration block.
    fn style_css_text(&self, node: Self::NodeId) -> Result<String, DomError>;
    /// Replaces the declaration block through the backend CSS parser.
    fn style_set_css_text(&mut self, node: Self::NodeId, css: &str) -> Result<(), DomError>;
}

impl<D: DomBackend> InlineStyleBackend for D {
    type NodeId = D::NodeId;

    fn style_property(
        &self,
        node: Self::NodeId,
        property: &str,
    ) -> Result<Option<String>, DomError> {
        self.inline_style(node, property)
    }

    fn style_set_property(
        &mut self,
        node: Self::NodeId,
        property: &str,
        value: &str,
    ) -> Result<bool, DomError> {
        self.set_inline_style(node, property, value)
    }

    fn style_remove_property(
        &mut self,
        node: Self::NodeId,
        property: &str,
    ) -> Result<Option<String>, DomError> {
        self.remove_inline_style(node, property)
    }

    fn style_css_text(&self, node: Self::NodeId) -> Result<String, DomError> {
        self.inline_style_text(node)
    }

    fn style_set_css_text(&mut self, node: Self::NodeId, css: &str) -> Result<(), DomError> {
        self.set_inline_style_text(node, css)
    }
}

/// Runtime-neutral `CSSStyleDeclaration` implementation for inline styles.
pub struct InlineStyleApi<'a, D: InlineStyleBackend> {
    backend: &'a mut D,
    node: D::NodeId,
}

impl<'a, D: InlineStyleBackend> InlineStyleApi<'a, D> {
    /// Wraps one element's inline declaration block.
    pub fn new(backend: &'a mut D, node: D::NodeId) -> Self {
        Self { backend, node }
    }

    /// Reads a camelCase JavaScript style property.
    pub fn get_js_property(&self, property: &str) -> Result<String, DomError> {
        self.get_property_value(&js_property_to_css(property))
    }

    /// Writes a camelCase JavaScript style property. Invalid CSS is ignored.
    pub fn set_js_property(&mut self, property: &str, value: &str) -> Result<(), DomError> {
        self.set_property(&js_property_to_css(property), value)
    }

    /// Implements `getPropertyValue` for a kebab-case or custom property.
    pub fn get_property_value(&self, property: &str) -> Result<String, DomError> {
        Ok(self
            .backend
            .style_property(self.node, property)?
            .unwrap_or_default())
    }

    /// Implements `setProperty`; invalid declarations do not throw.
    pub fn set_property(&mut self, property: &str, value: &str) -> Result<(), DomError> {
        self.backend
            .style_set_property(self.node, property, value)?;
        Ok(())
    }

    /// Implements `removeProperty`, returning the previous value.
    pub fn remove_property(&mut self, property: &str) -> Result<String, DomError> {
        Ok(self
            .backend
            .style_remove_property(self.node, property)?
            .unwrap_or_default())
    }

    /// Implements the `cssText` getter.
    pub fn css_text(&self) -> Result<String, DomError> {
        self.backend.style_css_text(self.node)
    }

    /// Implements the `cssText` setter through the backend CSS parser.
    pub fn set_css_text(&mut self, css: &str) -> Result<(), DomError> {
        self.backend.style_set_css_text(self.node, css)
    }
}

/// Maps a JavaScript camelCase style property to its CSS spelling.
pub fn js_property_to_css(property: &str) -> String {
    if property.starts_with("--") {
        return property.into();
    }
    if property == "cssFloat" {
        return "float".into();
    }
    let mut css = String::with_capacity(property.len() + 4);
    for (index, character) in property.chars().enumerate() {
        if character.is_ascii_uppercase() {
            if index == 0 || !css.ends_with('-') {
                css.push('-');
            }
            css.push(character.to_ascii_lowercase());
        } else {
            css.push(character);
        }
    }
    css
}
