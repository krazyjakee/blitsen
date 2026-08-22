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
//! the invariant cannot be half-kept. [`attach_widgets`] likewise owns the
//! shared attachment lifecycle: a detached-but-live node retains its state for
//! reparenting, while a node the document has dropped is swept from the map.

use std::cell::{Ref, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use blitsen_dom::{DomBackend as _, DomError};
use blitz::dom::{NodeId, Widget};

use crate::BlitzDom;

/// Attaches one kind of surface widget and forgets states whose nodes are stale.
///
/// Building the state and building the widget are two closures rather than one
/// because they happen at different moments. A `<canvas>` may have contents
/// before it is ever in the document — an application draws into one it made
/// with `createElement` and reads the pixels back — so the state can already
/// exist by the time the element first appears here, and only the widget is
/// owed. `is_attached` is what tells the two apart; without it a canvas that
/// was drawn on before it was inserted would be found in the map, skipped, and
/// never painted.
pub(crate) fn attach_widgets<S: Surface>(
    dom: &mut BlitzDom,
    tag: &str,
    states: fn(&mut BlitzDom) -> &mut HashMap<NodeId, Rc<RefCell<S>>>,
    mut make: impl FnMut(&mut BlitzDom, NodeId) -> Result<Rc<RefCell<S>>, DomError>,
    widget: fn(Rc<RefCell<S>>) -> Box<dyn Widget>,
) -> Result<(), DomError> {
    for node in dom.query_selector_all(dom.document(), tag)? {
        let state = match states(dom).get(&node) {
            Some(state) => Rc::clone(state),
            None => {
                let state = make(dom, node)?;
                states(dom).insert(node, Rc::clone(&state));
                state
            }
        };
        if state.borrow().is_attached() {
            continue;
        }
        state.borrow_mut().mark_attached();
        dom.document.mutate().set_custom_widget(node, widget(state));
    }

    // A detached node remains in the document arena while JavaScript holds it,
    // and keeps the same surface if it is reparented. Only a stale node is dead.
    let tracked: Vec<_> = states(dom).keys().copied().collect();
    for node in tracked {
        if dom.document.get_node(node).is_none() {
            states(dom).remove(&node);
        }
    }
    Ok(())
}

/// Contents that can say when they last changed, and whether anything paints
/// them yet.
pub(crate) trait Surface {
    /// Increments whenever what should be painted differs from before.
    fn revision(&self) -> u64;
    /// Whether the widget that paints these contents has been installed.
    fn is_attached(&self) -> bool;
    /// Records that the widget has been installed.
    fn mark_attached(&mut self);
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
}
