//! Native platform services used below the bridge boundary.
//!
//! Capability the web has no spelling for, in a crate that knows nothing about
//! JavaScript. The `native:` modules in `blitsen-node` are a thin translation
//! of what is here, so the Phase 2 host swap re-wires the bridge rather than
//! rewriting the platform code behind it.

pub mod app;
pub mod clipboard;

use std::fmt;

/// A platform operation the running system refused, or does not offer at all.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlatformError(String);

impl PlatformError {
    /// Builds an error carrying `message`, which the bridge reports verbatim.
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    /// The message.
    pub fn message(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PlatformError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for PlatformError {}
