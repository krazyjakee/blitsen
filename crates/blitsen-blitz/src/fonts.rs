//! The font collection the document and its canvases both shape from.
//!
//! Blitz builds its own [`FontContext`] when a document is created and keeps it
//! private, so a canvas that made one of its own would be a second font
//! collection: a second system-font scan at startup, a second copy of every
//! face in memory, and — the part that shows — no sight of the `@font-face`
//! fonts the document registered, so `ctx.font = "16px MyWebFont"` would
//! silently fall back while the same family rendered correctly in the DOM.
//!
//! So Blitsen builds it instead and hands Blitz a clone. A [`Collection`] made
//! shared keeps its registered families behind an `Arc` that every clone reads
//! through, which is what carries a `@font-face` registered by the document
//! into the canvas's copy. The source cache is shared for the same reason one
//! step lower down: a face loaded from disk once is loaded once.
//!
//! Blitz's bullet font is registered here because Blitz only registers it into
//! a context it built itself — supplying one and not registering it is what
//! makes every `<ul>` marker vanish.

use parley::FontContext;
use parley::fontique::{Blob, Collection, CollectionOptions, SourceCache};
use std::sync::Arc;

/// Builds the shared font context a document and its canvases share.
///
/// Returns the context to hand to Blitz and a clone for canvas shaping. The
/// clone is not a copy of the fonts: both read the same shared collection.
pub(crate) fn shared_context() -> (FontContext, FontContext) {
    let mut collection = Collection::new(CollectionOptions {
        shared: true,
        system_fonts: true,
    });
    collection.register_fonts(Blob::new(Arc::new(blitz::dom::BULLET_FONT) as _), None);
    let context = FontContext {
        collection,
        source_cache: SourceCache::new_shared(),
    };
    let canvas = context.clone();
    (context, canvas)
}
