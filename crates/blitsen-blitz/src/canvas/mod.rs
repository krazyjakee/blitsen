//! The surface behind one `<canvas>` element.
//!
//! Blitz owns the element's box — `<canvas>` is already a replaced element
//! upstream, sized from its `width`/`height` content attributes and defaulting
//! to 300×150 — and this module owns what is drawn inside it.
//!
//! Contents are a recorded [`Scene`], not pixels. `Widget::paint` returns an
//! anyrender command list that whichever backend is live replays: vello on the
//! GPU in the window, vello_cpu headless and in the tests. So a canvas costs no
//! rasterisation and no upload on the paint path, and it composites in the same
//! frame as the DOM at the element's own paint position — z-order, ancestor
//! `overflow` and `border-radius` come from that position rather than from a
//! second pass. This is what `<blitsen-view>` pays a full-frame RGBA upload for
//! (see [`crate::viewport`]), and the difference is that an application writes
//! that surface from JavaScript while a canvas's ops are recorded here.
//!
//! Rasterisation is still needed, but only where the specification demands a
//! readback — `getImageData`, `toDataURL` — and not once per frame.
//!
//! The backing store is in canvas pixels and is independent of the box: the
//! recorded scene is scaled into whatever CSS makes the element, exactly as a
//! browser scales a canvas whose attribute size and style size disagree.

use std::cell::RefCell;
use std::rc::Rc;

use anyrender::{PaintScene as _, RenderContext, Scene};
use blitsen_dom::{DomBackend as _, DomError, DomName};
use blitz::dom::node::ComputedStyles;
use blitz::dom::{NodeId, Widget};
use kurbo::Affine;

use crate::BlitzDom;

/// The tag whose elements carry a canvas.
pub(crate) const CANVAS_TAG: &str = "canvas";

/// The backing store a canvas has when its attributes do not say otherwise.
///
/// The same pair as the default object size Blitz lays the element out at, so
/// an unconfigured canvas draws at one backing-store pixel per CSS pixel.
const DEFAULT_SIZE: (u32, u32) = (300, 150);

/// Parses a `width`/`height` content attribute into a backing-store dimension.
///
/// HTML asks for a non-negative integer and falls back to the default for
/// anything else, which includes a negative number, a float and a unit.
fn dimension(value: Option<&str>, default: u32) -> u32 {
    value
        .and_then(|value| value.trim().parse::<u32>().ok())
        .unwrap_or(default)
}

/// The backing store and recorded contents shared with the DOM bridge.
#[derive(Debug)]
pub(crate) struct CanvasState {
    width: u32,
    height: u32,
    generation: u64,
    revision: u64,
    scene: Scene,
}

impl Default for CanvasState {
    fn default() -> Self {
        Self {
            width: DEFAULT_SIZE.0,
            height: DEFAULT_SIZE.1,
            generation: 0,
            revision: 0,
            scene: Scene::new(),
        }
    }
}

impl CanvasState {
    /// Adopts a backing-store size, reporting whether it changed.
    ///
    /// Contents recorded for the previous size are dropped. That is not an
    /// optimisation — HTML says assigning either dimension clears the canvas to
    /// transparent black — and it is why `generation` exists separately from
    /// `revision`: a resize replaces the surface, an ordinary draw does not.
    pub(crate) fn resize(&mut self, width: u32, height: u32) -> bool {
        if self.width == width && self.height == height {
            return false;
        }
        self.width = width;
        self.height = height;
        self.generation += 1;
        self.clear();
        true
    }

    /// Discards the recorded contents, leaving the canvas transparent black.
    pub(crate) fn clear(&mut self) {
        self.scene.reset();
        self.revision += 1;
    }

    /// Records drawing commands into the canvas's own coordinate space.
    // Called by the DOM bridge once the 2D context lands; until then the
    // tests are the only caller, and a warning here would be noise.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn record(&mut self, ops: impl FnOnce(&mut Scene)) {
        ops(&mut self.scene);
        self.revision += 1;
    }

    /// Reports the backing store size in canvas pixels.
    pub(crate) fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Reports how many times the backing store has been replaced.
    // Called by the DOM bridge once the 2D context lands; until then the
    // tests are the only caller, and a warning here would be noise.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
}

/// Paints one `<canvas>` element's recorded contents into the document's scene.
pub(crate) struct CanvasWidget {
    state: Rc<RefCell<CanvasState>>,
    /// Revision of the contents last recorded into a scene.
    painted_revision: u64,
}

impl CanvasWidget {
    pub(crate) fn new(state: Rc<RefCell<CanvasState>>) -> Self {
        Self {
            state,
            painted_revision: 0,
        }
    }
}

impl Widget for CanvasWidget {
    fn requires_redraw(&self) -> bool {
        self.state.borrow().revision != self.painted_revision
    }

    /// Tracks the two content attributes that own the backing store.
    ///
    /// Removing one restores the default rather than leaving the old value in
    /// place, because the attribute is the only thing that was holding it.
    fn attribute_changed(&mut self, name: &str, _old: Option<&str>, new: Option<&str>) {
        let mut state = self.state.borrow_mut();
        let (width, height) = state.size();
        match name {
            "width" => {
                state.resize(dimension(new, DEFAULT_SIZE.0), height);
            }
            "height" => {
                state.resize(width, dimension(new, DEFAULT_SIZE.1));
            }
            _ => {}
        }
    }

    fn paint(
        &mut self,
        _context: &mut dyn RenderContext,
        _styles: &ComputedStyles,
        width: u32,
        height: u32,
        _scale: f64,
    ) -> Scene {
        let mut scene = Scene::new();
        let state = self.state.borrow();
        self.painted_revision = state.revision;

        let (canvas_width, canvas_height) = state.size();
        // A canvas with no backing store has nowhere to draw and no ratio to
        // scale by; a browser renders it as nothing rather than as an error.
        if canvas_width == 0 || canvas_height == 0 {
            return scene;
        }
        // The recorded scene is in canvas pixels. Everything that makes the box
        // a different size — CSS, the display's density — arrives here as the
        // painted width and height, so one transform covers both.
        scene.append_scene(
            state.scene.clone(),
            Affine::scale_non_uniform(
                f64::from(width) / f64::from(canvas_width),
                f64::from(height) / f64::from(canvas_height),
            ),
        );
        scene
    }
}

impl BlitzDom {
    /// Gives every connected `<canvas>` a backing store, and forgets dead ones.
    ///
    /// Attaching is a tree mutation and so runs before layout resolves, for the
    /// reason [`BlitzDom::attach_native_viewports`] does. The initial
    /// `width`/`height` are read here rather than waiting for
    /// `Widget::attribute_changed`, which only fires for a mutation made after
    /// the widget is attached — a parsed document's attributes are already in
    /// place by then.
    pub(crate) fn attach_canvases(&mut self) -> Result<(), DomError> {
        for node in self.query_selector_all(self.document(), CANVAS_TAG)? {
            if self.canvases.contains_key(&node) {
                continue;
            }
            let width = self.attribute(node, &DomName::attribute("width"))?;
            let height = self.attribute(node, &DomName::attribute("height"))?;
            let state = Rc::new(RefCell::new(CanvasState {
                width: dimension(width.as_deref(), DEFAULT_SIZE.0),
                height: dimension(height.as_deref(), DEFAULT_SIZE.1),
                ..Default::default()
            }));
            self.document
                .mutate()
                .set_custom_widget(node, Box::new(CanvasWidget::new(Rc::clone(&state))));
            self.canvases.insert(node, state);
        }
        let dropped: Vec<NodeId> = self
            .canvases
            .keys()
            .copied()
            .filter(|node| self.document.get_node(*node).is_none())
            .collect();
        for node in dropped {
            self.canvases.remove(&node);
        }
        Ok(())
    }
}
