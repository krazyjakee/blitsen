//! Dynamically loaded JavaScriptCore support for the Phase 2 host.
//!
//! Production exports ship a pinned JavaScriptCore build, but load it through
//! its C ABI so recipients can replace the LGPL library without relinking the
//! Blitsen runtime. [`JavaScriptCore`] implements the engine-neutral
//! [`blitsen_js::JsEngine`] boundary without exposing JSC handles upstream.

mod engine;
mod ffi;

use std::{env, fmt, path::PathBuf};

use libloading::Library;

pub use engine::{JavaScriptCore, JscClass, JscValue, JscWeakRef};

/// Environment variable that overrides the JavaScriptCore shared library.
pub const LIBRARY_ENV: &str = "BLITSEN_JSC_LIBRARY";

/// A failure while locating, loading, or booting JavaScriptCore.
#[derive(Debug)]
pub enum Error {
    /// No usable library was found in the configured or platform-default locations.
    LibraryNotFound(Vec<(PathBuf, String)>),
    /// The library does not expose a required JavaScriptCore C API symbol.
    MissingSymbol {
        /// Name of the missing symbol.
        symbol: &'static str,
        /// Dynamic loader error.
        source: libloading::Error,
    },
    /// JavaScriptCore could not create a global context or callback class.
    ContextCreation,
    /// The acquisition smoke expression failed.
    Evaluation(String),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LibraryNotFound(attempts) => {
                write!(formatter, "could not load JavaScriptCore")?;
                for (path, error) in attempts {
                    write!(formatter, "\n  {}: {error}", path.display())?;
                }
                write!(
                    formatter,
                    "\nset {LIBRARY_ENV} to an ABI-compatible shared library"
                )
            }
            Self::MissingSymbol { symbol, source } => {
                write!(formatter, "JavaScriptCore is missing {symbol}: {source}")
            }
            Self::ContextCreation => write!(formatter, "JavaScriptCore did not create a context"),
            Self::Evaluation(error) => write!(formatter, "JavaScript evaluation failed: {error}"),
        }
    }
}

impl std::error::Error for Error {}

impl JavaScriptCore {
    /// Loads the configured library, or tries the conventional names for the host OS.
    pub fn load() -> Result<Self, Error> {
        if let Some(configured) = env::var_os(LIBRARY_ENV) {
            return Self::load_from(configured);
        }

        let mut attempts = Vec::new();
        for candidate in platform_library_candidates() {
            match Self::load_from(candidate) {
                Ok(runtime) => return Ok(runtime),
                Err(Error::LibraryNotFound(mut candidate_attempts)) => {
                    attempts.append(&mut candidate_attempts);
                }
                Err(error) => return Err(error),
            }
        }
        Err(Error::LibraryNotFound(attempts))
    }

    /// Loads an ABI-compatible JavaScriptCore shared library from `path`.
    pub fn load_from(path: impl AsRef<std::path::Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        // SAFETY: the handle is retained by Runtime, and ffi::Functions checks
        // every required public C API symbol before the context is created.
        let library = unsafe { Library::new(path) }.map_err(|error| {
            Error::LibraryNotFound(vec![(path.to_path_buf(), error.to_string())])
        })?;
        Self::from_library(library)
    }
}

#[cfg(target_os = "linux")]
fn platform_library_candidates() -> &'static [&'static str] {
    &[
        "libJavaScriptCore.so",
        "libjavascriptcoregtk-6.0.so.1",
        "libjavascriptcoregtk-4.1.so.0",
    ]
}

#[cfg(target_os = "macos")]
fn platform_library_candidates() -> &'static [&'static str] {
    &[
        "libJavaScriptCore.dylib",
        "/System/Library/Frameworks/JavaScriptCore.framework/JavaScriptCore",
    ]
}

#[cfg(target_os = "windows")]
fn platform_library_candidates() -> &'static [&'static str] {
    &["JavaScriptCore.dll"]
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn platform_library_candidates() -> &'static [&'static str] {
    &[]
}
