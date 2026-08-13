//! Hit testing: which node a viewport point lands on, in paint order.

use std::cmp::Ordering;

use blitsen_dom::DomError;
use blitz::dom::NodeId;
use kurbo::Point;
use style::computed_values::pointer_events::T as PointerEvents;
use style::computed_values::visibility::T as Visibility;
use style::values::computed::Overflow;

use crate::BlitzDom;

pub(crate) type HitCandidate = (Vec<i32>, usize, f32, f32);
pub(crate) type RankedHit = (Vec<i32>, usize, usize, NodeId, f32, f32);

pub(crate) fn compare_stacking_paths(left: &[i32], right: &[i32]) -> Ordering {
    (0..left.len().max(right.len()))
        .find_map(|index| {
            let ordering = left
                .get(index)
                .copied()
                .unwrap_or(0)
                .cmp(&right.get(index).copied().unwrap_or(0));
            (ordering != Ordering::Equal).then_some(ordering)
        })
        .unwrap_or(Ordering::Equal)
}

impl BlitzDom {
    pub(crate) fn hit_candidate(
        &self,
        target: NodeId,
        viewport_x: f32,
        viewport_y: f32,
    ) -> Result<Option<HitCandidate>, DomError> {
        // Up the *layout* tree, not the DOM tree.
        //
        // A box's `final_layout().location` is relative to the box that laid it
        // out, and that is not always its DOM parent: a block container with
        // both block and inline children wraps the inline runs in anonymous
        // block boxes, and an inline element's offset is then relative to the
        // anonymous box rather than to the element it is written inside.
        // Walking DOM parents skipped that box, so its offset was never
        // subtracted and every inline element inside one hit-tested as though
        // it sat at the anonymous box's origin — which put a control near the
        // bottom of a document in front of everything at the top of it.
        // `<div>…</div><p>…</p><input>` is enough to reproduce, and that is
        // ordinary markup, so this mis-routed real clicks and not just
        // `elementFromPoint`.
        let mut chain = vec![target];
        while let Some(parent) = self.layout_parent(*chain.last().expect("target starts chain"))? {
            chain.push(parent);
        }
        chain.reverse();

        let mut x = viewport_x;
        let mut y = viewport_y;
        let mut stacking_path = Vec::new();
        let mut depth = 0;
        for (index, id) in chain.into_iter().enumerate() {
            let node = self.node(id)?;
            let Some(styles) = node.primary_styles() else {
                continue;
            };
            if matches!(
                styles.clone_visibility(),
                Visibility::Hidden | Visibility::Collapse
            ) {
                return Ok(None);
            }
            if index > 0 {
                let layout = node.final_layout();
                x = x - layout.location.x + node.scroll_offset().x as f32;
                y = y - layout.location.y + node.scroll_offset().y as f32;
                if let Some(transform) = *node.transform() {
                    let point = transform.inverse() * Point::new(f64::from(x), f64::from(y));
                    x = point.x as f32;
                    y = point.y as f32;
                }
                depth += 1;
            }
            if node.z_index() != 0 || node.is_stacking_context_root(false) {
                stacking_path.push(node.z_index());
            }

            let layout = node.final_layout();
            let inside_x = x >= 0.0 && x < layout.size.width;
            let inside_y = y >= 0.0 && y < layout.size.height;
            if let Some(styles) = node.primary_styles() {
                let clips_x = styles.clone_overflow_x() != Overflow::Visible;
                let clips_y = styles.clone_overflow_y() != Overflow::Visible;
                if (clips_x && !inside_x) || (clips_y && !inside_y) {
                    return Ok(None);
                }
            }
        }
        let target = self.node(target)?;
        let layout = target.final_layout();
        let inside = x >= 0.0 && x < layout.size.width && y >= 0.0 && y < layout.size.height;
        let interactive = target
            .primary_styles()
            .is_some_and(|styles| styles.clone_pointer_events() != PointerEvents::None);
        Ok((inside && interactive).then_some((stacking_path, depth, x, y)))
    }
}
