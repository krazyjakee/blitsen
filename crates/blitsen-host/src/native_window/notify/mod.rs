//! Platform notification lifecycle.

#[cfg(target_os = "android")]
mod android;
#[cfg(not(target_os = "android"))]
mod desktop;

#[cfg(target_os = "android")]
pub(crate) use android::*;
#[cfg(not(target_os = "android"))]
pub(crate) use desktop::*;
