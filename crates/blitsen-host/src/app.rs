//! Where a running application's files come from, and how the rest of the host
//! stops caring which.
//!
//! Three shapes reach the same window session: a directory of built output being
//! run, the section appended to an exported executable (issue #88), and a
//! development server answering over HTTP (issue #67). The difference is
//! confined to this module — everything downstream sees an entrypoint, a base
//! URL, a subresource provider and a script loader.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use blitsen_blitz::BlitzDom;
use blitsen_core::bundle::AppBundle;
use blitsen_core::{ScriptDocument, ScriptLoader, WindowState, simplified};
use blitsen_dom::DomBackend;
use blitsen_js::{JsEngine, JsError};
use blitz::dom::DocumentConfig;
use blitz::traits::net::{Bytes, NetHandler, NetProvider, Request};
use blitz::traits::shell::{ColorScheme, Viewport};
use url::Url;

use crate::dev_server::DevServer;
use crate::modules::{APP_ORIGIN, AppSource, DirectorySource, path_of, url_of};

/// What a served URL with no filename asks for.
const DEFAULT_DOCUMENT: &str = "index.html";

/// The application's files, and which of the three shapes they came in.
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
            Self::Bundle { entrypoint, .. } => url_of(entrypoint),
            Self::Server { entrypoint, .. } => url_of(entrypoint),
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
            // somewhere else (issue #67).
            Self::Bundle { .. } | Self::Server { .. } => APP_ORIGIN.to_owned(),
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
                Self::Directory { root, .. } => Some(root.clone()),
                Self::Bundle { .. } | Self::Server { .. } => None,
            },
        }
    }

    /// The files, as the module resolver sees them.
    pub fn source(&self) -> Arc<dyn AppSource> {
        match self {
            Self::Directory { root, .. } => Arc::new(DirectorySource::new(root.clone())),
            Self::Bundle { bundle, .. } => Arc::clone(bundle) as Arc<dyn AppSource>,
            Self::Server { server, .. } => Arc::clone(server) as Arc<dyn AppSource>,
        }
    }

    /// How the document's `<script src>` elements are read.
    pub fn script_loader(&self) -> Box<dyn ScriptLoader> {
        Box::new(AppScripts {
            source: self.source(),
            entrypoint: self.entrypoint_path(),
            transformed: matches!(self, Self::Server { .. }),
        })
    }

    /// The entrypoint's path inside the application, which is what its scripts
    /// and their imports are addressed relative to.
    fn entrypoint_path(&self) -> String {
        match self {
            Self::Directory { root, entrypoint } => entrypoint
                .strip_prefix(root)
                .map(|relative| relative.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| "index.html".to_owned()),
            Self::Bundle { entrypoint, .. } | Self::Server { entrypoint, .. } => entrypoint.clone(),
        }
    }

    /// The subresource provider for images, stylesheets and fonts, or `None`
    /// when the ordinary Blitz provider should be used.
    ///
    /// A bundle needs its own: no other provider can read a file that exists
    /// only as a byte range inside the running executable.
    pub fn net_provider(&self) -> Option<Arc<dyn NetProvider>> {
        match self {
            Self::Directory { root, .. } => Some(Arc::new(DirectoryResources {
                source: Arc::new(DirectorySource::new(root.clone())),
                root: root.clone(),
            }) as Arc<dyn NetProvider>),
            Self::Bundle { bundle, .. } => Some(Arc::new(BundleResources {
                bundle: Arc::clone(bundle),
            }) as Arc<dyn NetProvider>),
            Self::Server { server, .. } => Some(Arc::new(ServerResources {
                server: Arc::clone(server),
            }) as Arc<dyn NetProvider>),
        }
    }
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
    /// `file:` URLs, because that is what its document's base URL is.
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
        let root = self.root.as_ref()?.canonicalize().ok()?;
        let target = url.to_file_path().ok()?;
        // Canonicalised when it can be, so a symlink out of the directory is
        // still out of it. A path that does not exist cannot be, and must not be
        // rejected for it: a file the application does not ship is a 404, and
        // reporting it as "not this application's" would send the reader looking
        // for the wrong mistake. `Url` has already resolved any `..` segments,
        // and `AppSource` re-canonicalises and re-checks on the way in.
        let target = target.canonicalize().unwrap_or(target);
        let relative = target.strip_prefix(&root).ok()?;
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
#[allow(clippy::too_many_arguments)]
pub fn load_document<E: JsEngine + Clone + 'static>(
    engine: &mut E,
    files: &AppFiles,
    net_provider: Arc<dyn NetProvider>,
    width: u32,
    height: u32,
    viewport: Option<Viewport>,
    test_harness: bool,
) -> Result<LoadedDocument, JsError> {
    APPLICATION_ROOT.with(|current| {
        *current.borrow_mut() = match files {
            AppFiles::Directory { root, .. } => Some(root.clone()),
            AppFiles::Bundle { .. } | AppFiles::Server { .. } => None,
        };
    });
    let source = files.entrypoint_source()?;
    let dom_runtime = crate::DomRuntime::new(BlitzDom::from_html(
        &source,
        DocumentConfig {
            base_url: Some(files.base_url()),
            net_provider: Some(net_provider),
            viewport: Some(
                viewport.unwrap_or_else(|| Viewport::new(width, height, 1.0, ColorScheme::Light)),
            ),
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
    let scripts = document
        .borrow()
        .document_scripts()
        .map_err(crate::dom_error)?;
    let window_state = crate::harness::execute_window_scripts_from(
        engine,
        dom_runtime,
        scripts,
        &files.entrypoint_name(),
        width,
        height,
        test_harness,
        files.script_loader().as_ref(),
        Some(files.reader()),
    )?;
    document
        .borrow_mut()
        .flush_layout()
        .map_err(crate::dom_error)?;
    Ok(LoadedDocument {
        document,
        window_state,
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

/// Reads `<script src>` out of the application, whichever shape it came in.
///
/// One loader for both, because the identifier it hands back is load-bearing: a
/// module resolves its own imports against it, and the resolver accepts nothing
/// but application URLs. A directory run used to be named by its path on disk,
/// so every `import` in a document module failed with "is not an application
/// URL" — while the same application, exported, ran. That is exactly the
/// difference between `blitsen run ./dist` and the binary it exports to that
/// `modules.rs` says must not exist.
///
/// A script is still confined to the application: `resolve` refuses anything
/// that would leave it, which is the same check reading loose files off disk
/// made with a canonicalized prefix.
struct AppScripts {
    source: Arc<dyn AppSource>,
    /// The entrypoint's application-relative path, which a `src` resolves
    /// against.
    entrypoint: String,
    /// Whether the source transforms what it serves — a dev server does.
    transformed: bool,
}

impl ScriptLoader for AppScripts {
    fn load(&self, _root: &Path, src: &str) -> Result<(String, String), JsError> {
        let url = crate::modules::resolve(&url_of(&self.entrypoint), &relative(src))?;
        let path = path_of(&url).expect("resolve returns application URLs");
        let bytes = self
            .source
            .read(path)
            .ok_or_else(|| JsError::new(format!("the application has no script at {path}")))?;
        let source = String::from_utf8(bytes)
            .map_err(|_| JsError::new(format!("the script at {path} is not UTF-8")))?;
        Ok((source, url))
    }

    fn document_url(&self) -> Option<String> {
        Some(url_of(&self.entrypoint))
    }

    fn serves_transformed(&self) -> bool {
        self.transformed
    }
}

/// Serves a directory's subresources, with a server-root URL meaning the
/// application root.
///
/// Blitz resolves `href="/assets/app.css"` against the document's `file:` base
/// and arrives at `file:///assets/app.css` — the top of the disk, where nothing
/// is. The application root is what the URL meant: it is what `blitsen build`
/// rewrites it to at ingest, and what the application origin already means
/// inside a shipped executable. Without this the default `vite build` output
/// exported fine and would not open, which is a difference between two commands
/// pointed at one directory.
///
/// The retry goes through [`AppSource`], so it is confined to the application by
/// the same check every other read of it uses.
struct DirectoryResources {
    source: Arc<dyn AppSource>,
    root: PathBuf,
}

impl NetProvider for DirectoryResources {
    fn fetch(&self, doc_id: usize, request: Request, handler: Box<dyn NetHandler>) {
        // Only a path that landed outside the application is reinterpreted. One
        // already inside it is an ordinary relative reference and is read where
        // it points, so a directory that happens to sit at the filesystem root
        // cannot change what a relative URL means.
        //
        // Stated as "not demonstrably inside", because a `file:` URL that names
        // no path at all is not inside either. `file:///assets/app.css` has no
        // drive letter, so `to_file_path` fails on Windows where it succeeds on
        // Unix — and reading that failure as "not outside" left the server-root
        // retry unreachable there, so a stock `vite build` export opened with no
        // stylesheet on Windows alone.
        let outside = !request
            .url
            .to_file_path()
            .is_ok_and(|path| path.starts_with(&self.root));
        if request.url.scheme() == "file" && outside {
            let relative = request.url.path().trim_start_matches('/');
            if let Some(bytes) = self.source.read(relative) {
                handler.bytes(request.url.as_str().to_owned(), Bytes::from(bytes));
                return;
            }
        }
        blitsen_blitz::resources::LocalResources.fetch(doc_id, request, handler);
    }
}

/// Serves a document's subresources out of the appended section.
struct BundleResources {
    bundle: Arc<AppBundle>,
}

impl NetProvider for BundleResources {
    fn fetch(&self, doc_id: usize, request: Request, handler: Box<dyn NetHandler>) {
        let url = request.url.as_str().to_owned();
        if let Some(path) = path_of(&url) {
            // Blitz holds a stylesheet as a pending critical resource until its
            // handler completes, so a missing file is answered with no bytes
            // rather than left hanging — the same contract `LocalResources`
            // keeps, and the reason a broken `<img>` reaches its errored state.
            let bytes = self.bundle.read(path).unwrap_or_default();
            handler.bytes(url, Bytes::from(bytes));
            return;
        }
        // `data:` subresources, and anything else the ordinary local provider
        // understands, still work inside a bundle.
        blitsen_blitz::resources::LocalResources.fetch(doc_id, request, handler);
    }
}

/// Serves a dev server's subresources, on the application origin (issue #67).
///
/// The same shape as [`BundleResources`], and for the same reason: a stylesheet
/// is a pending critical resource until its handler completes, so one the server
/// will not serve is answered with no bytes rather than left hanging.
struct ServerResources {
    server: Arc<DevServer>,
}

impl NetProvider for ServerResources {
    fn fetch(&self, doc_id: usize, request: Request, handler: Box<dyn NetHandler>) {
        let url = request.url.as_str().to_owned();
        if let Some(path) = path_of(&url) {
            let bytes = self.server.read(path).unwrap_or_default();
            handler.bytes(url, Bytes::from(bytes));
            return;
        }
        blitsen_blitz::resources::LocalResources.fetch(doc_id, request, handler);
    }
}

/// Turns a document-relative `src` into a specifier the resolver accepts.
fn relative(src: &str) -> String {
    if src.starts_with('/') || src.starts_with("./") || src.starts_with("../") {
        src.to_owned()
    } else {
        format!("./{src}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bundle_addresses_its_files_by_the_application_origin() {
        let root = std::env::temp_dir().join(format!("blitsen-app-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let runtime = root.join("runtime");
        std::fs::write(&runtime, vec![0_u8; 512]).unwrap();
        let linked = root.join("app");
        blitsen_core::bundle::write_bundle(
            &runtime,
            &linked,
            &[
                ("index.html".to_owned(), b"<p>hi".to_vec()),
                ("assets/app.js".to_owned(), b"globalThis.ran = 1".to_vec()),
            ],
        )
        .unwrap();

        let bundle = AppBundle::open(&linked).unwrap().unwrap();
        let files = AppFiles::bundle(bundle, "index.html").unwrap();
        assert_eq!(files.entrypoint_source().unwrap(), "<p>hi");
        assert_eq!(files.base_url(), APP_ORIGIN);
        assert_eq!(files.entrypoint_name(), "blitsen://app/index.html");
        assert_eq!(
            files
                .script_loader()
                .load(Path::new("."), "assets/app.js")
                .unwrap(),
            (
                "globalThis.ran = 1".to_owned(),
                "blitsen://app/assets/app.js".to_owned()
            )
        );
        assert!(files.net_provider().is_some());
        assert_eq!(
            files.source().read("assets/app.js").unwrap(),
            b"globalThis.ran = 1"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// A stock `vite build` writes `/assets/index-<hash>.js`, and Blitz resolves
    /// that against the document's `file:` base to the top of the disk. The
    /// application root is what it meant.
    #[test]
    fn a_directory_serves_a_server_root_subresource_from_the_application_root() {
        use blitz::traits::net::Request;
        use std::sync::Mutex;

        #[derive(Clone, Default)]
        struct Collector(Arc<Mutex<Vec<(String, usize)>>>);
        impl NetHandler for Collector {
            fn bytes(self: Box<Self>, resolved_url: String, bytes: Bytes) {
                self.0.lock().unwrap().push((resolved_url, bytes.len()));
            }
        }

        let root = std::env::temp_dir().join(format!("blitsen-dir-net-{}", std::process::id()));
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join("index.html"), b"<p>hi").unwrap();
        std::fs::write(root.join("assets/app.css"), b"body{margin:0}").unwrap();
        let files = AppFiles::directory(root.join("index.html")).unwrap();
        let provider = files
            .net_provider()
            .expect("a directory serves its own files");

        let collector = Collector::default();
        let served = |url: &str| {
            provider.fetch(
                0,
                Request::get(Url::parse(url).unwrap()),
                Box::new(collector.clone()),
            );
            collector.0.lock().unwrap().last().unwrap().1
        };
        // What Blitz asks for after resolving `href="/assets/app.css"`.
        assert_eq!(served("file:///assets/app.css"), 14);
        // An ordinary relative reference still reads where it points. Built
        // through `Url::from_file_path` rather than by formatting the path into
        // a string: on Windows that string is `C:\...`, whose drive letter parses
        // as a URL host and whose separators are not URL separators.
        let inside = Url::from_file_path(root.join("assets").join("app.css")).unwrap();
        assert_eq!(served(inside.as_str()), 14);
        // And a server-root path the application does not ship stays empty, so
        // the document paints without it rather than waiting on it.
        assert_eq!(served("file:///assets/nothing.css"), 0);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_bundle_without_an_entrypoint_says_so_rather_than_opening_a_blank_window() {
        let root = std::env::temp_dir().join(format!("blitsen-app-empty-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let runtime = root.join("runtime");
        std::fs::write(&runtime, vec![0_u8; 512]).unwrap();
        let linked = root.join("app");
        blitsen_core::bundle::write_bundle(
            &runtime,
            &linked,
            &[("readme.txt".to_owned(), b"x".to_vec())],
        )
        .unwrap();
        let bundle = AppBundle::open(&linked).unwrap().unwrap();
        let error = AppFiles::bundle(bundle, "index.html")
            .err()
            .expect("no entrypoint");
        assert!(error.message().contains("no index.html"));
        std::fs::remove_dir_all(&root).ok();
    }
}
