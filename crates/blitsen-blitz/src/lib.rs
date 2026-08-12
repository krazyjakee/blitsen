//! The concrete [`blitsen_dom::DomBackend`] implemented over Blitz.
//!
//! [`BlitzDom`] is the only place in Blitsen that translates renderer-neutral
//! DOM operations into Blitz calls. It owns one authoritative `HtmlDocument`;
//! no parallel tree or attribute store is maintained.

mod backend;
mod forms;
mod hit_test;
pub mod resources;
mod stylesheets;
mod tree;
mod viewport;

#[cfg(test)]
mod tests;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use blitsen_dom::{
    DomError, FrameInvalidation, InvalidationMetrics, InvalidationMode, InvalidationTracker,
};
use blitz::dom::{DocumentConfig, NodeId};
use blitz::html::{HtmlDocument, HtmlProvider};

use forms::FormState;
use resources::ResourceLog;
use viewport::{NATIVE_VIEWPORT_UA_CSS, ViewportState};

/// Upper bound on resolve passes one layout flush will spend chasing resources.
///
/// A synchronous provider hands bytes back from inside `resolve`, after the
/// pass that would have consumed them. Without another pass a `background-image`
/// discovered during style resolution would first paint one frame late. Each
/// pass can only uncover resources referenced by the previous one, so the bound
/// caps a chain of `@import`ed stylesheets that each pull in the next.
const RESOURCE_RESOLVE_PASSES: usize = 4;

/// Serializes a CSS-pixel length the way a resolved value is written.
///
/// Layout arithmetic is `f32`, so a used length can carry noise no browser
/// would ever print; two decimals is finer than any display can show and coarse
/// enough to hide it.
fn css_pixels(length: f32) -> String {
    format!("{}px", (f64::from(length) * 100.0).round() / 100.0)
}

/// A Blitz HTML document exposed only through Blitsen's DOM boundary.
pub struct BlitzDom {
    document: HtmlDocument,
    revision: u64,
    flushed_revision: u64,
    invalidation: InvalidationTracker<NodeId>,
    last_invalidation_metrics: InvalidationMetrics,
    last_frame_was_full_document: bool,
    js_references: HashMap<NodeId, u32>,
    native_viewports: HashMap<NodeId, Rc<RefCell<ViewportState>>>,
    resources: ResourceLog,
    form_state: HashMap<NodeId, FormState>,
    animation_time: f64,
}

impl BlitzDom {
    /// Parses an HTML document with the real Blitz fragment parser installed.
    ///
    /// The configured net provider is wrapped so subresource outcomes stay
    /// observable, and a configuration without one gets
    /// [`resources::LocalResources`] rather than Blitz's silent no-op provider.
    pub fn from_html(html: &str, mut config: DocumentConfig) -> Self {
        config.html_parser_provider = Some(Arc::new(HtmlProvider));
        let (provider, log) = resources::track(config.net_provider.take());
        config.net_provider = Some(provider);
        let mut dom = Self::new(HtmlDocument::from_html(html, config));
        dom.resources = log;
        dom
    }

    /// Wraps an existing Blitz document and installs the fragment parser.
    pub fn new(mut document: HtmlDocument) -> Self {
        document.set_html_parser_provider(Arc::new(HtmlProvider));
        document.add_user_agent_stylesheet(NATIVE_VIEWPORT_UA_CSS);
        let invalidation_mode = if document.incremental_layout() {
            InvalidationMode::FineGrained
        } else {
            InvalidationMode::FullDocumentFallback
        };
        Self {
            document,
            revision: 0,
            flushed_revision: u64::MAX,
            invalidation: InvalidationTracker::new(invalidation_mode),
            last_invalidation_metrics: InvalidationMetrics::default(),
            last_frame_was_full_document: false,
            js_references: HashMap::new(),
            native_viewports: HashMap::new(),
            resources: ResourceLog::default(),
            form_state: HashMap::new(),
            animation_time: 0.0,
        }
    }

    /// Returns the record of every subresource this document has requested.
    pub fn resources(&self) -> &ResourceLog {
        &self.resources
    }

    /// Aborts every subresource still loading, and reports how many that was.
    ///
    /// This is the renderer half of `window.stop()`. See [`ResourceLog::stop`]
    /// for why an aborted request is completed rather than abandoned. There is
    /// no parser half: a Blitsen document is parsed whole before any script
    /// runs, so by the time anything can call `stop()` there is nothing left to
    /// stop parsing.
    pub fn stop_loading(&self) -> usize {
        self.resources.stop()
    }

    /// Borrows the authoritative Blitz document for painting or inspection.
    pub fn document_ref(&self) -> &HtmlDocument {
        &self.document
    }

    /// Mutably borrows the authoritative document for renderer integration.
    ///
    /// Callers must not mutate the tree through this escape hatch. DOM bridge
    /// writes go through [`DomBackend`] so revision tracking remains sound.
    pub fn document_mut(&mut self) -> &mut HtmlDocument {
        &mut self.document
    }

    /// Returns the owned Blitz document for transfer to the window renderer.
    pub fn into_document(self) -> HtmlDocument {
        self.document
    }

    /// Retains a node while a JavaScript wrapper is live.
    pub fn retain_for_js(&mut self, node: NodeId) -> Result<(), DomError> {
        self.node(node)?;
        let count = self.js_references.entry(node).or_default();
        *count = count
            .checked_add(1)
            .ok_or_else(|| DomError::Backend("JavaScript node reference count overflow".into()))?;
        Ok(())
    }

    /// Releases a JavaScript wrapper and collects an otherwise-unowned subtree.
    pub fn release_from_js(&mut self, node: NodeId) -> Result<bool, DomError> {
        self.node(node)?;
        let count = self
            .js_references
            .get_mut(&node)
            .ok_or_else(|| DomError::Backend("node has no JavaScript reference".into()))?;
        *count = count
            .checked_sub(1)
            .ok_or_else(|| DomError::Backend("node has no JavaScript reference".into()))?;
        if *count == 0 {
            self.js_references.remove(&node);
        }
        Ok(self.collect_detached_tree(node))
    }

    /// Drains observable invalidation work for the next frame.
    pub fn take_frame_invalidation(&mut self) -> FrameInvalidation<NodeId> {
        let frame = self.invalidation.take_frame(self.document.tree().len());
        self.last_invalidation_metrics = frame.metrics;
        self.last_frame_was_full_document = frame.full_document;
        frame
    }

    /// Returns the restyle/relayout scope consumed by the latest layout flush.
    pub fn last_frame_invalidation(&self) -> (InvalidationMetrics, bool) {
        (
            self.last_invalidation_metrics,
            self.last_frame_was_full_document,
        )
    }
}
