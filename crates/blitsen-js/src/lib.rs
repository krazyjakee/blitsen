//! JavaScript-engine-independent interfaces.

/// Boundary implemented by every JavaScript host.
///
/// Phase 1 will implement this over Bun/Node-API. Phase 2 will implement it
/// over an embedded JavaScriptCore host. Bridge code must depend on this trait,
/// not either concrete runtime.
pub trait JsEngine {}
