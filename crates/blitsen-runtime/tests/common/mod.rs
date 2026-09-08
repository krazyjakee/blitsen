//! What every test that runs the real runtime binary needs.

use std::path::{Path, PathBuf};

/// The runtime this test was built alongside, or `None` when it was not built.
pub fn runtime_binary() -> Option<PathBuf> {
    let path = PathBuf::from(env!("CARGO_BIN_EXE_blitsen-runtime"));
    path.is_file().then_some(path)
}

/// The directory a test's fixtures are created under: below `target`, so a
/// large one does not fill a system `/tmp` mount.
pub fn scratch_root() -> PathBuf {
    let scratch = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
    std::fs::create_dir_all(&scratch).expect("temp directory root");
    scratch
}
