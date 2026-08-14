//! `pointer-events` values the cascade cannot parse, rewritten to `auto`.
//!
//! Stylo's `pointer-events` accepts `auto` and `none` and nothing else unless it
//! is built with the `gecko` feature, which needs Gecko's bindings and is not
//! something an embedder can turn on. Every other value the property has —
//! `all`, `visible`, `painted`, `fill`, `stroke` and the `visible*` pair — is
//! therefore an invalid declaration, and an invalid declaration is *dropped*:
//! the element keeps the value it inherited.
//!
//! That inverts the author's meaning wherever the two are used together, which
//! is the only way they are used. React Flow writes
//! `.react-flow__nodes { pointer-events: none }` with
//! `.react-flow__node { pointer-events: all }` inside it, so a node — and every
//! connection handle on it — inherited `none` and was transparent to hits, to
//! `elementFromPoint` and to the cursor (issue #128).
//!
//! All of the dropped values mean "this element takes hits"; only `none` does
//! not, and `auto` is how this engine spells it. The values are rewritten as CSS
//! enters the document rather than worked around at the hit test, because the
//! cascade is what has to agree: a rule the hit test honoured but
//! `getComputedStyle` denied would be a worse divergence than this one, which is
//! that a readback reports `auto` where a browser reports the author's keyword.
//!
//! Only a `pointer-events` declaration is touched. The property name has to be
//! followed by a colon, so `setProperty("pointer-events", "all")` — where the
//! two are separate strings — is not text this can see; that call arrives at
//! [`crate::BlitzDom::set_inline_style`] as a property and a value instead, and
//! is normalised there.

use blitsen_dom::{DomBackend, DomName};

use crate::BlitzDom;

impl BlitzDom {
    /// Rewrites the `pointer-events` values the parser has just dropped.
    ///
    /// Runs once, on the document the HTML parser produced, because that is the
    /// only CSS that never passes through [`DomBackend`]: Blitz parses a
    /// `<style>` element's text and an element's `style` attribute while it
    /// builds the tree. Everything written afterwards — by a script, by a
    /// CSS-in-JS shim, by a linked sheet arriving — is normalised where it
    /// arrives instead.
    ///
    /// Only elements whose text actually declares the property are rewritten, so
    /// a document that never mentions it is not touched at all and keeps the
    /// tree the parser built.
    pub(crate) fn normalize_pointer_events(&mut self) {
        let style = DomName::attribute("style");
        let Ok(sheets) = self.query_selector_all(self.document(), "style") else {
            return;
        };
        for node in sheets {
            let Ok(css) = self.text_content(node) else {
                continue;
            };
            if let Some(rewritten) = normalize_css(&css) {
                let _ = self.set_text_content(node, &rewritten);
            }
        }
        let Ok(styled) = self.query_selector_all(self.document(), "[style]") else {
            return;
        };
        for node in styled {
            let Ok(Some(css)) = self.attribute(node, &style) else {
                continue;
            };
            if let Some(rewritten) = normalize_css(&css) {
                let _ = self.set_attribute(node, &style, &rewritten);
            }
        }
    }
}

/// Rewrites CSS bytes on their way in from a subresource, when they are CSS.
///
/// A subresource handler sees fonts and images as well as stylesheets and is not
/// told which it has, so this asks the bytes: text that is not UTF-8 is not a
/// stylesheet, and text that never names the property has nothing to rewrite.
pub(crate) fn normalize_subresource(bytes: &[u8]) -> Option<Vec<u8>> {
    let css = std::str::from_utf8(bytes).ok()?;
    normalize_css(css).map(String::into_bytes)
}

/// The values stylo drops that mean the element is hit-testable.
///
/// An allow-list rather than "anything but `none`": a value this does not know
/// — a CSS-wide keyword, a `var()`, a typo — is left exactly as written, so the
/// cascade decides it, which is the only party entitled to.
const HIT_TESTABLE: [&str; 9] = [
    "all",
    "visible",
    "visiblepainted",
    "visiblefill",
    "visiblestroke",
    "painted",
    "fill",
    "stroke",
    "bounding-box",
];

const PROPERTY: &str = "pointer-events";

/// Rewrites the unparseable `pointer-events` values in a stylesheet or a
/// `style` attribute, or reports that there were none.
///
/// `None` rather than a copy of the input, so the ordinary case — CSS that
/// never mentions the property — costs one substring search and no allocation.
pub(crate) fn normalize_css(css: &str) -> Option<String> {
    if !contains_property(css) {
        return None;
    }
    let bytes = css.as_bytes();
    let mut rewritten = String::with_capacity(css.len());
    // Everything before this is already in `rewritten`, and nothing is until the
    // first value is replaced — which is also how "was anything rewritten" is
    // known without a second flag.
    let mut copied = 0;
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'*') {
            index = comment_end(bytes, index + 2);
            continue;
        }
        // A string is CSS content, not CSS: `content: "pointer-events: all"` is
        // text an author is drawing.
        if matches!(bytes[index], b'"' | b'\'') {
            index = string_end(bytes, index);
            continue;
        }
        let Some((value_start, value_end)) = declared_value_at(css, index) else {
            index += 1;
            continue;
        };
        let value = &css[value_start..value_end];
        if let Some(replacement) = normalize_value(value) {
            // The whitespace around the value is the author's formatting, and a
            // rewritten sheet is still a sheet somebody may read back.
            let keyword_start = value_start + (value.len() - value.trim_start().len());
            rewritten.push_str(&css[copied..keyword_start]);
            rewritten.push_str(replacement);
            copied = keyword_start + value.trim().len();
        }
        index = value_end;
    }
    (copied > 0).then(|| {
        rewritten.push_str(&css[copied..]);
        rewritten
    })
}

/// Rewrites one declared value, or reports that it is one the cascade can read.
pub(crate) fn normalize_value(value: &str) -> Option<&'static str> {
    HIT_TESTABLE
        .contains(&value.trim().to_ascii_lowercase().as_str())
        .then_some("auto")
}

/// Whether a property name is the one this module rewrites.
pub(crate) fn is_property(name: &str) -> bool {
    name.eq_ignore_ascii_case(PROPERTY)
}

/// Whether text mentions the property at all, ASCII case-insensitively.
fn contains_property(css: &str) -> bool {
    css.len() >= PROPERTY.len()
        && css
            .as_bytes()
            .windows(PROPERTY.len())
            .any(|window| window.eq_ignore_ascii_case(PROPERTY.as_bytes()))
}

/// The span of the value of a `pointer-events` declaration starting at `index`.
///
/// The value ends where the declaration does — at `;` or the `}` closing the
/// block — or at the `!` of `!important`, which is kept as written.
fn declared_value_at(css: &str, index: usize) -> Option<(usize, usize)> {
    let rest = css.get(index..)?;
    if !rest
        .as_bytes()
        .get(..PROPERTY.len())
        .is_some_and(|name| name.eq_ignore_ascii_case(PROPERTY.as_bytes()))
    {
        return None;
    }
    // `-webkit-pointer-events` and `--pointer-events` are other properties.
    if css[..index]
        .chars()
        .next_back()
        .is_some_and(|character| character.is_alphanumeric() || matches!(character, '-' | '_'))
    {
        return None;
    }
    let after_colon = rest[PROPERTY.len()..].trim_start().strip_prefix(':')?;
    let value_start = css.len() - after_colon.len();
    let value_end = value_start
        + after_colon
            .find([';', '}', '!'])
            .unwrap_or(after_colon.len());
    Some((value_start, value_end))
}

/// The index past the `*/` that closes a comment opened before `from`.
fn comment_end(bytes: &[u8], from: usize) -> usize {
    let mut index = from;
    while index + 1 < bytes.len() {
        if bytes[index] == b'*' && bytes[index + 1] == b'/' {
            return index + 2;
        }
        index += 1;
    }
    bytes.len()
}

/// The index past the quote closing the string that opens at `start`.
fn string_end(bytes: &[u8], start: usize) -> usize {
    let quote = bytes[start];
    let mut index = start + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            byte if byte == quote => return index + 1,
            _ => index += 1,
        }
    }
    bytes.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_declared_value_the_cascade_would_drop_is_rewritten() {
        assert_eq!(
            normalize_css(".node { pointer-events: all }").as_deref(),
            Some(".node { pointer-events: auto }")
        );
        assert_eq!(
            normalize_css("a{pointer-events:visiblePainted;color:red}").as_deref(),
            Some("a{pointer-events:auto;color:red}")
        );
        assert_eq!(
            normalize_css(".a { pointer-events : STROKE !important }").as_deref(),
            Some(".a { pointer-events : auto !important }")
        );
        assert_eq!(
            normalize_css(".pane { pointer-events: none } .node { pointer-events: all }")
                .as_deref(),
            Some(".pane { pointer-events: none } .node { pointer-events: auto }")
        );
    }

    #[test]
    fn anything_the_cascade_can_read_is_left_exactly_as_written() {
        for css in [
            ".a { color: red }",
            ".a { pointer-events: none }",
            ".a { pointer-events: auto }",
            ".a { pointer-events: inherit }",
            ".a { pointer-events: var(--hits) }",
            // Another property whose name ends in this one, and the value of a
            // property that is not this one.
            ".a { -webkit-pointer-events: all }",
            ".a { --pointer-events: all }",
            r#".a::after { content: "pointer-events: all" }"#,
        ] {
            assert_eq!(normalize_css(css), None, "rewrote {css}");
        }
    }

    #[test]
    fn a_property_and_a_value_arriving_apart_are_normalised_apart() {
        assert!(is_property("pointer-events"));
        assert!(is_property("Pointer-Events"));
        assert!(!is_property("pointer_events"));
        assert_eq!(normalize_value(" All "), Some("auto"));
        assert_eq!(normalize_value("none"), None);
        assert_eq!(normalize_value("auto"), None);
    }
}
