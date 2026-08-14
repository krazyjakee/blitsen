//! What a `<canvas>` and a `<blitsen-view>` are the same shape of.
//!
//! Both are replaced elements whose contents live in a `RefCell` the DOM bridge
//! also holds, and both are painted through Blitz's custom-widget seam. What
//! they draw is entirely different — one replays a recorded scene, the other
//! samples an uploaded image — but the bookkeeping around the drawing is not: a
//! revision counter the contents bump, and the revision a widget last recorded,
//! whose inequality is the whole of `Widget::requires_redraw`.
//!
//! That pairing is the reason this module exists rather than each widget
//! carrying its own copy. The counters only mean anything if every paint marks
//! the revision it painted, and a paint that forgets leaves the element either
//! redrawing every frame or never again. [`SurfaceWidget::begin_paint`] is the
//! only way to reach the contents from a paint, and it marks as it borrows, so
//! the invariant cannot be half-kept.

use std::cell::{Ref, RefCell, RefMut};
use std::rc::Rc;

/// Contents that can say when they last changed.
pub(crate) trait Surface {
    /// Increments whenever what should be painted differs from before.
    fn revision(&self) -> u64;
}

/// The bookkeeping half of a surface-backed custom widget.
pub(crate) struct SurfaceWidget<S> {
    state: Rc<RefCell<S>>,
    /// Revision of the contents last recorded into a scene.
    painted_revision: u64,
}

impl<S: Surface> SurfaceWidget<S> {
    /// Adopts shared contents, with nothing painted from them yet.
    pub(crate) fn new(state: Rc<RefCell<S>>) -> Self {
        Self {
            state,
            painted_revision: 0,
        }
    }

    /// Whether the contents have moved on since the last recorded scene.
    ///
    /// This is `Widget::requires_redraw` for every surface below.
    pub(crate) fn needs_repaint(&self) -> bool {
        self.state.borrow().revision() != self.painted_revision
    }

    /// Borrows the contents for a paint, marking this revision as painted.
    pub(crate) fn begin_paint(&mut self) -> Ref<'_, S> {
        let state = self.state.borrow();
        self.painted_revision = state.revision();
        state
    }

    /// Borrows the contents to change them, as an attribute change does.
    pub(crate) fn state_mut(&self) -> RefMut<'_, S> {
        self.state.borrow_mut()
    }
}
