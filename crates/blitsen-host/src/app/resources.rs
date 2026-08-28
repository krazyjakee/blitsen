//! Script and renderer adapters over an application's [`AppSource`].
//!
//! The source owns where bytes come from and any source-specific caching or
//! diagnostics. These adapters only translate the consumer's URL or script
//! contract into one source read and preserve the callback behavior Blitz
//! requires for missing resources.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use blitsen_blitz::resources::LocalResources;
use blitsen_core::ScriptLoader;
use blitsen_js::JsError;
use blitz::traits::net::{Bytes, NetHandler, NetProvider, Request};

use crate::modules::{AppSource, path_of, url_of};

/// Builds the script loader shared by every application source.
pub(super) fn script_loader(
    source: Arc<dyn AppSource>,
    entrypoint: String,
    transformed: bool,
) -> Box<dyn ScriptLoader> {
    Box::new(AppScripts {
        source,
        entrypoint,
        transformed,
    })
}

/// Builds the renderer provider for one application source.
pub(super) fn net_provider(
    source: Arc<dyn AppSource>,
    directory_root: Option<PathBuf>,
) -> Arc<dyn NetProvider> {
    match directory_root {
        Some(root) => Arc::new(DirectoryResources {
            source,
            root: root.canonicalize().unwrap_or(root),
        }),
        None => Arc::new(SourceResources { source }),
    }
}

/// Reads `<script src>` out of the application, whichever shape it came in.
///
/// One loader for all sources, because the identifier it hands back is
/// load-bearing: a module resolves its own imports against it, and the resolver
/// accepts nothing but application URLs. A directory run used to be named by
/// its path on disk, so every `import` in a document module failed with "is not
/// an application URL" while the same application, exported, ran.
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
/// is. The application root is what it meant. The retry goes through
/// [`AppSource`], so it is confined to the application by the same check every
/// other read uses.
struct DirectoryResources {
    source: Arc<dyn AppSource>,
    root: PathBuf,
}

impl NetProvider for DirectoryResources {
    fn fetch(&self, doc_id: usize, request: Request, handler: Box<dyn NetHandler>) {
        if request.url.scheme() == "file" {
            // Canonical paths identify the same file across platform aliases
            // such as macOS' /var and /private/var. Every file read goes through
            // AppSource as well, so an inside-looking symlink cannot bypass its
            // canonical root check by falling through to LocalResources.
            let relative = request
                .url
                .to_file_path()
                .ok()
                .and_then(|path| {
                    let path = path.canonicalize().unwrap_or(path);
                    path.strip_prefix(&self.root).ok().map(Path::to_path_buf)
                })
                .map(|path| path.to_string_lossy().replace('\\', "/"))
                // A server-root URL landed at the filesystem root. Reinterpret
                // it relative to the application, preserving the Vite case.
                .unwrap_or_else(|| request.url.path().trim_start_matches('/').to_owned());
            let bytes = self.source.read(&relative).unwrap_or_default();
            handler.bytes(request.url.as_str().to_owned(), Bytes::from(bytes));
            return;
        }
        LocalResources.fetch(doc_id, request, handler);
    }
}

/// Serves subresources from any source addressed on the application origin.
///
/// Missing application resources are answered with an empty body rather than
/// left pending: Blitz holds stylesheets as critical resources until their
/// handler completes. The source remains responsible for its own reads,
/// caching and errors; the provider neither caches nor retries them.
struct SourceResources {
    source: Arc<dyn AppSource>,
}

impl NetProvider for SourceResources {
    fn fetch(&self, doc_id: usize, request: Request, handler: Box<dyn NetHandler>) {
        let url = request.url.as_str().to_owned();
        if let Some(path) = path_of(&url) {
            let bytes = self.source.read(path).unwrap_or_default();
            handler.bytes(url, Bytes::from(bytes));
            return;
        }
        if request.url.scheme() == "file" {
            handler.bytes(url, Bytes::new());
            return;
        }
        // `data:` subresources, and anything else the ordinary local provider
        // understands, retain their existing behavior.
        LocalResources.fetch(doc_id, request, handler);
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
    use std::sync::Mutex;

    use url::Url;

    use super::*;

    #[derive(Default)]
    struct Fixture {
        reads: Mutex<Vec<String>>,
    }

    impl AppSource for Fixture {
        fn read(&self, path: &str) -> Option<Vec<u8>> {
            self.reads.lock().unwrap().push(path.to_owned());
            match path {
                "assets/app.css?theme=dark" => Some(b"body { color: white }".to_vec()),
                "assets/app.css" => Some(b"body { color: black }".to_vec()),
                _ => None,
            }
        }
    }

    type Response = (String, Vec<u8>);

    #[derive(Clone, Default)]
    struct Collector(Arc<Mutex<Vec<Response>>>);

    impl NetHandler for Collector {
        fn bytes(self: Box<Self>, resolved_url: String, bytes: Bytes) {
            self.0.lock().unwrap().push((resolved_url, bytes.to_vec()));
        }
    }

    impl Collector {
        fn fetch(&self, provider: &dyn NetProvider, url: &str) -> Response {
            let before = self.0.lock().unwrap().len();
            provider.fetch(
                0,
                Request::get(Url::parse(url).unwrap()),
                Box::new(self.clone()),
            );
            let responses = self.0.lock().unwrap();
            assert_eq!(
                responses.len(),
                before + 1,
                "{url} must answer exactly once"
            );
            responses.last().unwrap().clone()
        }
    }

    #[test]
    fn every_erased_application_source_has_the_same_provider_contract() {
        let source = Arc::new(Fixture::default());
        let provider = net_provider(Arc::clone(&source) as Arc<dyn AppSource>, None);
        let collector = Collector::default();

        let url = "blitsen://app/assets/app.css?theme=dark";
        assert_eq!(
            collector.fetch(provider.as_ref(), url),
            (url.to_owned(), b"body { color: white }".to_vec())
        );
        assert_eq!(
            collector.fetch(provider.as_ref(), "blitsen://app/missing.css"),
            ("blitsen://app/missing.css".to_owned(), Vec::new())
        );
        assert_eq!(
            collector.fetch(provider.as_ref(), "data:text/plain,fallback"),
            ("data:text/plain,fallback".to_owned(), b"fallback".to_vec())
        );

        // The provider neither caches nor retries: source-specific cache and
        // error state remain solely the source's responsibility.
        assert_eq!(
            *source.reads.lock().unwrap(),
            ["assets/app.css?theme=dark", "missing.css"]
        );
    }

    #[test]
    fn directory_provider_confines_both_file_url_shapes_through_its_source() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("assets")).unwrap();
        std::fs::write(root.path().join("assets/app.css"), b"body { color: disk }").unwrap();

        let source = Arc::new(Fixture::default());
        let provider = net_provider(
            Arc::clone(&source) as Arc<dyn AppSource>,
            Some(root.path().to_path_buf()),
        );
        let collector = Collector::default();

        assert_eq!(
            collector.fetch(provider.as_ref(), "file:///assets/app.css"),
            (
                "file:///assets/app.css".to_owned(),
                b"body { color: black }".to_vec()
            )
        );
        let inside = Url::from_file_path(root.path().join("assets/app.css")).unwrap();
        assert_eq!(
            collector.fetch(provider.as_ref(), inside.as_str()),
            (inside.to_string(), b"body { color: black }".to_vec())
        );
        assert_eq!(
            collector.fetch(provider.as_ref(), "data:text/plain,fallback"),
            ("data:text/plain,fallback".to_owned(), b"fallback".to_vec())
        );
        assert_eq!(
            *source.reads.lock().unwrap(),
            ["assets/app.css", "assets/app.css"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn directory_provider_recognises_path_aliases_without_following_escapes() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("assets")).unwrap();
        std::fs::write(root.path().join("assets/app.css"), b"body { color: disk }").unwrap();
        let aliases = tempfile::tempdir().unwrap();
        let alias = aliases.path().join("application");
        symlink(root.path(), &alias).unwrap();

        let provider = net_provider(
            Arc::new(crate::modules::DirectorySource::new(root.path())),
            Some(root.path().canonicalize().unwrap()),
        );
        let url = Url::from_file_path(alias.join("assets/app.css")).unwrap();
        assert_eq!(
            Collector::default().fetch(provider.as_ref(), url.as_str()),
            (url.to_string(), b"body { color: disk }".to_vec())
        );

        let outside = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(outside.path(), b"outside").unwrap();
        let escape = root.path().join("escape");
        symlink(outside.path(), &escape).unwrap();
        let escape = Url::from_file_path(escape).unwrap();
        assert_eq!(
            Collector::default().fetch(provider.as_ref(), escape.as_str()),
            (escape.to_string(), Vec::new())
        );
    }

    #[test]
    fn both_provider_shapes_refuse_files_outside_the_application() {
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(outside.path(), b"not application data").unwrap();
        let outside = Url::from_file_path(outside.path()).unwrap();
        let collector = Collector::default();

        let exported = net_provider(Arc::new(Fixture::default()), None);
        assert_eq!(
            collector.fetch(exported.as_ref(), outside.as_str()),
            (outside.to_string(), Vec::new())
        );

        let root = tempfile::tempdir().unwrap();
        let directory = net_provider(
            Arc::new(Fixture::default()),
            Some(root.path().to_path_buf()),
        );
        assert_eq!(
            collector.fetch(directory.as_ref(), outside.as_str()),
            (outside.to_string(), Vec::new())
        );
    }
}
