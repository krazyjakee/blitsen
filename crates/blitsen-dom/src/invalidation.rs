//! Turning mutation marks into the work one frame owes.
//!
//! Kept apart from the boundary itself: a backend consumes a
//! [`FrameInvalidation`] but never implements any of this, and the tracker is
//! the one piece here with behaviour rather than shape.

use std::collections::HashSet;
use std::hash::Hash;

/// Strategy used to turn dirty marks into frame work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InvalidationMode {
    /// Restyle and relayout only explicitly dirty nodes/subtrees.
    FineGrained,
    /// Documented v0 fallback when the backend cannot expose incremental work.
    FullDocumentFallback,
}

/// Observable style/layout work performed for one frame.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct InvalidationMetrics {
    /// Number of nodes restyled.
    pub restyled_nodes: usize,
    /// Number of nodes relaid out.
    pub relaid_out_nodes: usize,
}

/// Dirty-node plan consumed by a backend at the next frame boundary.
#[derive(Debug)]
pub struct FrameInvalidation<N> {
    /// Nodes requiring selector/cascade recomputation.
    pub style_nodes: HashSet<N>,
    /// Nodes whose layout subtrees may have changed.
    pub layout_nodes: HashSet<N>,
    /// Whether the backend must recompute the complete document.
    pub full_document: bool,
    /// Work counters for regression instrumentation.
    pub metrics: InvalidationMetrics,
}

/// Tracks mutation invalidation without maintaining a shadow DOM.
#[derive(Debug)]
pub struct InvalidationTracker<N> {
    mode: InvalidationMode,
    style_dirty: HashSet<N>,
    layout_dirty: HashSet<N>,
}

impl<N: Copy + Eq + Hash> InvalidationTracker<N> {
    /// Creates an empty tracker in the selected backend mode.
    pub fn new(mode: InvalidationMode) -> Self {
        Self {
            mode,
            style_dirty: HashSet::new(),
            layout_dirty: HashSet::new(),
        }
    }

    /// Marks selector/cascade data dirty without assuming layout changed.
    pub fn mark_style(&mut self, node: N) {
        self.style_dirty.insert(node);
    }

    /// Marks layout dirty and propagates the mark through every ancestor.
    pub fn mark_layout(&mut self, node: N, mut parent: impl FnMut(N) -> Option<N>) {
        let mut current = Some(node);
        while let Some(node) = current {
            if !self.layout_dirty.insert(node) {
                break;
            }
            current = parent(node);
        }
    }

    /// Drains dirty state into the next frame's work plan.
    ///
    /// `document_nodes` is used only by the full-document fallback to make its
    /// cost visible in metrics.
    pub fn take_frame(&mut self, document_nodes: usize) -> FrameInvalidation<N> {
        let dirty = !self.style_dirty.is_empty() || !self.layout_dirty.is_empty();
        let full_document = dirty && self.mode == InvalidationMode::FullDocumentFallback;
        let metrics = if full_document {
            InvalidationMetrics {
                restyled_nodes: document_nodes,
                relaid_out_nodes: document_nodes,
            }
        } else {
            InvalidationMetrics {
                restyled_nodes: self.style_dirty.len(),
                relaid_out_nodes: self.layout_dirty.len(),
            }
        };
        FrameInvalidation {
            style_nodes: std::mem::take(&mut self.style_dirty),
            layout_nodes: std::mem::take(&mut self.layout_dirty),
            full_document,
            metrics,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalidation_separates_style_and_propagated_layout_work() {
        let parents = [(3, 2), (2, 1)]
            .into_iter()
            .collect::<std::collections::HashMap<_, _>>();
        let mut dirty = InvalidationTracker::new(InvalidationMode::FineGrained);
        dirty.mark_style(4);
        dirty.mark_style(3);
        dirty.mark_layout(3, |node| parents.get(&node).copied());
        let frame = dirty.take_frame(100);

        assert_eq!(frame.style_nodes, HashSet::from([3, 4]));
        assert_eq!(frame.layout_nodes, HashSet::from([1, 2, 3]));
        assert_eq!(
            frame.metrics,
            InvalidationMetrics {
                restyled_nodes: 2,
                relaid_out_nodes: 3,
            }
        );
        assert!(!frame.full_document);
        let clean_frame = dirty.take_frame(100);
        assert!(clean_frame.style_nodes.is_empty());
        assert!(clean_frame.layout_nodes.is_empty());
        assert!(!clean_frame.full_document);
        assert_eq!(clean_frame.metrics, InvalidationMetrics::default());
    }

    #[test]
    fn layout_propagation_stops_before_looking_past_a_dirty_ancestor() {
        let parents = [(4, 3), (3, 2), (2, 1)]
            .into_iter()
            .collect::<std::collections::HashMap<_, _>>();
        let mut dirty = InvalidationTracker::new(InvalidationMode::FineGrained);
        dirty.mark_layout(3, |node| parents.get(&node).copied());

        let mut looked_up = Vec::new();
        dirty.mark_layout(4, |node| {
            looked_up.push(node);
            parents.get(&node).copied()
        });

        assert_eq!(looked_up, [4]);
        let frame = dirty.take_frame(100);
        assert_eq!(frame.layout_nodes, HashSet::from([1, 2, 3, 4]));
        assert_eq!(frame.metrics.relaid_out_nodes, 4);
    }

    #[test]
    fn full_layout_fallback_reports_its_true_frame_cost() {
        let mut dirty = InvalidationTracker::new(InvalidationMode::FullDocumentFallback);
        dirty.mark_style(9);
        let frame = dirty.take_frame(250);
        assert!(frame.full_document);
        assert_eq!(frame.metrics.restyled_nodes, 250);
        assert_eq!(frame.metrics.relaid_out_nodes, 250);
    }
}
