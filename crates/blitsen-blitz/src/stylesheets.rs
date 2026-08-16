//! Stylesheet text: reading a sheet's source, and splitting it into rules.

use blitsen_dom::{DomBackend, DomError};
use blitz::dom::NodeId;
use style::context::QuirksMode;
use style::properties::PropertyId;
use style::servo_arc::Arc;
use style::shared_lock::SharedRwLock;
use style::stylesheets::{AllowImportRules, Origin, StylesheetContents, UrlExtraData};

use crate::BlitzDom;

impl BlitzDom {
    pub(crate) fn style_text(&self, node: NodeId) -> Result<String, DomError> {
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

    /// Returns the `<style>` element a CSSOM sheet operation may act on.
    ///
    /// A `<link>` sheet's source is a file this process fetched, not text in the
    /// tree, so there is nothing here to insert a rule into; saying so is the
    /// point of the error.
    pub(crate) fn sheet_owner(&self, node: NodeId) -> Result<(), DomError> {
        if self.is_tag(node, "style") {
            Ok(())
        } else if self.is_tag(node, "link") {
            Err(DomError::Backend(
                "the rules of a stylesheet loaded from a URL are not implemented".into(),
            ))
        } else {
            Err(DomError::InvalidNodeType)
        }
    }

    /// Writes a sheet's rules back as the owning element's text.
    ///
    /// Blitz reparses a `<style>` element whose text changed and re-registers
    /// the sheet with the stylist, so this — and not a rule list kept alongside
    /// — is what puts a scripted rule into the cascade.
    pub(crate) fn write_sheet_rules(
        &mut self,
        node: NodeId,
        rules: &[String],
    ) -> Result<(), DomError> {
        self.set_text_content(node, &rules.join("\n"))
    }

    /// Counts the rules Stylo makes of some CSS, using the cascade's own parser.
    ///
    /// The parser drops what it cannot understand, so text that yields no rule
    /// is text the cascade would have ignored — which is the difference between
    /// refusing a rule and accepting one that does nothing.
    pub(crate) fn parsed_rule_count(&self, css: &str) -> usize {
        let guard = self.document.guard().clone();
        let sheet = self.parse_stylesheet(css, &guard);
        let read = guard.read();
        sheet.rules.read_with(&read).0.len()
    }

    /// Parses CSS with the cascade's own parser, against this document's URL.
    ///
    /// The author origin and the refusal of `@import` are what make a sheet
    /// parsed here answer the way the one Stylo already holds would.
    pub(crate) fn parse_stylesheet(
        &self,
        css: &str,
        guard: &SharedRwLock,
    ) -> Arc<StylesheetContents> {
        StylesheetContents::from_str(
            css,
            UrlExtraData::from(self.document.url().clone()),
            Origin::Author,
            guard,
            None,
            None,
            QuirksMode::NoQuirks,
            AllowImportRules::No,
            None,
        )
    }

    /// Splits stylesheet source into the source text of each top-level rule.
    ///
    /// Slices of the original text rather than anything reserialized: a sheet
    /// that is rewritten to insert one rule must not quietly lose whatever the
    /// serializer would have dropped from the rules around it.
    pub(crate) fn split_css_rules(css: &str) -> Vec<String> {
        let bytes = css.as_bytes();
        let comment_end = |from: usize| match css[from..].find("*/") {
            Some(offset) => from + offset + 2,
            None => bytes.len(),
        };
        let mut rules = Vec::new();
        let mut index = 0;
        while index < bytes.len() {
            // Between rules: whitespace and comments belong to no rule, exactly
            // as a browser's rule list reports.
            if bytes[index].is_ascii_whitespace() {
                index += 1;
                continue;
            }
            if bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'*') {
                index = comment_end(index + 2);
                continue;
            }
            let start = index;
            let mut depth = 0usize;
            let mut end = bytes.len();
            while index < bytes.len() {
                match bytes[index] {
                    b'/' if bytes.get(index + 1) == Some(&b'*') => {
                        index = comment_end(index + 2);
                        continue;
                    }
                    quote @ (b'"' | b'\'') => {
                        index += 1;
                        while index < bytes.len() {
                            match bytes[index] {
                                b'\\' => index += 2,
                                byte if byte == quote => {
                                    index += 1;
                                    break;
                                }
                                _ => index += 1,
                            }
                        }
                        continue;
                    }
                    b'{' => depth += 1,
                    // A block rule ends at the brace that closes it; a statement
                    // at-rule (`@import`, `@charset`) ends at its semicolon.
                    b'}' => {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            index += 1;
                            end = index;
                            break;
                        }
                    }
                    b';' if depth == 0 => {
                        index += 1;
                        end = index;
                        break;
                    }
                    _ => {}
                }
                index += 1;
            }
            rules.push(css[start..end].trim().to_owned());
        }
        rules
    }

    /// Reads one value from the same parsed declaration block the cascade uses.
    ///
    /// In particular, this keeps semicolons and colons inside strings, URLs,
    /// escapes, comments and custom-property values out of the declaration
    /// boundary business entirely. Stylo has already parsed the style attribute
    /// and applied declaration order and `!important` before this read.
    pub(crate) fn style_property_value(
        &self,
        node: NodeId,
        property: &str,
    ) -> Result<Option<String>, DomError> {
        let element = self
            .node(node)?
            .element_data()
            .ok_or(DomError::InvalidNodeType)?;
        let Some(style) = &element.style_attribute else {
            return Ok(None);
        };
        let Ok(property) = PropertyId::parse_enabled_for_all_content(property) else {
            return Ok(None);
        };
        let guard = self.document.guard().read();
        let mut value = String::new();
        style
            .read_with(&guard)
            .property_value_to_css(&property, &mut value)
            .map_err(|error| DomError::Backend(error.to_string()))?;
        Ok((!value.is_empty()).then_some(value))
    }
}
