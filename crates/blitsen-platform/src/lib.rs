//! Native platform services used below the bridge boundary.
//!
//! Capability the web has no spelling for, in a crate that knows nothing about
//! JavaScript. The `native:` modules in `blitsen-node` are a thin translation
//! of what is here, so the Phase 2 host swap re-wires the bridge rather than
//! rewriting the platform code behind it.
//!
//! Not every module is present on every platform, and a `cfg` here is a decision
//! rather than a gap waiting to be filled: `docs/PRODUCT.md` §7 says a capability
//! is absent rather than approximated, and a module that does not compile out is
//! one that answers honestly everywhere it is compiled in. Each `cfg` below
//! names the reason in one line and the module's own documentation carries the
//! argument. Android is where most of them bite, because none of the desktop
//! plumbing — XDG, an executable to re-exec, a selection owner — exists there
//! (#147).

// Absent on Android: the directories are the Activity's, relaunch has no
// executable to spawn, and the single-instance lock is the platform's own job.
#[cfg(not(target_os = "android"))]
pub mod app;
// Absent on Android: `arboard` has no backend there, and the service it would
// wrap refuses a read unless the application holds focus.
#[cfg(not(target_os = "android"))]
pub mod clipboard;
// Absent off the XDG portal platforms rather than approximated there; the
// module's own documentation says why.
#[cfg(all(unix, not(target_os = "macos")))]
pub mod dialog;
// Present everywhere, including Android: `sysinfo` reads the same `/proc` there
// as on Linux, and the facts it cannot get come back `None` by design rather
// than by omission. See the module docs.
pub mod os;

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
