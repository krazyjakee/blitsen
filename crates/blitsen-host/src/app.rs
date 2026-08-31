//! Where a running application's files come from, and how the rest of the host
//! stops caring which.
//!
//! Four shapes reach the same window session: a directory of built output being
//! run, the section appended to an exported executable (issue #88), a
//! development server answering over HTTP (issue #67), and the `assets/` of an
//! APK read in place (issue #144). The difference is confined to this module —
//! everything downstream sees an entrypoint, a base URL, a subresource provider
//! and a script loader.

mod resources;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use blitsen_blitz::BlitzDom;
use blitsen_core::bundle::AppBundle;
use blitsen_core::{ScriptLoader, WindowState, document_scripts, simplified};
use blitsen_dom::DomBackend;
use blitsen_js::{JsEngine, JsError};
use blitz::dom::DocumentConfig;
use blitz::traits::net::NetProvider;
use blitz::traits::shell::{ColorScheme, Viewport};
use url::Url;

use crate::apk::ApkAssets;
use crate::dev_server::DevServer;
use crate::dom_bridge::DocumentMode;
use crate::modules::{APP_ORIGIN, AppSource, DirectorySource, url_of};

/// What a served URL with no filename asks for.
const DEFAULT_DOCUMENT: &str = "index.html";

/// The application's files, and which of the four shapes they came in.
#[derive(Clone)]
pub enum AppFiles {
    /// A directory of built output, run in place.
    Directory {
        /// Canonical application root.
        root: PathBuf,
        /// Canonical entrypoint inside it.
        entrypoint: PathBuf,
    },
    /// The section appended to this executable.
    Bundle {
        /// The opened bundle, shared with the subresource provider.
        bundle: Arc<AppBundle>,
        /// Application-relative entrypoint, conventionally `index.html`.
        entrypoint: String,
    },
    /// A development server answering over HTTP (issue #67).
    Server {
        /// The server, shared with the subresource provider.
        server: Arc<DevServer>,
        /// The path the document is served at, relative to the server's root.
        entrypoint: String,
    },
    /// The `assets/` of an APK, read in place (issue #144).
    ///
    /// The one shape with no exec'd binary behind it: on Android the code is a
    /// `.so` and `current_exe()` means nothing, so there is nothing to append a
    /// bundle to. See [`crate::apk`] for why the files are read where they lie
    /// rather than extracted on first run.
    Assets {
        /// The opened assets, shared with the subresource provider.
        assets: Arc<ApkAssets>,
        /// Application-relative entrypoint, conventionally `index.html`.
        entrypoint: String,
    },
}

impl AppFiles {
    /// Opens a directory of built output at `entrypoint`.
    pub fn directory(entrypoint: impl AsRef<Path>) -> Result<Self, JsError> {
        // Simplified, because this path becomes a script identifier and an
        // inline module's resolution base: Windows' extended-length spelling is
        // one a module resolver refuses (see `blitsen_core::simplified`).
        let entrypoint = simplified(entrypoint.as_ref().canonicalize().map_err(|error| {
            JsError::new(format!(
                "could not resolve {}: {error}",
                entrypoint.as_ref().display()
            ))
        })?);
        let root = entrypoint
            .parent()
            .ok_or_else(|| JsError::new("the entrypoint has no directory"))?
            .to_path_buf();
        Ok(Self::Directory { root, entrypoint })
    }

    /// Runs whatever a development server is serving at `url` (issue #67).
    ///
    /// The path in the URL is the document; a URL that names a directory — the
    /// ordinary `http://localhost:5173` — asks for `index.html` under it, which
    /// is what every dev server in the audience serves there.
    pub fn server(url: &str) -> Result<Self, JsError> {
        let parsed = Url::parse(url).map_err(|error| {
            JsError::new(format!("{url} is not a URL Blitsen can open: {error}"))
        })?;
        let path = parsed.path().trim_start_matches('/');
        let entrypoint = if path.is_empty() || path.ends_with('/') {
            format!("{path}{DEFAULT_DOCUMENT}")
        } else {
            path.to_owned()
        };
        let mut origin = parsed.clone();
        origin.set_path("/");
        origin.set_query(None);
        origin.set_fragment(None);
        let server = DevServer::connect(origin.as_str(), &entrypoint)?;
        Ok(Self::Server { server, entrypoint })
    }

    /// Runs the application appended to this executable.
    pub fn bundle(bundle: AppBundle, entrypoint: &str) -> Result<Self, JsError> {
        if !bundle.contains(entrypoint) {
            return Err(JsError::new(format!(
                "the application bundle has no {entrypoint}: it carries {} file(s), \
                 and an application needs an HTML entrypoint",
                bundle.len()
            )));
        }
        Ok(Self::Bundle {
            bundle: Arc::new(bundle),
            entrypoint: entrypoint.to_owned(),
        })
    }

    /// Runs the application packaged into an APK's `assets/` (issue #144).
    ///
    /// The entrypoint is checked by opening it, not by consulting an index: an
    /// APK carries a listing only if the packaging step wrote one, and a
    /// document that is there must open whether or not it was listed.
    pub fn assets(assets: ApkAssets, entrypoint: &str) -> Result<Self, JsError> {
        if !assets.contains(entrypoint) {
            return Err(JsError::new(format!(
                "this package has no {entrypoint} under assets/{}: an application needs an \
                 HTML entrypoint",
                assets.root()
            )));
        }
        Ok(Self::Assets {
            assets: Arc::new(assets),
            entrypoint: entrypoint.to_owned(),
        })
    }

    /// The entrypoint's source text.
    pub fn entrypoint_source(&self) -> Result<String, JsError> {
        match self {
            Self::Directory { entrypoint, .. } => {
                std::fs::read_to_string(entrypoint).map_err(|error| {
                    JsError::new(format!("could not read {}: {error}", entrypoint.display()))
                })
            }
            Self::Bundle { bundle, entrypoint } => bundle
                .read_to_string(entrypoint)
                .map_err(|error| JsError::new(error.to_string())),
            Self::Assets { assets, entrypoint } => {
                let bytes = assets.read(entrypoint).ok_or_else(|| {
                    JsError::new(format!(
                        "this package has no {entrypoint} under assets/{}",
                        assets.root()
                    ))
                })?;
                String::from_utf8(bytes).map_err(|_| {
                    JsError::new(format!(
                        "{entrypoint} is not UTF-8, so it is not a document"
                    ))
                })
            }
            Self::Server { server, entrypoint } => {
                let bytes = server.read(entrypoint).ok_or_else(|| {
                    JsError::new(format!(
                        "{} did not serve the document{}",
                        server.url_for(entrypoint),
                        server
                            .last_error()
                            .map_or_else(String::new, |error| format!(": {error}"))
                    ))
                })?;
                String::from_utf8(bytes).map_err(|_| {
                    JsError::new(format!(
                        "{} is not UTF-8, so it is not a document",
                        server.url_for(entrypoint)
                    ))
                })
            }
        }
    }

    /// What the entrypoint is called, for stack traces and script identifiers.
    pub fn entrypoint_name(&self) -> String {
        match self {
            Self::Directory { entrypoint, .. } => entrypoint.to_string_lossy().into_owned(),
            Self::Bundle { entrypoint, .. }
            | Self::Server { entrypoint, .. }
            | Self::Assets { entrypoint, .. } => url_of(entrypoint),
        }
    }

    /// The document's base URL, which relative subresources resolve against.
    pub fn base_url(&self) -> String {
        match self {
            // Percent-encoding is limited to the space, which is the character
            // a real application directory actually contains.
            Self::Directory { root, .. } => {
                format!("file://{}/", root.to_string_lossy().replace(' ', "%20"))
            }
            // Served, but still addressed as an application: the document, its
            // modules and its assets are on the application origin here exactly
            // as they are inside an export, and only the bytes come from
            // somewhere else (issue #67). An APK's assets are the same case —
            // an asset has no path a `file:` URL could name (issue #144).
            Self::Bundle { .. } | Self::Server { .. } | Self::Assets { .. } => {
                APP_ORIGIN.to_owned()
            }
        }
    }

    /// Stable development identity used when no export recorded an installed
    /// application identity. Canonical directory paths keep sibling projects
    /// apart; served applications use the normalized origin and entrypoint.
    pub fn storage_identity(&self) -> String {
        match self {
            Self::Directory { root, .. } => format!("directory:{}", root.to_string_lossy()),
            Self::Server { server, entrypoint } => {
                format!("server:{}{entrypoint}", server.origin())
            }
            Self::Bundle { entrypoint, .. } => format!(
                "bundle:{}:{entrypoint}",
                std::env::current_exe()
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| "unknown-executable".to_owned())
            ),
            // Android's files directory is already isolated by package. The
            // fallback is only for an old package with no recorded identity.
            Self::Assets { assets, entrypoint } => {
                format!("assets:{}:{entrypoint}", assets.root())
            }
        }
    }

    /// How many of the application's own files this carries.
    ///
    /// The number the standalone check reports, so it counts what the export
    /// collected and not what the runtime added: `blitsen.runtime.json` is the
    /// CLI's own record of the window settings, not one of the app's assets.
    pub fn asset_count(&self) -> usize {
        match self {
            Self::Directory { root, .. } => count_files(root),
            Self::Bundle { bundle, .. } => bundle
                .paths()
                .filter(|path| *path != RUNTIME_CONFIG && *path != NOTICES)
                .count(),
            // The index, when the package carries one. Zero without it — and
            // that is a package built with no listing rather than an
            // application with no files, because `AAssetManager` cannot walk a
            // directory tree and so cannot be asked (issue #144).
            Self::Assets { assets, .. } => assets
                .paths()
                .filter(|path| {
                    *path != RUNTIME_CONFIG && *path != NOTICES && *path != crate::apk::ASSET_INDEX
                })
                .count(),
            // Unknowable, and not worth guessing: a server has no list of what
            // it would serve if asked.
            Self::Server { .. } => 0,
        }
    }

    /// The files, as JavaScript asks for them by URL.
    pub fn reader(&self) -> AppReader {
        AppReader {
            source: self.source(),
            root: match self {
                // Canonicalised once, here, rather than on every URL the
                // reader is asked about: the root does not move while the
                // document it anchors is loaded, and each *target* is still
                // resolved per read. (The stored root is the simplified
                // spelling; the comparison in `path_of_url` needs the
                // canonical one, which on Windows is the `\\?\` form.)
                Self::Directory { root, .. } => root.canonicalize().ok(),
                Self::Bundle { .. } | Self::Server { .. } | Self::Assets { .. } => None,
            },
        }
    }

    /// The files, as the module resolver sees them.
    pub fn source(&self) -> Arc<dyn AppSource> {
        match self {
            Self::Directory { root, .. } => Arc::new(DirectorySource::new(root.clone())),
            Self::Bundle { bundle, .. } => Arc::clone(bundle) as Arc<dyn AppSource>,
            Self::Server { server, .. } => Arc::clone(server) as Arc<dyn AppSource>,
            Self::Assets { assets, .. } => Arc::clone(assets) as Arc<dyn AppSource>,
        }
    }

    /// How the document's `<script src>` elements are read.
    pub fn script_loader(&self) -> Box<dyn ScriptLoader> {
        resources::script_loader(
            self.source(),
            self.entrypoint_path(),
            matches!(self, Self::Server { .. }),
        )
    }

    /// The entrypoint's path inside the application, which is what its scripts
    /// and their imports are addressed relative to.
    fn entrypoint_path(&self) -> String {
        match self {
            Self::Directory { root, entrypoint } => entrypoint
                .strip_prefix(root)
                .map(|relative| relative.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| "index.html".to_owned()),
            Self::Bundle { entrypoint, .. }
            | Self::Server { entrypoint, .. }
            | Self::Assets { entrypoint, .. } => entrypoint.clone(),
        }
    }

    /// The subresource provider for images, stylesheets and fonts, or `None`
    /// when the ordinary Blitz provider should be used.
    ///
    /// A non-directory source needs its own: the ordinary provider cannot read
    /// bytes held by a bundle, development server or installed package.
    pub fn net_provider(&self) -> Option<Arc<dyn NetProvider>> {
        let directory_root = match self {
            Self::Directory { root, .. } => Some(root.clone()),
            Self::Bundle { .. } | Self::Server { .. } | Self::Assets { .. } => None,
        };
        Some(resources::net_provider(self.source(), directory_root))
    }
}

/// The third-party notices an application carries, decompressed (issue #121).
///
/// Taken over [`AppSource`] rather than over a bundle, because the notices were
/// never a property of the trailer: they are a file at a known path, and every
/// shape an application arrives in can be asked for one. That is what makes the
/// obligation survive onto Android, where an APK is launched with no argv and
/// `--licenses` cannot be typed at it — the file still travels inside the signed
/// archive, and the entry point can print or display it (see [`crate::apk`]).
///
/// Two names are looked for, and the second one is not a convenience (#148).
/// `aapt` **rewrites an asset whose name ends in `.gz`**: it strips the suffix
/// and stores the decompressed contents under the shortened name, so
/// `blitsen.notices.txt.gz` staged into an APK arrives as
/// [`NOTICES_UNCOMPRESSED`] holding plain text. That was measured by building an
/// APK and reading the archive back, not inferred from documentation. Looking
/// for one name only would have meant every Android artifact reporting itself
/// uncleared for redistribution while carrying the notices it owes — which is
/// the failure `docs/LICENSING.md` exists to gate against, arriving silently.
///
/// So the packaging step writes the uncompressed name on that path and this
/// reads either. Compression was never load-bearing here: it saves 88 KB inside
/// a container that deflates its own entries anyway.
///
/// `None` when the artifact carries none, or carries something that is neither
/// the gzipped text nor the plain text it should be. All of those mean the same
/// thing to a caller: this is not an artifact cleared for redistribution.
pub fn notices(source: &dyn AppSource) -> Option<String> {
    use std::io::Read as _;

    if let Some(compressed) = source.read(NOTICES) {
        let mut text = String::new();
        return flate2::read::GzDecoder::new(compressed.as_slice())
            .read_to_string(&mut text)
            .ok()
            .map(|_| text);
    }
    String::from_utf8(source.read(NOTICES_UNCOMPRESSED)?).ok()
}

/// Reads the files an application shipped, addressed the way JavaScript sees
/// them (issue #125).
///
/// The renderer already reads images and fonts out of the application, and the
/// module resolver already reads its scripts. What had no reader was the case
/// where the application asks by URL — `fetch`, and a media element's source —
/// and the consequence was that an application could not read a file it shipped
/// at all: `fetch` is http(s) only and says so, the shipped runtime implements
/// no `node:fs`, and `blitsen/app` answers with directories rather than
/// contents. `decodeAudioData` had no reachable source, and neither did a
/// bundled `.json` or `.wasm`.
///
/// Confined to the application on purpose. `fetch` is a web API, and an
/// application reading its own files is a different thing from one reading the
/// disk — that is `blitsen/*` territory, and it is not this.
#[derive(Clone)]
pub struct AppReader {
    source: Arc<dyn AppSource>,
    /// A directory being run is the one shape whose files are also addressed by
    /// `file:` URLs, because that is what its document's base URL is. Held
    /// canonical, resolved once when the reader is made — see
    /// [`AppFiles::reader`].
    root: Option<PathBuf>,
}

/// Why a URL named no readable application file.
pub enum NotRead {
    /// The URL does not address this application at all.
    Outside,
    /// It does, and the application shipped no such file.
    Missing(String),
}

impl AppReader {
    /// The application's files, for a reader that also has to resolve modules —
    /// which is what a worker's own context does with them.
    pub fn source(&self) -> Arc<dyn AppSource> {
        Arc::clone(&self.source)
    }

    /// Reads what `url` names inside the application.
    pub fn read_url(&self, url: &Url) -> Result<Vec<u8>, NotRead> {
        let path = self.path_of_url(url).ok_or(NotRead::Outside)?;
        self.source.read(&path).ok_or(NotRead::Missing(path))
    }

    /// The application-relative path `url` addresses, if any.
    fn path_of_url(&self, url: &Url) -> Option<String> {
        if url.scheme() == "blitsen" && url.host_str() == Some("app") {
            // Left percent-encoded exactly as `url_of` leaves it, so a path the
            // module resolver would read and one `fetch` reads are the same
            // string rather than two spellings that agree on most inputs.
            return Some(url.path().trim_start_matches('/').to_owned());
        }
        if url.scheme() != "file" {
            return None;
        }
        let root = self.root.as_ref()?;
        let target = url.to_file_path().ok()?;
        // Canonicalised when it can be, so a symlink out of the directory is
        // still out of it. A path that does not exist cannot be, and must not be
        // rejected for it: a file the application does not ship is a 404, and
        // reporting it as "not this application's" would send the reader looking
        // for the wrong mistake. `Url` has already resolved any `..` segments,
        // and `AppSource` re-canonicalises and re-checks on the way in.
        let target = target.canonicalize().unwrap_or(target);
        let relative = target.strip_prefix(root).ok()?;
        Some(relative.to_string_lossy().replace('\\', "/"))
    }
}

/// A parsed document with its scripts already run.
pub struct LoadedDocument {
    /// The authoritative Blitz tree.
    pub document: Rc<RefCell<BlitzDom>>,
    /// The `window` object's observable state.
    pub window_state: Rc<RefCell<WindowState>>,
}

/// A window document additionally carrying its private native dispatch hooks.
pub(crate) struct LoadedWindowDocument<V> {
    pub(crate) document: Rc<RefCell<BlitzDom>>,
    pub(crate) window_state: Rc<RefCell<WindowState>>,
    pub(crate) host_hooks: crate::dom_bridge::HostHooks<V>,
}

/// Viewport and JavaScript environment for loading one document.
pub struct LoadOptions {
    width: u32,
    height: u32,
    viewport: Option<Viewport>,
    mode: DocumentMode,
    storage: Option<crate::storage::LocalStorage>,
}

impl LoadOptions {
    /// Creates options for an initial viewport in the selected environment.
    pub fn new(width: u32, height: u32, mode: DocumentMode) -> Self {
        Self {
            width,
            height,
            viewport: None,
            mode,
            storage: None,
        }
    }

    /// Supplies this application's durable Web Storage area.
    pub fn with_storage(mut self, storage: crate::storage::LocalStorage) -> Self {
        self.storage = Some(storage);
        self
    }

    /// Preserves a live native viewport while reloading its document.
    pub fn with_viewport(mut self, viewport: Viewport) -> Self {
        self.viewport = Some(viewport);
        self
    }
}

thread_local! {
    static APPLICATION_ROOT: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Where the running application's files are on disk, when they are on disk.
///
/// An application is addressed by URL — `blitsen://app/assets/app.js` — whether
/// it is a directory being run or a section inside the executable, and that is
/// the property issue #90 turns on. A host whose module loader is the
/// filesystem's still needs the path behind that URL to resolve one module's
/// import of the next, and this is where it asks. `None` is a bundle, which has
/// no path at all.
pub fn application_root() -> Option<PathBuf> {
    APPLICATION_ROOT.with(|root| root.borrow().clone())
}

/// Parses the entrypoint, validates its assets and runs its scripts.
///
/// The one place a document is built, whichever host is doing it and whether or
/// not a window is involved, so opening a window, reloading one, and the
/// headless standalone check cannot drift apart in what they install or in what
/// order.
pub fn load_document<E: JsEngine + Clone + 'static>(
    engine: &mut E,
    files: &AppFiles,
    net_provider: Arc<dyn NetProvider>,
    options: LoadOptions,
) -> Result<LoadedDocument, JsError> {
    let loaded = load_window_document(engine, files, net_provider, options)?;
    Ok(LoadedDocument {
        document: loaded.document,
        window_state: loaded.window_state,
    })
}

/// Loads a document for a real window, retaining private native input hooks.
pub(crate) fn load_window_document<E: JsEngine + Clone + 'static>(
    engine: &mut E,
    files: &AppFiles,
    net_provider: Arc<dyn NetProvider>,
    options: LoadOptions,
) -> Result<LoadedWindowDocument<E::StrongRef>, JsError> {
    let LoadOptions {
        width,
        height,
        viewport,
        mode,
        storage,
    } = options;
    APPLICATION_ROOT.with(|current| {
        *current.borrow_mut() = match files {
            AppFiles::Directory { root, .. } => Some(root.clone()),
            AppFiles::Bundle { .. } | AppFiles::Server { .. } | AppFiles::Assets { .. } => None,
        };
    });
    let source = files.entrypoint_source()?;
    let viewport =
        viewport.unwrap_or_else(|| Viewport::new(width, height, 1.0, ColorScheme::Light));
    let device_pixel_ratio = f64::from(viewport.hidpi_scale);
    let dom_runtime = crate::DomRuntime::new(BlitzDom::from_html(
        &source,
        DocumentConfig {
            base_url: Some(files.base_url()),
            net_provider: Some(net_provider),
            viewport: Some(viewport),
            ..Default::default()
        },
    ));
    let document = dom_runtime.document();
    if let AppFiles::Directory { root, entrypoint } = files {
        // Only a directory can carry a reference outside itself; a bundle's
        // paths were checked when its index was read. What it cannot serve is
        // reported rather than refused — the renderer degrades it, and so does
        // an export, so refusing here would mean a document that runs once
        // exported and will not open from the directory it was exported from.
        for note in crate::validate_local_assets(&document.borrow(), root, entrypoint)? {
            eprintln!("blitsen: {note}");
        }
    }
    let scripts = document_scripts(&*document.borrow()).map_err(crate::dom_error)?;
    let entrypoint = files.entrypoint_name();
    let loader = files.script_loader();
    let installed = crate::harness::execute_window_scripts_from(
        engine,
        dom_runtime,
        scripts,
        crate::harness::WindowScriptOptions {
            entrypoint: &entrypoint,
            width,
            height,
            device_pixel_ratio,
            mode,
            loader: loader.as_ref(),
            reader: Some(files.reader()),
            storage,
        },
    )?;
    document
        .borrow_mut()
        .flush_layout()
        .map_err(crate::dom_error)?;
    Ok(LoadedWindowDocument {
        document,
        window_state: installed.window_state,
        host_hooks: installed.host_hooks,
    })
}

/// The window settings the CLI writes beside an exported application.
pub const RUNTIME_CONFIG: &str = "blitsen.runtime.json";

/// The third-party notices the export carries, gzipped (issue #121).
///
/// Inside the bundle rather than beside the executable, because an export is one
/// file and a notice a user can delete is not one that travels with the binary.
/// Compressed because it is 876 KB of licence text and 88 KB of bytes.
pub const NOTICES: &str = "blitsen.notices.txt.gz";

/// The same notices, as they arrive inside an APK (issue #148).
///
/// Not an alternative format: it is the *only* name that survives `aapt`, which
/// strips `.gz` from an asset and inflates it on the way in. The Android
/// packaging step therefore stages the text uncompressed under this name, and
/// [`notices`] reads either. See the argument there.
pub const NOTICES_UNCOMPRESSED: &str = "blitsen.notices.txt";

fn count_files(root: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => count_files(&entry.path()),
            Ok(kind) if kind.is_file() => 1,
            _ => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests;
