//! Runtime-neutral bridge between a DOM backend and JavaScript engine.

/// The distribution version compiled into native release artifacts.
///
/// Cargo workspace versions describe unpublished implementation crates and are
/// deliberately unrelated to the npm release. The release build helper sets
/// `BLITSEN_RELEASE_VERSION` from `packages/blitsen/package.json`; a direct
/// checkout build has no package identity and says so rather than reporting the
/// workspace's internal `0.0.0` version.
pub const RELEASE_VERSION: &str = match option_env!("BLITSEN_RELEASE_VERSION") {
    Some(version) => version,
    None => "checkout",
};

/// The native executable identity used by both command and runtime reports.
pub fn runtime_identity() -> String {
    format!("blitsen-runtime {RELEASE_VERSION}")
}

pub mod bundle;
pub mod frame;
pub mod replay;
mod scripts;
mod style;
mod window;
mod wrappers;

#[cfg(test)]
mod tests;

pub use scripts::*;
pub use style::*;
pub use window::*;
pub use wrappers::*;

#[cfg(test)]
mod release_version_tests {
    use super::{RELEASE_VERSION, runtime_identity};

    #[test]
    fn runtime_identity_uses_the_authoritative_release_version() {
        assert_eq!(
            runtime_identity(),
            format!("blitsen-runtime {RELEASE_VERSION}")
        );
    }

    #[test]
    fn an_unstamped_checkout_is_explicitly_unversioned() {
        if option_env!("BLITSEN_RELEASE_VERSION").is_none() {
            assert_eq!(RELEASE_VERSION, "checkout");
        }
    }
}
