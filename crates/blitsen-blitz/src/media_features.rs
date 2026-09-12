//! `prefers-reduced-motion`, evaluated by Blitsen because Stylo's Servo feature
//! table does not know it (#385).
//!
//! Stylo evaluates `@media` and `matchMedia` from one table of features, and
//! `prefers-reduced-motion` is not in the Servo half of it: to the cascade it
//! is an unknown feature, and an unknown feature never matches. The table is
//! not extensible from outside the crate, and the custom-media route
//! (`@custom-media --x true`) is compiled out behind a static preference. So
//! the feature is resolved here, on the way in, and re-resolved when the
//! preference changes.
//!
//! The rewrite replaces each `(prefers-reduced-motion …)` expression with a
//! stand-in the cascade *does* evaluate, chosen so that it is always true or
//! always false on any device and never written by hand: a `resolution` bound
//! of a thousandth of a device pixel. Which bound — `min-` or `max-` — carries
//! the answer, and which thousandth carries what the author wrote, so the text
//! `MediaQueryList.media` reports can be restored exactly. Stylesheets keep
//! the stand-in in their text, which is what lets a later change find and
//! flip it without the original.
//!
//! What this cannot do is make a `<link>` sheet's text re-parse in place: a
//! linked sheet is bytes the loader handed to the cascade, so a change reloads
//! it through the same loader.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use crate::pointer_events;

/// The feature this module resolves.
const FEATURE: &str = "prefers-reduced-motion";

/// How each spelling of the feature is encoded in its stand-in.
///
/// Distinct values per spelling so the serialized text can be restored to what
/// the author wrote rather than to whichever spelling happens to be true.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Spelling {
    /// `(prefers-reduced-motion: reduce)`
    Reduce,
    /// `(prefers-reduced-motion: no-preference)`
    NoPreference,
    /// `(prefers-reduced-motion)`, the boolean context.
    Boolean,
}

impl Spelling {
    const ALL: [Self; 3] = [Self::Reduce, Self::NoPreference, Self::Boolean];

    fn thousandths(self) -> &'static str {
        match self {
            Self::Reduce => "0.001dppx",
            Self::NoPreference => "0.002dppx",
            Self::Boolean => "0.003dppx",
        }
    }

    fn canonical(self) -> &'static str {
        match self {
            Self::Reduce => "(prefers-reduced-motion: reduce)",
            Self::NoPreference => "(prefers-reduced-motion: no-preference)",
            Self::Boolean => "(prefers-reduced-motion)",
        }
    }

    /// Whether this spelling matches under the given preference.
    fn matches(self, reduced: bool) -> bool {
        match self {
            Self::Reduce | Self::Boolean => reduced,
            Self::NoPreference => !reduced,
        }
    }

    /// The expression Stylo evaluates to the answer this spelling has.
    fn stand_in(self, reduced: bool) -> String {
        let bound = if self.matches(reduced) { "min" } else { "max" };
        format!("({bound}-resolution: {})", self.thousandths())
    }

    /// The spelling a stand-in at `index` encodes, and the stand-in's length.
    fn at(css: &str, index: usize) -> Option<(Self, usize)> {
        let rest = &css[index..];
        for spelling in Self::ALL {
            for bound in ["min", "max"] {
                let candidate = format!("({bound}-resolution: {})", spelling.thousandths());
                if rest.starts_with(&candidate) {
                    return Some((spelling, candidate.len()));
                }
            }
        }
        None
    }
}

/// The preference and the sheets it has been baked into, shared between a
/// document and the subresource handler that rewrites its linked sheets.
#[derive(Clone, Default)]
pub(crate) struct MediaState(Arc<Shared>);

#[derive(Default)]
struct Shared {
    reduced_motion: AtomicBool,
    /// Resolved URLs of linked sheets whose text was rewritten, which are the
    /// ones a change has to reload.
    rewritten_sheets: Mutex<HashSet<String>>,
}

impl MediaState {
    pub(crate) fn reduced_motion(&self) -> bool {
        self.0.reduced_motion.load(Ordering::Acquire)
    }

    pub(crate) fn set_reduced_motion(&self, reduced: bool) {
        self.0.reduced_motion.store(reduced, Ordering::Release);
    }

    /// Records that a linked sheet at `url` carried the feature.
    pub(crate) fn sheet_rewritten(&self, url: &str) {
        self.0
            .rewritten_sheets
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(url.to_owned());
    }

    /// Whether a linked sheet at `url` has to be reloaded on a change.
    pub(crate) fn sheet_was_rewritten(&self, url: &str) -> bool {
        self.0
            .rewritten_sheets
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(url)
    }

    /// Rewrites stylesheet text on its way into the cascade: the
    /// `pointer-events` values it cannot parse, and this feature.
    pub(crate) fn normalize_stylesheet(&self, css: &str) -> Option<String> {
        let hits = pointer_events::normalize_css(css);
        let source = hits.as_deref().unwrap_or(css);
        match rewrite(source, self.reduced_motion()) {
            Some(rewritten) => Some(rewritten),
            None => hits,
        }
    }
}

/// Rewrites every `prefers-reduced-motion` expression — and every stand-in
/// an earlier rewrite left — to the stand-in for `reduced`.
///
/// `None` when the text names neither, which is the common case and costs one
/// scan for the feature name.
pub(crate) fn rewrite(css: &str, reduced: bool) -> Option<String> {
    if !mentions(css, FEATURE) && !mentions(css, "-resolution: 0.00") {
        return None;
    }
    let bytes = css.as_bytes();
    let mut rewritten = String::with_capacity(css.len());
    let mut copied = 0;
    let mut changed = false;
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index = pointer_events::comment_end(bytes, index + 2);
                continue;
            }
            b'"' | b'\'' => {
                index = pointer_events::string_end(bytes, index);
                continue;
            }
            b'(' => {}
            _ => {
                index += 1;
                continue;
            }
        }
        let (spelling, end) = match Spelling::at(css, index) {
            Some((spelling, length)) => (spelling, index + length),
            None => match feature_at(css, index) {
                Some(found) => found,
                None => {
                    index += 1;
                    continue;
                }
            },
        };
        let stand_in = spelling.stand_in(reduced);
        if css[index..end] != stand_in {
            rewritten.push_str(&css[copied..index]);
            rewritten.push_str(&stand_in);
            copied = end;
            changed = true;
        }
        index = end;
    }
    changed.then(|| {
        rewritten.push_str(&css[copied..]);
        rewritten
    })
}

/// Whether rewritten text carries a stand-in, and so has to be revisited when
/// the preference changes.
pub(crate) fn carries_stand_in(css: &[u8]) -> bool {
    std::str::from_utf8(css).is_ok_and(|css| mentions(css, "-resolution: 0.00"))
}

/// Puts the author's spelling back into a query the cascade serialized.
pub(crate) fn restore(serialized: &str) -> String {
    let mut text = serialized.to_owned();
    for spelling in Spelling::ALL {
        for bound in ["min", "max"] {
            let stand_in = format!("({bound}-resolution: {})", spelling.thousandths());
            text = text.replace(&stand_in, spelling.canonical());
        }
    }
    text
}

/// The spelling of a `(prefers-reduced-motion …)` expression opening at
/// `index`, and the index past its closing parenthesis.
///
/// A value that is neither `reduce` nor `no-preference` is left to the
/// cascade, which treats it as the unknown it is.
fn feature_at(css: &str, index: usize) -> Option<(Spelling, usize)> {
    let inner = css[index + 1..].trim_start();
    let name_end = inner
        .find(|character: char| !character.is_ascii_alphanumeric() && character != '-')
        .unwrap_or(inner.len());
    if !inner[..name_end].eq_ignore_ascii_case(FEATURE) {
        return None;
    }
    let after_name = inner[name_end..].trim_start();
    let (spelling, after_value) = if let Some(after_colon) = after_name.strip_prefix(':') {
        let value = after_colon.trim_start();
        let value_end = value.find(')')?;
        let spelling = match value[..value_end].trim().to_ascii_lowercase().as_str() {
            "reduce" => Spelling::Reduce,
            "no-preference" => Spelling::NoPreference,
            _ => return None,
        };
        (spelling, &value[value_end..])
    } else {
        (Spelling::Boolean, after_name)
    };
    let close = after_value.strip_prefix(')')?;
    let end = css.len() - close.len();
    Some((spelling, end))
}

/// Whether text mentions `needle` at all, ASCII case-insensitively.
fn mentions(css: &str, needle: &str) -> bool {
    css.len() >= needle.len()
        && css
            .as_bytes()
            .windows(needle.len())
            .any(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_spelling_is_rewritten_to_a_stand_in_that_carries_it() {
        let css = "@media (prefers-reduced-motion: reduce) { * { animation: none } } \
                   @media (prefers-reduced-motion: no-preference) { .a { transition: 1s } } \
                   @media screen and (PREFERS-REDUCED-MOTION) { .b { color: red } }";
        assert_eq!(
            rewrite(css, true).as_deref(),
            Some(
                "@media (min-resolution: 0.001dppx) { * { animation: none } } \
                 @media (max-resolution: 0.002dppx) { .a { transition: 1s } } \
                 @media screen and (min-resolution: 0.003dppx) { .b { color: red } }"
            )
        );
        assert_eq!(
            rewrite(css, false).as_deref(),
            Some(
                "@media (max-resolution: 0.001dppx) { * { animation: none } } \
                 @media (min-resolution: 0.002dppx) { .a { transition: 1s } } \
                 @media screen and (max-resolution: 0.003dppx) { .b { color: red } }"
            )
        );
    }

    #[test]
    fn a_rewritten_sheet_is_flipped_in_place_by_the_next_rewrite() {
        let css = "@media ( prefers-reduced-motion : reduce ) { a { b: c } }";
        let reduced = rewrite(css, true).expect("rewritten");
        let restored = rewrite(&reduced, false).expect("flipped");
        assert_eq!(
            restored,
            "@media (max-resolution: 0.001dppx) { a { b: c } }"
        );
        assert_eq!(rewrite(&restored, false), None, "already the right answer");
        assert_eq!(rewrite(&restored, true).as_deref(), Some(reduced.as_str()));
    }

    #[test]
    fn text_without_the_feature_is_left_alone() {
        for css in [
            "",
            ".a { color: red }",
            "@media (min-width: 500px) { a { b: c } }",
            "@media (prefers-color-scheme: dark) { a { b: c } }",
            // A value the cascade would not understand is the cascade's to refuse.
            "@media (prefers-reduced-motion: sometimes) { a { b: c } }",
            // Strings and comments are not queries.
            ".a::after { content: \"(prefers-reduced-motion: reduce)\" }",
            "/* (prefers-reduced-motion: reduce) */ .a { b: c }",
            // Somebody's own resolution query is not a stand-in.
            "@media (min-resolution: 2dppx) { a { b: c } }",
        ] {
            assert_eq!(rewrite(css, true), None, "rewrote {css:?}");
        }
    }

    #[test]
    fn a_serialized_query_reads_as_the_author_wrote_it() {
        assert_eq!(
            restore("(min-resolution: 0.001dppx)"),
            "(prefers-reduced-motion: reduce)"
        );
        assert_eq!(
            restore("screen and (max-resolution: 0.002dppx) and (min-width: 1px)"),
            "screen and (prefers-reduced-motion: no-preference) and (min-width: 1px)"
        );
        assert_eq!(
            restore("not all and (max-resolution: 0.003dppx)"),
            "not all and (prefers-reduced-motion)"
        );
        assert_eq!(
            restore("(min-resolution: 2dppx)"),
            "(min-resolution: 2dppx)"
        );
    }

    #[test]
    fn the_shared_state_composes_both_rewrites() {
        let state = MediaState::default();
        state.set_reduced_motion(true);
        assert_eq!(
            state
                .normalize_stylesheet(
                    ".n { pointer-events: all } @media (prefers-reduced-motion) { .n { a: b } }"
                )
                .as_deref(),
            Some(".n { pointer-events: auto } @media (min-resolution: 0.003dppx) { .n { a: b } }")
        );
        assert_eq!(state.normalize_stylesheet(".n { color: red }"), None);
        assert!(!state.sheet_was_rewritten("file:///a.css"));
        state.sheet_rewritten("file:///a.css");
        assert!(state.sheet_was_rewritten("file:///a.css"));
    }
}
