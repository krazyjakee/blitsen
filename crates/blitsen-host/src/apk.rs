//! The application's files inside an APK, read where they lie (issue #144).
//!
//! The fourth shape an application arrives in. A directory is read off disk, a
//! bundle out of a byte range in the running executable, a dev server over HTTP
//! — and on Android the files are entries in `assets/` inside the APK, which is
//! a signed zip the platform mounts and never unpacks.
//!
//! # The decision: in place, not extracted
//!
//! **Read the assets through `AAssetManager` where they sit. Extract nothing to
//! app storage.**
//!
//! The alternative was the ordinary Android recipe: unpack `assets/` into the
//! activity's `filesDir` the first time the application starts, then read plain
//! files. What that buys is `File` semantics — a path, a descriptor, `seek`,
//! `mmap`, something to hand to a C library that only takes a filename. An
//! engine that needs those has to extract, and many do.
//!
//! Blitsen does not need any of them. The entire interface between the host and
//! its files is [`AppSource::read`]: a path in, owned bytes out. That is not an
//! accident of this module — [`blitsen_core::bundle::AppBundle`] already proves
//! it is sufficient, because a file inside an exported executable has no path
//! either, and every consumer downstream of `AppSource` was written against that
//! constraint on the six desktop targets.
//!
//! So extraction would buy nothing, and it costs three things the appended
//! bundle deliberately does not:
//!
//! - **A second copy of the application, permanently.** The APK stays on disk
//!   whatever else happens; an extracted tree is added to it, not substituted
//!   for it. A 40 MB application occupies 80 MB. The appended bundle's whole
//!   point is that the export is one file read in place, and the property that
//!   made it worth having on desktop is worth more on a phone.
//! - **A first-run delay proportional to the application's size.** Product
//!   requirement P2 is a cold start measured in milliseconds. Copying every
//!   asset before the first frame is the one startup cost that grows with the
//!   application, and it lands on the launch a user judges the product by.
//! - **A staleness problem the appended bundle cannot have.** An extracted tree
//!   is a cache, and a cache needs an invalidation rule: which copy is current
//!   after an update, what happens when a write is interrupted, what reads the
//!   half-extracted directory left behind by a process the system killed. None
//!   of that exists for a bundle, because the files and the code that reads them
//!   are the same file and cannot disagree. Assets read in place keep exactly
//!   that property: they and the `.so` are in the same signed APK.
//!
//! What in-place reading gives up is real but small. Every read is a copy out of
//! the archive rather than a mapping the kernel shares, and an asset Gradle
//! chose to deflate is inflated on each read rather than once. Both are
//! answerable at package time by storing the application's assets uncompressed,
//! which is the one packaging instruction this design asks of the build, and
//! neither is answerable at all once a first-run extraction is in the way.
//!
//! That instruction is now kept, and it took the packager with it. No tool in
//! the chain would express it — Gradle's `androidResources { noCompress += .. }`
//! would have, but `cargo apk` ties compression to the debug profile with no
//! override — so `blitsen build --android` writes the archive itself, every
//! entry stored. See `packages/blitsen/src/android-apk.mjs` (#148).
//!
//! # What the trailer did, and what does it instead
//!
//! [`blitsen_core::bundle`] carries a trailer because bytes appended to an
//! executable have to be **found**, **verified** and **enumerated**. None of
//! that design transfers — an APK is signed as a zip and there is nothing to
//! append to — but the three jobs still have to be done, and inside an APK each
//! already has an owner:
//!
//! - **Found** — the zip central directory, read by the platform. An asset is
//!   addressed by name, so the scan-backwards-for-a-magic-number that the
//!   append-then-sign ordering forced is simply absent.
//! - **Verified** — the APK signature. Scheme v2 and v3 sign the archive itself
//!   rather than each file inside it, and the platform refuses to install or run
//!   an APK whose contents do not match. This is strictly stronger than the
//!   trailer's SHA-256, which is a digest sitting in the same file as the bytes
//!   it describes and therefore proves damage rather than tampering. Nothing
//!   here recomputes a digest, because a second digest of the same bytes, stored
//!   next to them inside a container something else already signs, would be
//!   reassurance with nothing behind it.
//! - **Enumerated** — nobody. This is the one job that does not transfer, and it
//!   is why this module ships an index. `AAssetManager_openDir` lists the files
//!   in one directory and **not its subdirectories**, so there is no walk of
//!   `assets/` available from the NDK at all. An application's own layout is
//!   `assets/app.js`, `fonts/`, whatever its bundler emitted, so a listing that
//!   stops at the first directory is not a listing.
//!
//! Hence [`ASSET_INDEX`]: a small JSON file the packaging step writes beside the
//! application, recording every path and its length. **Reading needs no index**
//! — a path is opened by name — so an artifact without one still runs. What
//! needs it is anything that has to answer "what is in here": the standalone
//! check's asset count, and the report below that stands in for
//! `--bundle-report`.
//!
//! # `--bundle-report` and `--licenses`
//!
//! An APK is launched by the system with no argv, so neither flag can be typed
//! at an Android artifact. That does not make either obligation go away; it
//! moves where they are answered.
//!
//! `--bundle-report` becomes [`ApkAssets::report`], the same JSON with the
//! fields that exist and honest absences for the ones that do not: no
//! `payloadBytes`, no `digest`, no `verified`, because there is no payload, no
//! trailer, and the verification is the platform's. Issue #142's entry point can
//! print it; a packaging test can read it back out of an installed APK.
//!
//! `--licenses` stops being a property of the trailer and becomes what it always
//! actually was — a read of one known path. `blitsen.notices.txt.gz` ships
//! inside `assets/` exactly as it ships inside the appended section, travelling
//! with the artifact under the same signature, which is what the acceptance gate
//! in `docs/LICENSING.md` requires (issue #121). [`crate::app::notices`] reads it
//! from any of the four shapes.

use std::path::PathBuf;

use blitsen_core::bundle::is_safe_path;
use serde::Deserialize;

use crate::modules::{AppSource, DirectorySource, file_of};

/// Where inside an APK's `assets/` a Blitsen application is packaged.
///
/// Namespaced rather than laid at the root of `assets/` because Gradle merges
/// the assets of every library in the build into one directory, so the root is
/// shared with whatever else the graph brought.
pub const DEFAULT_ASSET_ROOT: &str = "blitsen";

/// The listing the packaging step writes beside the application's files.
pub const ASSET_INDEX: &str = "blitsen.assets.json";

/// The only index format this build writes, and the newest it reads.
pub const INDEX_VERSION: u32 = 1;

/// One file the index records.
#[derive(Clone, Debug, Deserialize)]
struct IndexEntry {
    /// Application-relative path, `/`-separated.
    path: String,
    /// Length in bytes.
    #[serde(default)]
    bytes: u64,
}

/// The index as it is written.
#[derive(Deserialize)]
struct Index {
    version: u32,
    #[serde(default)]
    files: Vec<IndexEntry>,
}

/// An application packaged into an APK's `assets/`, read in place.
///
/// Constructed on Android from the activity's asset manager, and on any target
/// from a directory standing in for `assets/` — which is what makes the provider
/// testable where no APK exists, and what a desktop spike of issue #142's entry
/// point can run against.
pub struct ApkAssets {
    /// Where inside `assets/` the application sits, without slashes at either
    /// end. Empty means the root.
    root: String,
    source: Source,
    /// The listing, when the package carries one.
    index: Option<Vec<IndexEntry>>,
}

/// Where the bytes come from.
///
/// No mutex, unlike [`blitsen_core::bundle::AppBundle`]'s file handle. That one
/// exists because a single seek-then-read on one descriptor is not atomic, and
/// Blitz may call the subresource provider from a worker thread. `AAssetManager`
/// is documented thread-safe and every `AAsset` handle is opened and closed
/// inside one call, so there is no shared cursor to serialise.
enum Source {
    /// The activity's assets, mapped out of the installed APK.
    #[cfg(target_os = "android")]
    Manager(ndk::asset::AssetManager),
    /// A directory standing in for `assets/`.
    Directory(DirectorySource),
}

impl ApkAssets {
    /// Reads the application under `root` in this activity's assets.
    ///
    /// What issue #142's `android_main` calls, with
    /// `AndroidApp::asset_manager()` and [`DEFAULT_ASSET_ROOT`].
    #[cfg(target_os = "android")]
    pub fn open(manager: ndk::asset::AssetManager, root: &str) -> Self {
        Self::with_index(Source::Manager(manager), root)
    }

    /// The same, from a raw `AAssetManager *`.
    ///
    /// Offered because the typed constructor only accepts the `ndk` major this
    /// crate resolved, and a consumer that reached its asset manager through a
    /// different `android-activity` may hold a different one. A pointer has no
    /// version.
    ///
    /// # Safety
    ///
    /// `manager` must be a valid `AAssetManager *` that outlives the returned
    /// value — the activity's own, which lives as long as the process.
    #[cfg(target_os = "android")]
    pub unsafe fn open_raw(manager: *mut std::ffi::c_void, root: &str) -> Option<Self> {
        let pointer = std::ptr::NonNull::new(manager.cast())?;
        Some(Self::open(
            unsafe { ndk::asset::AssetManager::from_ptr(pointer) },
            root,
        ))
    }

    /// Reads the application under `root` in a directory standing in for
    /// `assets/`.
    pub fn open_directory(assets: impl Into<PathBuf>, root: &str) -> Self {
        Self::with_index(Source::Directory(DirectorySource::new(assets)), root)
    }

    /// Reads the index once, at construction, the way a bundle reads its.
    ///
    /// A package without one is not an error: nothing about reading a file needs
    /// it, and refusing to start over a listing would fail an application that
    /// works.
    fn with_index(source: Source, root: &str) -> Self {
        let mut assets = Self {
            root: root.trim_matches('/').to_owned(),
            source,
            index: None,
        };
        assets.index = assets.read(ASSET_INDEX).and_then(|raw| {
            let index: Index = serde_json::from_slice(&raw).ok()?;
            let mut files = index.files;
            files.sort_by(|left, right| left.path.cmp(&right.path));
            (index.version <= INDEX_VERSION).then_some(files)
        });
        assets
    }

    /// Where inside `assets/` the application sits.
    pub fn root(&self) -> &str {
        &self.root
    }

    /// Whether the package carries a listing of its own files.
    pub fn is_indexed(&self) -> bool {
        self.index.is_some()
    }

    /// Every path the index records, in sorted order.
    ///
    /// Empty when there is no index, which says "this package was built without
    /// a listing" rather than "this application ships nothing" — the difference
    /// [`Self::is_indexed`] is there to report.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.index.iter().flatten().map(|entry| entry.path.as_str())
    }

    /// How many files the index records.
    pub fn len(&self) -> usize {
        self.index.as_ref().map_or(0, Vec::len)
    }

    /// Whether the index records no files, or there is no index.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// What `--bundle-report` prints, for an artifact with no trailer to print
    /// one from.
    ///
    /// Same shape and same first question — `bundled` — so a reader of one
    /// report can read the other. What is missing is missing on purpose:
    /// `payloadBytes` and `digest` describe a trailer, and `verified` describes
    /// a check the platform has already done against the APK signature. Naming
    /// them null would say Blitsen looked and found nothing; omitting them says
    /// the concept does not apply here, which is what is true.
    pub fn report(&self) -> serde_json::Value {
        let mut report = serde_json::json!({
            "source": "apk-assets",
            "bundled": true,
            "assetRoot": self.root,
            "indexed": self.is_indexed(),
        });
        if let Some(files) = &self.index {
            report["indexVersion"] = INDEX_VERSION.into();
            report["files"] = serde_json::Value::Array(
                files
                    .iter()
                    .map(|entry| serde_json::json!({ "path": entry.path, "bytes": entry.bytes }))
                    .collect(),
            );
        }
        report
    }

    /// The asset name an application-relative path addresses.
    ///
    /// `None` for anything that would leave the application. The check is here
    /// rather than at construction because there is no index to check up front —
    /// and it cannot lean on canonicalisation the way [`DirectorySource`] does,
    /// because an asset is not a path on a filesystem.
    fn asset_path(&self, path: &str) -> Option<String> {
        // The query is what a dev server would have answered; the file is what
        // an archive holds. Same rule the bundle reads by.
        let file = file_of(path);
        if !is_safe_path(file) {
            return None;
        }
        Some(if self.root.is_empty() {
            file.to_owned()
        } else {
            format!("{}/{file}", self.root)
        })
    }
}

impl AppSource for ApkAssets {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        let path = self.asset_path(path)?;
        match &self.source {
            #[cfg(target_os = "android")]
            Source::Manager(manager) => {
                let name = std::ffi::CString::new(path).ok()?;
                let mut asset = manager.open(&name)?;
                // Opened in streaming mode and copied out, rather than through
                // `AAsset_getBuffer`: the host's interface is owned bytes, so
                // the mapping would be copied from anyway, and streaming does
                // not require the asset to be stored uncompressed to work.
                let mut bytes = Vec::with_capacity(asset.length());
                std::io::Read::read_to_end(&mut asset, &mut bytes).ok()?;
                Some(bytes)
            }
            Source::Directory(directory) => directory.read(&path),
        }
    }

    fn contains(&self, path: &str) -> bool {
        match &self.source {
            // Opened and dropped without reading: whether a file is there is a
            // question the archive answers, and an entrypoint check should not
            // pull a document into memory to ask it.
            #[cfg(target_os = "android")]
            Source::Manager(manager) => self
                .asset_path(path)
                .and_then(|path| std::ffi::CString::new(path).ok())
                .is_some_and(|name| manager.open(&name).is_some()),
            Source::Directory(_) => self.read(path).is_some(),
        }
    }
}

impl std::fmt::Debug for ApkAssets {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ApkAssets")
            .field("root", &self.root)
            .field("indexed", &self.is_indexed())
            .field("files", &self.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An APK's `assets/` laid out as a directory, which is what it is once the
    /// platform has mounted it.
    fn packaged(name: &str, files: &[(&str, &[u8])]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("blitsen-apk-{}-{name}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        for (path, bytes) in files {
            let target = root.join(path);
            std::fs::create_dir_all(target.parent().expect("a parent")).unwrap();
            std::fs::write(target, bytes).unwrap();
        }
        root
    }

    fn index(files: &[(&str, u64)]) -> Vec<u8> {
        let entries: Vec<_> = files
            .iter()
            .map(|(path, bytes)| serde_json::json!({ "path": path, "bytes": bytes }))
            .collect();
        serde_json::to_vec(&serde_json::json!({ "version": 1, "files": entries })).unwrap()
    }

    #[test]
    fn an_application_under_the_asset_root_reads_by_its_own_relative_path() {
        let listing = index(&[("assets/app.js", 18), ("index.html", 5)]);
        let root = packaged(
            "read",
            &[
                ("blitsen/index.html", b"<p>hi"),
                ("blitsen/assets/app.js", b"globalThis.ran = 1"),
                ("blitsen/blitsen.assets.json", &listing),
                // Another library's assets, at the root Gradle merges into.
                ("other/thing.txt", b"not ours"),
            ],
        );
        let assets = ApkAssets::open_directory(&root, DEFAULT_ASSET_ROOT);

        assert_eq!(assets.read("index.html").unwrap(), b"<p>hi");
        assert_eq!(assets.read("assets/app.js").unwrap(), b"globalThis.ran = 1");
        assert!(assets.contains("assets/app.js"));
        assert!(!assets.contains("assets/missing.js"));
        // A query is what a server would have answered; the archive holds the
        // file, so the file is what is opened.
        assert_eq!(
            assets.read("assets/app.js?v=2").unwrap(),
            b"globalThis.ran = 1"
        );
        // The application is under its own root, so a sibling in `assets/` is
        // not reachable by naming it.
        assert!(assets.read("other/thing.txt").is_none());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn nothing_outside_the_application_is_reachable_by_asking_for_it() {
        let root = packaged(
            "escape",
            &[("blitsen/index.html", b"<p>hi"), ("secret.txt", b"no")],
        );
        let assets = ApkAssets::open_directory(&root, DEFAULT_ASSET_ROOT);
        for escape in [
            "../secret.txt",
            "assets/../../secret.txt",
            "/etc/passwd",
            "..\\secret.txt",
            "",
        ] {
            assert!(assets.read(escape).is_none(), "{escape} was served");
            assert!(!assets.contains(escape), "{escape} was found");
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_index_is_what_lets_an_apk_say_what_it_carries() {
        let listing = index(&[("assets/app.js", 18), ("index.html", 5)]);
        let root = packaged(
            "indexed",
            &[
                ("blitsen/index.html", b"<p>hi"),
                ("blitsen/assets/app.js", b"globalThis.ran = 1"),
                ("blitsen/blitsen.assets.json", &listing),
            ],
        );
        let assets = ApkAssets::open_directory(&root, DEFAULT_ASSET_ROOT);
        assert!(assets.is_indexed());
        assert_eq!(assets.len(), 2);
        assert_eq!(
            assets.paths().collect::<Vec<_>>(),
            ["assets/app.js", "index.html"]
        );

        let report = assets.report();
        assert_eq!(report["bundled"], true);
        assert_eq!(report["source"], "apk-assets");
        assert_eq!(report["assetRoot"], DEFAULT_ASSET_ROOT);
        assert_eq!(report["indexed"], true);
        assert_eq!(report["indexVersion"], 1);
        assert_eq!(report["files"][0]["path"], "assets/app.js");
        assert_eq!(report["files"][0]["bytes"], 18);
        // The three fields a trailer would have carried, and this artifact has
        // no trailer: omitted rather than nulled.
        for absent in ["payloadBytes", "digest", "verified"] {
            assert!(report.get(absent).is_none(), "{absent} was reported");
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_package_without_an_index_still_reads_its_files() {
        let root = packaged("unindexed", &[("blitsen/index.html", b"<p>hi")]);
        let assets = ApkAssets::open_directory(&root, DEFAULT_ASSET_ROOT);
        assert_eq!(assets.read("index.html").unwrap(), b"<p>hi");
        assert!(!assets.is_indexed());
        assert_eq!(assets.len(), 0);
        assert_eq!(assets.paths().count(), 0);
        let report = assets.report();
        assert_eq!(report["bundled"], true);
        assert_eq!(report["indexed"], false);
        assert!(report.get("files").is_none());
        std::fs::remove_dir_all(&root).ok();
    }

    /// A newer index is ignored rather than half-read: an unknown format that
    /// parses as JSON is exactly the case where guessing produces a listing that
    /// is wrong instead of one that is absent.
    #[test]
    fn a_newer_index_is_ignored_rather_than_misread() {
        let listing = serde_json::to_vec(&serde_json::json!({
            "version": INDEX_VERSION + 1,
            "files": [{ "path": "index.html", "bytes": 5 }],
        }))
        .unwrap();
        let root = packaged(
            "future",
            &[
                ("blitsen/index.html", b"<p>hi"),
                ("blitsen/blitsen.assets.json", &listing),
            ],
        );
        let assets = ApkAssets::open_directory(&root, DEFAULT_ASSET_ROOT);
        assert!(!assets.is_indexed());
        // And the application still runs, because reading never needed it.
        assert_eq!(assets.read("index.html").unwrap(), b"<p>hi");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_application_at_the_root_of_assets_needs_no_prefix() {
        let root = packaged("bare", &[("index.html", b"<p>hi")]);
        let assets = ApkAssets::open_directory(&root, "");
        assert_eq!(assets.root(), "");
        assert_eq!(assets.read("index.html").unwrap(), b"<p>hi");
        std::fs::remove_dir_all(&root).ok();
    }
}
