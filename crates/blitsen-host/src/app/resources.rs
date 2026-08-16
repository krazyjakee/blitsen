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
        Some(root) => Arc::new(DirectoryResources { source, root }),
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
        // Only a path that landed outside the application is reinterpreted. One
        // already inside it is an ordinary relative reference and is read where
        // it points, so a directory at the filesystem root changes nothing.
        // `to_file_path` can fail for a root-shaped URL on Windows; that is also
        // not demonstrably inside and must reach the retry.
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
    fn directory_provider_preserves_its_distinct_file_url_fallbacks() {
        let root =
            std::env::temp_dir().join(format!("blitsen-resource-provider-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join("assets/app.css"), b"body { color: disk }").unwrap();

        let source = Arc::new(Fixture::default());
        let provider = net_provider(
            Arc::clone(&source) as Arc<dyn AppSource>,
            Some(root.clone()),
        );
        let collector = Collector::default();

        assert_eq!(
            collector.fetch(provider.as_ref(), "file:///assets/app.css"),
            (
                "file:///assets/app.css".to_owned(),
                b"body { color: black }".to_vec()
            )
        );
        let inside = Url::from_file_path(root.join("assets/app.css")).unwrap();
        assert_eq!(
            collector.fetch(provider.as_ref(), inside.as_str()),
            (inside.to_string(), b"body { color: disk }".to_vec())
        );
        assert_eq!(
            collector.fetch(provider.as_ref(), "data:text/plain,fallback"),
            ("data:text/plain,fallback".to_owned(), b"fallback".to_vec())
        );
        assert_eq!(*source.reads.lock().unwrap(), ["assets/app.css"]);

        std::fs::remove_dir_all(&root).ok();
    }
}
