//! Selection of the explicit document-script check mode.
/// Whether the launcher requested a document-script check.
pub fn requested() -> bool {
    std::env::var("BLITSEN_STANDALONE_CHECK").is_ok_and(|value| value == "1")
}
