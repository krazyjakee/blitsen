//! Renderer-independent DOM interfaces.
//!
//! Blitz owns the live tree in the first backend.  This crate describes the
//! operations the bridge may perform without exposing Blitz types or keeping a
//! second, shadow DOM.

mod invalidation;
mod types;

use std::fmt;
use std::hash::Hash;

pub use invalidation::{
    FrameInvalidation, InvalidationMetrics, InvalidationMode, InvalidationTracker,
};
pub use types::{
    CANVAS_TAG, CanvasCommands, CanvasEncoding, CanvasSurface, CanvasTextMetrics, CanvasTextStyle,
    CaretPosition, DomError, DomName, HitTest, ImageState, LayoutMetrics, LayoutSnapshot,
    LinkState, MediaQueryMatch, NATIVE_VIEWPORT_BYTES_PER_PIXEL, NATIVE_VIEWPORT_TAG, Namespace,
    NodeId, NodeKind, Rect, SelectionDirection, TextEdit, TextMotion, TextSelection,
    ViewportSurface,
};

/// Boundary implemented by every DOM and renderer backend.
///
/// A backend's [`DomBackend::NodeId`] values are handles into its own tree, not
/// copies of nodes.  Every method must validate a handle before using it.
pub trait DomBackend {
    /// Opaque stable handle into the backend's authoritative tree.
    type NodeId: Copy + fmt::Debug + Eq + Hash;

    /// Returns the document root node.
    fn document(&self) -> Self::NodeId;
    /// Returns the document element, when one exists.
    fn document_element(&self) -> Option<Self::NodeId>;
    /// Returns the body element, when one exists.
    fn body(&self) -> Option<Self::NodeId>;
    /// Returns a node's kind, validating the handle.
    fn node_kind(&self, node: Self::NodeId) -> Result<NodeKind, DomError>;
    /// Returns an element's namespace-aware name.
    fn element_name(&self, node: Self::NodeId) -> Result<DomName, DomError>;

    /// Creates a detached element owned by this backend.
    fn create_element(&mut self, name: &DomName) -> Result<Self::NodeId, DomError>;
    /// Creates a detached text node owned by this backend.
    fn create_text(&mut self, text: &str) -> Result<Self::NodeId, DomError>;
    /// Appends a node, first detaching it from any existing parent.
    fn append_child(&mut self, parent: Self::NodeId, child: Self::NodeId) -> Result<(), DomError>;
    /// Inserts a node before an optional reference child.
    ///
    /// `None` has the same semantics as [`DomBackend::append_child`].
    fn insert_before(
        &mut self,
        parent: Self::NodeId,
        child: Self::NodeId,
        reference: Option<Self::NodeId>,
    ) -> Result<(), DomError>;
    /// Detaches a node without invalidating its handle.
    fn remove(&mut self, node: Self::NodeId) -> Result<(), DomError>;
    /// Replaces a node with another, detaching the replacement first.
    fn replace(&mut self, old: Self::NodeId, replacement: Self::NodeId) -> Result<(), DomError>;

    /// Returns a node's parent.
    fn parent(&self, node: Self::NodeId) -> Result<Option<Self::NodeId>, DomError>;
    /// Returns a snapshot of a node's children in tree order.
    fn children(&self, node: Self::NodeId) -> Result<Vec<Self::NodeId>, DomError>;
    /// Returns a node's previous sibling.
    fn previous_sibling(&self, node: Self::NodeId) -> Result<Option<Self::NodeId>, DomError>;
    /// Returns a node's next sibling.
    fn next_sibling(&self, node: Self::NodeId) -> Result<Option<Self::NodeId>, DomError>;
    /// Reports whether the node is currently connected to the document.
    fn is_connected(&self, node: Self::NodeId) -> Result<bool, DomError>;

    /// Returns an attribute value.
    fn attribute(&self, node: Self::NodeId, name: &DomName) -> Result<Option<String>, DomError>;
    /// Sets an attribute value and invalidates selector-dependent style.
    fn set_attribute(
        &mut self,
        node: Self::NodeId,
        name: &DomName,
        value: &str,
    ) -> Result<(), DomError>;
    /// Removes an attribute and returns whether it was present.
    fn remove_attribute(&mut self, node: Self::NodeId, name: &DomName) -> Result<bool, DomError>;

    /// Returns a form control's current value.
    ///
    /// This is the control's state, not its `value` content attribute. HTML
    /// makes the attribute the control's *default*: typing into a field, or
    /// assigning to this, moves one without the other. A backend renders from
    /// the state, so this is the answer that agrees with what is painted.
    fn form_value(&self, node: Self::NodeId) -> Result<String, DomError>;
    /// Replaces a form control's value and raises HTML's dirty value flag.
    ///
    /// The content attribute is left alone, and from here on it no longer
    /// tracks the control: a later attribute write is the default changing,
    /// not the value.
    fn set_form_value(&mut self, node: Self::NodeId, value: &str) -> Result<(), DomError>;
    /// Returns the selection inside a text control.
    ///
    /// A control the backend has not laid out yet has no caret to report and
    /// answers with a collapsed selection at the start, which is where HTML
    /// puts one before anything has moved it.
    fn form_selection(&self, node: Self::NodeId) -> Result<TextSelection, DomError>;
    /// Replaces the selection inside a text control, clamping it to the value.
    fn set_form_selection(
        &mut self,
        node: Self::NodeId,
        selection: TextSelection,
    ) -> Result<(), DomError>;
    /// Moves the caret inside a text control.
    ///
    /// `extend` keeps the anchor where it is, which is the difference between
    /// an arrow key and a shifted one. Reports whether the control had a caret
    /// to move: one that has never been laid out does not.
    fn move_form_selection(
        &mut self,
        node: Self::NodeId,
        motion: TextMotion,
        extend: bool,
    ) -> Result<bool, DomError>;
    /// Puts the caret at a point inside a text control's border box.
    ///
    /// `offset_x` and `offset_y` are CSS pixels from the control's top-left
    /// corner — the offsets a mouse event already carries. `extend` leaves the
    /// anchor alone, which is a shift-click or a drag rather than a click.
    fn move_form_caret_to_point(
        &mut self,
        node: Self::NodeId,
        offset_x: f32,
        offset_y: f32,
        extend: bool,
    ) -> Result<bool, DomError>;
    /// Applies one editing operation to a text control and raises HTML's dirty
    /// value flag, exactly as [`DomBackend::set_form_value`] does.
    fn edit_form_value(&mut self, node: Self::NodeId, edit: TextEdit<'_>)
    -> Result<bool, DomError>;
    /// Replaces the active IME preedit range in a text control.
    ///
    /// `cursor` is a pair of UTF-8 byte offsets within `text`, matching the
    /// native IME boundary. `None` hides the composition caret. Reports
    /// whether the control had an editor able to display the preedit.
    fn set_form_composition(
        &mut self,
        node: Self::NodeId,
        text: &str,
        cursor: Option<(usize, usize)>,
    ) -> Result<bool, DomError>;
    /// Replaces the active IME preedit with committed text.
    fn commit_form_composition(&mut self, node: Self::NodeId, text: &str)
    -> Result<bool, DomError>;
    /// Removes an active IME preedit without committing it.
    fn clear_form_composition(&mut self, node: Self::NodeId) -> Result<bool, DomError>;
    /// Returns the focused editable control and its viewport-relative caret.
    ///
    /// Native window backends use this rectangle to keep an IME candidate
    /// window beside the text being edited. A readonly control, a non-text
    /// control and an unlaid-out control return `None`.
    fn focused_form_cursor_area(&self) -> Option<(Self::NodeId, Rect)>;
    /// Focuses a node in the renderer, or clears focus when given nothing.
    ///
    /// Focus is the bridge's to decide — it runs the focus events and knows
    /// what is focusable — but the renderer has to be told, because a caret,
    /// a selection highlight and every `:focus` rule are painted from it.
    fn set_focused(&mut self, node: Option<Self::NodeId>) -> Result<(), DomError>;
    /// Returns an `<input>`'s checkedness or an `<option>`'s selectedness.
    ///
    /// One method because they are one concept: a boolean control state whose
    /// content attribute — `checked`, `selected` — is only its default.
    fn form_checked(&self, node: Self::NodeId) -> Result<bool, DomError>;
    /// Replaces checkedness or selectedness, leaving the attribute alone.
    fn set_form_checked(&mut self, node: Self::NodeId, checked: bool) -> Result<(), DomError>;

    /// Returns one inline CSS declaration by kebab-case property name.
    fn inline_style(&self, node: Self::NodeId, property: &str) -> Result<Option<String>, DomError>;
    /// Sets one inline CSS declaration, returning whether the value was valid.
    fn set_inline_style(
        &mut self,
        node: Self::NodeId,
        property: &str,
        value: &str,
    ) -> Result<bool, DomError>;
    /// Removes one inline CSS declaration and returns its previous value.
    fn remove_inline_style(
        &mut self,
        node: Self::NodeId,
        property: &str,
    ) -> Result<Option<String>, DomError>;
    /// Serializes the complete inline declaration block.
    fn inline_style_text(&self, node: Self::NodeId) -> Result<String, DomError>;
    /// Parses and replaces the complete inline declaration block. Invalid
    /// declarations are ignored according to CSS parsing rules.
    fn set_inline_style_text(&mut self, node: Self::NodeId, css: &str) -> Result<(), DomError>;

    /// Returns the owner node of every stylesheet the document cascades from,
    /// in the order the cascade applies them.
    ///
    /// The owner is the `<style>` or `<link>` element the sheet came from, which
    /// is the only handle CSSOM has on a sheet here: this backend has no
    /// stylesheet that belongs to no element.
    fn style_sheets(&self) -> Result<Vec<Self::NodeId>, DomError>;
    /// Returns the source text of each top-level rule of a `<style>` element's
    /// sheet, in order.
    ///
    /// The sheet's source *is* the element's text, so this is derived from the
    /// live tree on every call rather than from a parallel rule list.
    fn sheet_rules(&self, node: Self::NodeId) -> Result<Vec<String>, DomError>;
    /// Parses one rule and inserts it into a `<style>` element's sheet at
    /// `index`, rewriting the element's text so the cascade picks it up.
    ///
    /// Text that does not parse as exactly one rule is refused rather than
    /// dropped, and an out-of-range index is [`DomError::NotFound`].
    fn insert_sheet_rule(
        &mut self,
        node: Self::NodeId,
        rule: &str,
        index: usize,
    ) -> Result<(), DomError>;
    /// Deletes the rule at `index` from a `<style>` element's sheet.
    fn delete_sheet_rule(&mut self, node: Self::NodeId, index: usize) -> Result<(), DomError>;

    /// Returns concatenated descendant text using DOM `textContent` semantics.
    fn text_content(&self, node: Self::NodeId) -> Result<String, DomError>;
    /// Replaces a node's children with text and invalidates layout.
    fn set_text_content(&mut self, node: Self::NodeId, text: &str) -> Result<(), DomError>;
    /// Parses an HTML fragment in the supplied element's context.
    ///
    /// Returned nodes are detached but adopted by this backend and may be
    /// inserted using the normal mutation methods.
    fn parse_fragment(
        &mut self,
        context: Self::NodeId,
        html: &str,
    ) -> Result<Vec<Self::NodeId>, DomError>;
    /// Serializes a node's children as HTML.
    fn inner_html(&self, node: Self::NodeId) -> Result<String, DomError>;
    /// Serializes a node and its children as HTML.
    fn outer_html(&self, node: Self::NodeId) -> Result<String, DomError>;
    /// Contextually parses HTML and replaces a node's children with the
    /// adopted fragment.
    fn set_inner_html(&mut self, node: Self::NodeId, html: &str) -> Result<(), DomError>;

    /// Returns the first matching descendant, or `None`.
    fn query_selector(
        &self,
        root: Self::NodeId,
        selector: &str,
    ) -> Result<Option<Self::NodeId>, DomError>;
    /// Returns a static, tree-ordered snapshot of matching descendants.
    fn query_selector_all(
        &self,
        root: Self::NodeId,
        selector: &str,
    ) -> Result<Vec<Self::NodeId>, DomError>;
    /// Returns the first element with the exact `id` attribute value.
    fn get_element_by_id(&self, id: &str) -> Result<Option<Self::NodeId>, DomError>;

    /// Sets the clock CSS animations and transitions are sampled at, in seconds.
    ///
    /// Nothing here reads a clock of its own: the host hands the frame's
    /// timestamp in, which is what keeps a recorded or replayed frame sequence
    /// identical to the one that was captured. The value is read by the next
    /// [`DomBackend::flush_layout`], so a `@keyframes` animation advances once
    /// per laid-out frame and not at all without one.
    fn set_animation_time(&mut self, seconds: f64);
    /// Reports whether the document has animation left to run.
    ///
    /// A host that stops calling [`DomBackend::set_animation_time`] freezes
    /// every running animation mid-flight, so this is what a frame loop asks to
    /// know that it still owes the document a frame.
    fn is_animating(&self) -> bool;
    /// Resolves pending style and layout work and returns a current snapshot.
    fn flush_layout(&mut self) -> Result<LayoutSnapshot, DomError>;
    /// Reports whether a layout-dependent read would force synchronous work.
    fn layout_is_dirty(&self) -> bool;
    /// Returns border-box geometry after validating a layout snapshot.
    fn bounding_rect(&self, node: Self::NodeId, snapshot: LayoutSnapshot)
    -> Result<Rect, DomError>;
    /// Returns CSSOM box and scroll measurements from a validated snapshot.
    fn layout_metrics(
        &self,
        node: Self::NodeId,
        snapshot: LayoutSnapshot,
    ) -> Result<LayoutMetrics, DomError>;
    /// Returns a node's box fragments, one per line box it was broken across.
    ///
    /// A node with a box of its own has exactly one, and it is the rectangle
    /// [`DomBackend::bounding_rect`] returns. An inline element is not laid out
    /// as a box at all — it is a run of styled text inside its block — so one
    /// that wraps occupies a rectangle per line, and their union is the only
    /// thing a single rectangle could report.
    fn client_rects(
        &self,
        node: Self::NodeId,
        snapshot: LayoutSnapshot,
    ) -> Result<Vec<Rect>, DomError>;
    /// Returns the rectangles a run of characters inside a text node occupies.
    ///
    /// `start` and `end` are UTF-16 code-unit offsets into the node's data, the
    /// units a DOM `Range` counts in, and are clamped to the node's length. The
    /// result is one rectangle per line box the run was broken across, in line
    /// order; a run that laid out no glyphs — a text node inside a
    /// `display: none` subtree, or one that whitespace collapsing removed
    /// entirely — has no rectangles rather than an empty one at the origin.
    fn text_rects(
        &self,
        node: Self::NodeId,
        start: u32,
        end: u32,
        snapshot: LayoutSnapshot,
    ) -> Result<Vec<Rect>, DomError>;
    /// Returns the character boundary a viewport point lands on.
    ///
    /// This is the read `caretRangeFromPoint` is: the same laid-out text
    /// [`DomBackend::text_rects`] measures, asked the other way round. A point
    /// over a box that contains no text — or outside the document — has no
    /// answer rather than a nearest one.
    fn caret_position(
        &self,
        x: f32,
        y: f32,
        snapshot: LayoutSnapshot,
    ) -> Result<Option<CaretPosition<Self::NodeId>>, DomError>;
    /// Returns one resolved CSS property value from a validated snapshot.
    ///
    /// `None` distinguishes "this renderer has no value here" — an unknown
    /// property name, or a node the cascade never reached — from a property
    /// that genuinely resolves to the empty string. The read is snapshot gated
    /// because CSSOM resolves the box properties to their used values, which
    /// only layout knows.
    fn resolved_style(
        &self,
        node: Self::NodeId,
        property: &str,
        snapshot: LayoutSnapshot,
    ) -> Result<Option<String>, DomError>;

    /// Evaluates a CSS media query against the document's current device.
    ///
    /// This is the same evaluation the cascade performs for `@media`, so a
    /// feature the style engine does not implement is unknown here too, and an
    /// unknown feature makes the query not match.
    fn media_query(&mut self, query: &str) -> Result<MediaQueryMatch, DomError>;

    /// Sets one or both scroll axes without bubbling into an ancestor scroller.
    fn set_scroll_offset(
        &mut self,
        node: Self::NodeId,
        left: Option<f64>,
        top: Option<f64>,
        snapshot: LayoutSnapshot,
    ) -> Result<(), DomError>;
    /// Returns the topmost node and its propagation path after validating layout.
    fn hit_test(
        &self,
        x: f32,
        y: f32,
        snapshot: LayoutSnapshot,
    ) -> Result<Option<HitTest<Self::NodeId>>, DomError>;

    /// Returns an `<img>` element's decode state from a validated snapshot.
    ///
    /// A subresource is applied while layout resolves, so this read is snapshot
    /// gated like the geometry ones: without the flush an image whose bytes have
    /// already arrived would still report itself as loading.
    fn image_state(
        &self,
        node: Self::NodeId,
        snapshot: LayoutSnapshot,
    ) -> Result<ImageState, DomError>;

    /// Returns a `<link>` element's stylesheet loading state from a validated
    /// snapshot.
    ///
    /// Snapshot gated for a reason the geometry reads do not have: a sheet
    /// enters the cascade while layout resolves, so a handler told the sheet
    /// loaded before the flush would read a computed style the sheet had not
    /// reached yet.
    fn link_state(
        &self,
        node: Self::NodeId,
        snapshot: LayoutSnapshot,
    ) -> Result<LinkState, DomError>;

    /// Returns every connected [`NATIVE_VIEWPORT_TAG`] element in tree order.
    fn native_viewports(&self) -> Result<Vec<Self::NodeId>, DomError>;
    /// Returns a native viewport element's surface from a validated snapshot.
    fn native_viewport_surface(
        &self,
        node: Self::NodeId,
        snapshot: LayoutSnapshot,
    ) -> Result<ViewportSurface, DomError>;
    /// Replaces a native viewport's contents with tightly packed RGBA8 rows.
    ///
    /// The slice must be exactly [`ViewportSurface::byte_length`] long: a
    /// partial write has no meaning for a surface that is composited whole.
    fn write_native_viewport(&mut self, node: Self::NodeId, pixels: &[u8]) -> Result<(), DomError>;

    /// Returns a [`CANVAS_TAG`] element's backing store, creating it if needed.
    ///
    /// Not snapshot gated, and that is the difference between this and
    /// [`Self::native_viewport_surface`]: a viewport is sized by layout, so
    /// reading one before layout resolves would read the previous frame's box.
    /// A canvas is sized by its own content attributes and by nothing else, so
    /// the answer does not depend on a flush — which is what lets a canvas that
    /// has never been in the document be drawn on at all.
    fn canvas_surface(&mut self, node: Self::NodeId) -> Result<CanvasSurface, DomError>;
    /// Records one submission of 2D context drawing commands.
    fn submit_canvas(
        &mut self,
        node: Self::NodeId,
        commands: CanvasCommands<'_>,
    ) -> Result<(), DomError>;
    /// Reads a rectangle of a canvas back as straight-alpha RGBA8 rows.
    ///
    /// The rectangle may extend past the backing store, which `getImageData`
    /// allows: nothing was drawn there, so it reads transparent black.
    fn canvas_pixels(
        &mut self,
        node: Self::NodeId,
        x: f64,
        y: f64,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, DomError>;
    /// Encodes a whole canvas as an image file of the named type.
    fn encode_canvas(
        &mut self,
        node: Self::NodeId,
        mime_type: &str,
        quality: f64,
    ) -> Result<CanvasEncoding, DomError>;
    /// Encodes a whole canvas as a `data:` URL.
    fn canvas_data_url(
        &mut self,
        node: Self::NodeId,
        mime_type: &str,
        quality: f64,
    ) -> Result<String, DomError>;
    /// Measures a run of text in a 2D context's font.
    fn measure_canvas_text(
        &mut self,
        style: CanvasTextStyle<'_>,
        text: &str,
    ) -> Result<CanvasTextMetrics, DomError>;
    /// Answers `isPointInPath`, or `isPointInStroke` when `stroked`.
    ///
    /// The geometry is a slice of the same command encoding
    /// [`CanvasCommands::numbers`] carries, for the same reason: a path is a
    /// variable-length run of numbers and the two sides already agree on how to
    /// write one.
    fn canvas_contains(&mut self, stroked: bool, geometry: &[f64]) -> Result<bool, DomError>;
}
