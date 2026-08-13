//! The shapes crossing the DOM boundary: handles, names, geometry and errors.
//!
//! Every one of these is a value the bridge and a backend both name. None of
//! them has behaviour a backend can override, which is what separates them
//! from [`crate::DomBackend`].

use std::error::Error;
use std::fmt;

/// Generational node handle in its stable wire representation.
///
/// The slot selects backend storage and the generation prevents a stale handle
/// from resolving to an unrelated node after that storage is reused. The
/// backend owns the tree; this is only how a handle is packed for the opaque
/// external of a JavaScript wrapper and unpacked again on the way back.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct NodeId {
    slot: u32,
    generation: u32,
}

impl NodeId {
    /// Creates a handle from its stable wire representation.
    pub const fn new(slot: u32, generation: u32) -> Self {
        Self { slot, generation }
    }

    /// Packs the handle for opaque storage in a JavaScript wrapper.
    pub const fn to_u64(self) -> u64 {
        (self.generation as u64) << 32 | self.slot as u64
    }

    /// Restores a handle previously produced by [`NodeId::to_u64`].
    pub const fn from_u64(value: u64) -> Self {
        Self {
            slot: value as u32,
            generation: (value >> 32) as u32,
        }
    }
}

/// Namespace of an element or attribute name.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Namespace {
    /// The HTML namespace.
    Html,
    /// The SVG namespace.
    Svg,
    /// The MathML namespace.
    MathMl,
    /// No namespace, used by ordinary HTML attributes.
    None,
    /// A namespace not known to the v0 bridge.
    Other(String),
}

/// A namespace-aware DOM name.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DomName {
    /// Namespace containing the name.
    pub namespace: Namespace,
    /// Namespace-local name.
    pub local: String,
}

impl DomName {
    /// Creates an HTML element name.
    pub fn html(local: impl Into<String>) -> Self {
        Self {
            namespace: Namespace::Html,
            local: local.into(),
        }
    }

    /// Creates a non-namespaced attribute name.
    pub fn attribute(local: impl Into<String>) -> Self {
        Self {
            namespace: Namespace::None,
            local: local.into(),
        }
    }
}

/// Kind of a node in the authoritative backend tree.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeKind {
    /// The document root.
    Document,
    /// An element.
    Element,
    /// A text node.
    Text,
    /// A comment node.
    Comment,
    /// A document fragment.
    Fragment,
}

/// A CSS-pixel rectangle returned by layout.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    /// Horizontal position relative to the viewport.
    pub x: f32,
    /// Vertical position relative to the viewport.
    pub y: f32,
    /// Rectangle width.
    pub width: f32,
    /// Rectangle height.
    pub height: f32,
}

/// CSSOM box and scroll measurements from one validated layout snapshot.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LayoutMetrics {
    /// Viewport-relative border box.
    pub rect: Rect,
    /// Content box, positioned relative to the border box's own origin.
    ///
    /// That origin is what `ResizeObserverEntry.contentRect` reports, and it is
    /// the one measurement `rect` and the client sizes below cannot express:
    /// they stop at the padding box.
    pub content_rect: Rect,
    /// Rounded border-box width.
    pub offset_width: f64,
    /// Rounded border-box height.
    pub offset_height: f64,
    /// Rounded padding-box width excluding any reserved scrollbar gutter.
    pub client_width: f64,
    /// Rounded padding-box height excluding any reserved scrollbar gutter.
    pub client_height: f64,
    /// Current element scroll offsets.
    pub scroll_left: f64,
    /// Current vertical element scroll offset.
    pub scroll_top: f64,
}

/// Local name of the native viewport element.
pub const NATIVE_VIEWPORT_TAG: &str = "blitsen-view";

/// Bytes per pixel of a native viewport surface.
///
/// Contents are RGBA8 with straight (unpremultiplied) alpha, which is what the
/// compositor samples, so a written frame reaches the screen without a
/// conversion pass in between.
pub const NATIVE_VIEWPORT_BYTES_PER_PIXEL: usize = 4;

/// The physical-pixel drawing surface behind one native viewport element.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ViewportSurface {
    /// Viewport-relative CSS-pixel border box the surface is composited into.
    pub rect: Rect,
    /// Surface width in physical pixels.
    pub width: u32,
    /// Surface height in physical pixels.
    pub height: u32,
    /// Physical pixels per CSS pixel at the last layout flush.
    pub device_pixel_ratio: f64,
    /// Counter incremented whenever the physical size or ratio changed.
    ///
    /// An application compares this against the value it last drew for to learn
    /// that a resize or a display-density change invalidated its own buffers.
    pub generation: u64,
}

impl ViewportSurface {
    /// Returns the byte length of one complete frame for this surface.
    pub const fn byte_length(self) -> usize {
        self.width as usize * self.height as usize * NATIVE_VIEWPORT_BYTES_PER_PIXEL
    }
}

/// Loading state and intrinsic size of one `<img>` element.
///
/// The three fields answer HTML's `naturalWidth`/`naturalHeight` and
/// `complete`. `errored` separates a request that finished badly from one still
/// in flight, which is the distinction `complete` alone cannot express and the
/// one that decides whether `load` or `error` fires.
///
/// No `Default`: every combination of these fields means something specific, so
/// the named constants below are the only way to build one from nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageState {
    /// Decoded width in CSS pixels; zero until the image is available.
    pub natural_width: u32,
    /// Decoded height in CSS pixels; zero until the image is available.
    pub natural_height: u32,
    /// Whether the element has finished loading, successfully or not.
    ///
    /// An element with no source has nothing to wait for and is complete.
    pub complete: bool,
    /// Whether the fetch was refused, or the bytes failed to decode.
    pub errored: bool,
}

impl ImageState {
    /// The state of an element with nothing to load.
    pub const IDLE: Self = Self {
        natural_width: 0,
        natural_height: 0,
        complete: true,
        errored: false,
    };

    /// The state of an element whose source is still in flight.
    pub const LOADING: Self = Self {
        natural_width: 0,
        natural_height: 0,
        complete: false,
        errored: false,
    };

    /// The state of an element whose source could not be fetched or decoded.
    pub const FAILED: Self = Self {
        natural_width: 0,
        natural_height: 0,
        complete: true,
        errored: true,
    };

    /// The state of an element showing a decoded image of the given size.
    pub const fn decoded(width: u32, height: u32) -> Self {
        Self {
            natural_width: width,
            natural_height: height,
            complete: true,
            errored: false,
        }
    }
}

/// Loading state of one `<link>` element's stylesheet.
///
/// The pair decides whether `load` or `error` fires, exactly as it does for
/// [`ImageState`]. There is no size to report and nothing corresponding to
/// `img.complete`, so a link that is not waiting on a sheet is [`LinkState::IDLE`]
/// rather than complete-and-loaded: a `rel` this renderer does not fetch owes no
/// event, and saying it loaded would be claiming a request that never happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkState {
    /// Whether the element is waiting on a sheet the renderer requested.
    ///
    /// False for a link with no `href`, and for one whose `rel` does not name a
    /// stylesheet — neither is loading anything, and neither ever will.
    pub pending: bool,
    /// Whether the request has finished, successfully or not.
    pub complete: bool,
    /// Whether the fetch was refused, or answered with nothing.
    pub errored: bool,
}

impl LinkState {
    /// The state of a link that is not loading a stylesheet.
    pub const IDLE: Self = Self {
        pending: false,
        complete: false,
        errored: false,
    };

    /// The state of a link whose sheet is still in flight.
    pub const LOADING: Self = Self {
        pending: true,
        complete: false,
        errored: false,
    };

    /// The state of a link whose sheet arrived and is in the cascade.
    pub const LOADED: Self = Self {
        pending: true,
        complete: true,
        errored: false,
    };

    /// The state of a link whose sheet could not be fetched.
    pub const FAILED: Self = Self {
        pending: true,
        complete: true,
        errored: true,
    };
}

/// One CSS media query evaluated against the current device.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaQueryMatch {
    /// The query as the CSS parser serializes it.
    ///
    /// A query the parser rejects serializes as `not all`, which is what CSS
    /// error handling turns an unparsable media query list into.
    pub media: String,
    /// Whether the query matches the device this document is rendered for.
    pub matches: bool,
}

/// Result of resolving a viewport point against the laid-out document.
#[derive(Clone, Debug, PartialEq)]
pub struct HitTest<N> {
    /// Deepest interactive DOM node at the point.
    pub target: N,
    /// Connected propagation path in root-to-target order.
    pub path: Vec<N>,
    /// Horizontal CSS-pixel coordinate within the target border box.
    pub offset_x: f32,
    /// Vertical CSS-pixel coordinate within the target border box.
    pub offset_y: f32,
}

/// The character boundary a viewport point resolves to.
///
/// Offsets crossing this boundary are UTF-16 code units, because that is what a
/// DOM `Range` counts and what a JavaScript string indexes by. The backend
/// counts them itself rather than handing back a byte offset the bridge would
/// have to convert against text it does not hold.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaretPosition<N> {
    /// The text node the point landed in.
    pub node: N,
    /// Offset of the boundary within that node, in UTF-16 code units.
    pub offset: u32,
}

impl Rect {
    /// Reports whether a viewport point lies inside the rectangle.
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
    }
}

/// Proof that style and layout were flushed at a particular tree revision.
///
/// Layout-dependent backend reads accept this token so an accidental stale
/// read cannot silently return geometry from a previous mutation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LayoutSnapshot {
    revision: u64,
}

impl LayoutSnapshot {
    /// Creates a snapshot token for a backend revision.
    pub fn new(revision: u64) -> Self {
        Self { revision }
    }

    /// Returns the tree revision represented by this snapshot.
    pub fn revision(self) -> u64 {
        self.revision
    }
}

/// Failure produced while accessing or mutating the DOM backend.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DomError {
    /// A node handle did not resolve to a live node.
    StaleNode,
    /// The requested operation is not valid for the node's kind.
    InvalidNodeType,
    /// A tree mutation would create an invalid hierarchy.
    HierarchyRequest,
    /// A reference child was not a child of the supplied parent.
    NotFound,
    /// A selector or HTML fragment could not be parsed.
    Syntax(String),
    /// Layout was read without a snapshot for the current revision.
    LayoutNotFlushed,
    /// The concrete renderer reported another failure.
    Backend(String),
}

impl fmt::Display for DomError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StaleNode => formatter.write_str("node handle is stale"),
            Self::InvalidNodeType => formatter.write_str("operation is invalid for this node type"),
            Self::HierarchyRequest => formatter.write_str("mutation would create an invalid tree"),
            Self::NotFound => formatter.write_str("reference node was not found"),
            Self::Syntax(message) => write!(formatter, "invalid DOM syntax: {message}"),
            Self::LayoutNotFlushed => formatter.write_str("layout has not been flushed"),
            Self::Backend(message) => formatter.write_str(message),
        }
    }
}

impl Error for DomError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangles_use_half_open_edges() {
        let rect = Rect {
            x: 10.0,
            y: 20.0,
            width: 30.0,
            height: 40.0,
        };
        assert!(rect.contains(10.0, 20.0));
        assert!(rect.contains(39.99, 59.99));
        assert!(!rect.contains(40.0, 60.0));
    }

    #[test]
    fn viewport_surfaces_are_measured_in_whole_physical_frames() {
        let surface = ViewportSurface {
            rect: Rect {
                x: 0.0,
                y: 0.0,
                width: 40.0,
                height: 20.0,
            },
            width: 80,
            height: 40,
            device_pixel_ratio: 2.0,
            generation: 3,
        };
        assert_eq!(surface.byte_length(), 80 * 40 * 4);
        assert_eq!(ViewportSurface::default().byte_length(), 0);
    }

    #[test]
    fn names_make_namespace_choice_explicit() {
        assert_eq!(DomName::html("div").namespace, Namespace::Html);
        assert_eq!(DomName::attribute("class").namespace, Namespace::None);
    }

    #[test]
    fn node_handles_have_a_stable_external_representation() {
        let node = NodeId::new(123, 456);
        assert_eq!(NodeId::from_u64(node.to_u64()), node);
    }
}
