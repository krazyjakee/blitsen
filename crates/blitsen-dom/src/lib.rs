//! Renderer-independent DOM interfaces.

/// Boundary implemented by every DOM and renderer backend.
///
/// The first implementation will adapt Blitz. Bridge code must depend on this
/// trait so upstream data structures do not become its public object model.
pub trait DomBackend {}
