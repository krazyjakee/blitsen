#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

/// The published Blitsen facade crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::VERSION;

    #[test]
    fn version_matches_the_package() {
        assert_eq!(VERSION, "0.0.1");
    }
}
