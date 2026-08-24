//! Collecting a document's scripts and running them in order.

use std::path::{Path, PathBuf};

use blitsen_dom::{DomBackend, DomError, DomName};
use blitsen_js::{JsEngine, JsError};

const INLINE_SCRIPT_FRAGMENT_PREFIX: &str = "#script-";

/// Names an inline document script without pretending it is a separate file.
///
/// A URL can carry only one fragment, so the script identity replaces a URL's
/// existing fragment while retaining its query. A filesystem path is kept
/// byte-for-byte: `#` and `?` are valid filename characters on Unix.
pub fn inline_script_identifier(document: &str, index: usize) -> String {
    let document = if document.contains("://") {
        document.split_once('#').map_or(document, |(head, _)| head)
    } else {
        document
    };
    format!("{document}{INLINE_SCRIPT_FRAGMENT_PREFIX}{index}")
}

/// Splits an identifier minted by [`inline_script_identifier`].
///
/// Recognition is anchored to the complete trailing fragment: a directory or
/// filename merely containing `#script-` is not an inline script. The returned
/// fragment includes its leading `#`, ready to append to a translated document
/// URL without rebuilding the protocol.
pub fn parse_inline_script_identifier(identifier: &str) -> Option<(&str, &str)> {
    let (document, index) = identifier.rsplit_once(INLINE_SCRIPT_FRAGMENT_PREFIX)?;
    if document.is_empty() || index.is_empty() || !index.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some((document, &identifier[document.len()..]))
}

/// Minimal script-element view provided by the authoritative DOM backend.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentScript {
    /// Inline source text.
    pub source: String,
    /// Optional local `src` attribute.
    pub src: Option<String>,
    /// Raw `type` attribute.
    pub script_type: Option<String>,
    /// Whether `async` was present.
    pub async_attribute: bool,
    /// Whether `defer` was present.
    pub defer_attribute: bool,
}

/// Collects script elements from a DOM backend in document order.
pub fn document_scripts<D: DomBackend>(document: &D) -> Result<Vec<DocumentScript>, DomError> {
    document
        .query_selector_all(document.document(), "script")?
        .into_iter()
        .map(|node| {
            Ok(DocumentScript {
                source: document.text_content(node)?,
                src: document.attribute(node, &DomName::attribute("src"))?,
                script_type: document.attribute(node, &DomName::attribute("type"))?,
                async_attribute: document
                    .attribute(node, &DomName::attribute("async"))?
                    .is_some(),
                defer_attribute: document
                    .attribute(node, &DomName::attribute("defer"))?
                    .is_some(),
            })
        })
        .collect()
}

/// Where a document's external scripts are read from.
///
/// The Phase 1 host and a directory being run take them off disk. An exported
/// Phase 2 application takes them out of the section appended to its own
/// executable, and never has a path to read.
pub trait ScriptLoader {
    /// Returns a `src` script's source and the identifier it evaluates under.
    ///
    /// The identifier is what appears in stack traces and, for a module, what
    /// its own imports resolve against — so it is a URL or path the loader can
    /// resolve again, not a display string.
    fn load(&self, root: &Path, src: &str) -> Result<(String, String), JsError>;

    /// Whether something between the file and this loader transforms it.
    ///
    /// A dev server does: `/src/main.jsx` comes back as JavaScript, which is the
    /// whole point of proxy mode (#67). A directory and a bundle do not, and an
    /// entrypoint that loads source in those shapes is a mistake with one fix —
    /// see [`source_only_extension`].
    fn serves_transformed(&self) -> bool {
        false
    }

    /// The document's own address, when it has one an import can resolve
    /// against.
    ///
    /// Only inline scripts need this: an external one is named by [`load`]. An
    /// inline module still has to resolve *its* imports against something, and
    /// the entrypoint's path on disk is not something the module resolver
    /// accepts — so a loader that serves an application answers with its
    /// application URL, and one reading loose files off disk answers `None` and
    /// keeps being named by its path.
    ///
    /// [`load`]: ScriptLoader::load
    fn document_url(&self) -> Option<String> {
        None
    }
}

/// Reads scripts from the filesystem, below the entrypoint's directory.
pub struct LocalScripts;

impl ScriptLoader for LocalScripts {
    fn load(&self, root: &Path, src: &str) -> Result<(String, String), JsError> {
        let path = resolve_local_script(root, src)?;
        let source = std::fs::read_to_string(&path).map_err(|error| {
            JsError::new(format!("could not read script {}: {error}", path.display()))
        })?;
        Ok((source, path.to_string_lossy().into_owned()))
    }
}

/// Executes a collected script list, reading `src` scripts through `loader`.
pub fn execute_collected_document_scripts_from<J>(
    scripts: Vec<DocumentScript>,
    engine: &mut J,
    entrypoint: &Path,
    loader: &dyn ScriptLoader,
) -> Result<Vec<J::Value>, JsError>
where
    J: JsEngine,
{
    execute_collected_document_scripts_with(scripts, entrypoint, loader, |module, source, name| {
        if module {
            engine.evaluate_module(source, name)
        } else {
            engine.evaluate_script(source, name)
        }
    })
}

/// Runs the script ordering and loading policy through a supplied evaluator.
///
/// The evaluator's first argument says whether the source is a module. Keeping
/// that one operation injectable tests the policy without mocking a complete
/// JavaScript runtime.
pub(crate) fn execute_collected_document_scripts_with<V>(
    scripts: Vec<DocumentScript>,
    entrypoint: &Path,
    loader: &dyn ScriptLoader,
    mut evaluate: impl FnMut(bool, &str, &str) -> Result<V, JsError>,
) -> Result<Vec<V>, JsError> {
    let root = entrypoint.parent().unwrap_or_else(|| Path::new("."));
    let mut results = Vec::with_capacity(scripts.len());
    for (index, script) in scripts.into_iter().enumerate() {
        let module = script
            .script_type
            .as_deref()
            .is_some_and(|kind| kind.eq_ignore_ascii_case("module"));
        if script.script_type.as_deref().is_some_and(|kind| {
            !kind.is_empty()
                && !kind.eq_ignore_ascii_case("module")
                && !kind.eq_ignore_ascii_case("text/javascript")
                && !kind.eq_ignore_ascii_case("application/javascript")
        }) {
            continue;
        }
        let (source, identifier) = if let Some(src) = script.src {
            // A source we will not fetch skips that script and leaves the rest of
            // the document running, as a browser does with a request that fails.
            // Aborting the run instead meant one analytics tag stopped every other
            // script on the page. The export refuses a remote script outright, so
            // this path is reached only by a directly opened directory, and it is
            // reported rather than passed over in silence.
            if is_remote_script(&src) {
                eprintln!("blitsen: skipping remote script, which is not fetched: {src}");
                continue;
            }
            // Pointed at source rather than at built output (issue #127). This
            // is refused rather than skipped, and refused before anything runs,
            // because it is a mistake about the whole application: nothing in
            // the document is going to work, and the answer is one command.
            // Phase 1 used to render such a tree — its host transpiles JSX and
            // resolves `react` out of `node_modules` — so an author could build
            // against something that would stop working under the shipped
            // runtime, which resolves and transpiles nothing by decision.
            if let Some(extension) =
                source_only_extension(&src).filter(|_| !loader.serves_transformed())
            {
                return Err(JsError::new(format!(
                    "{src} is {extension} source, not built output — a browser could not run it \
                     either. Blitsen loads the graph a bundler already produced: build the \
                     application (Vite: `vite build`) and point Blitsen at the output directory."
                )));
            }
            // A script the application does not ship is skipped for the same
            // reason a remote one is, and it is the same reason a browser has:
            // one source that does not arrive must not stop every other script
            // on the page. The preflight has already reported it as a
            // subresource the document renders without.
            match loader.load(root, &src) {
                Ok(loaded) => loaded,
                Err(error) => {
                    eprintln!(
                        "blitsen: skipping a script that will not load: {}",
                        error.message()
                    );
                    continue;
                }
            }
        } else {
            let document = loader
                .document_url()
                .unwrap_or_else(|| entrypoint.display().to_string());
            (
                script.source,
                inline_script_identifier(&document, index + 1),
            )
        };
        let result = evaluate(module, &source, &identifier).map_err(|error| {
            if error.stack().is_some() {
                error
            } else {
                JsError::new(format!("{identifier}: {}", error.message()))
            }
        })?;
        results.push(result);
    }
    Ok(results)
}

/// What a `<script src>` is written in, when it is not JavaScript.
///
/// Only extensions that are unambiguously a compiler's input: a `.js` file is
/// output whatever produced it, and a `.ts` file is not. Query strings and
/// fragments are dropped first, because a dev server's `?t=…` is part of the URL
/// and not of the name.
fn source_only_extension(src: &str) -> Option<&'static str> {
    let path = src.split(['?', '#']).next().unwrap_or_default();
    let extension = path.rsplit_once('.').map(|(_, tail)| tail)?;
    match extension.to_ascii_lowercase().as_str() {
        "ts" | "mts" | "cts" => Some("TypeScript"),
        "tsx" => Some("TypeScript JSX"),
        "jsx" => Some("JSX"),
        "vue" => Some("Vue single-file component"),
        "svelte" => Some("Svelte component"),
        _ => None,
    }
}

/// Reports a `src` Blitsen will not fetch: another origin, or a server root that
/// has no server behind it.
fn is_remote_script(src: &str) -> bool {
    src.starts_with("//") || src.contains("://")
}

/// Drops Windows' extended-length prefix from a canonicalised path.
///
/// `Path::canonicalize` answers `\\?\C:\…` on Windows. That is a valid path to
/// open a file with and *not* a valid module specifier: the identifier a script
/// evaluates under is handed back to a resolver — Bun's `createRequire` on the
/// Phase 1 host — which cannot open it, and reports the module as missing while
/// naming the file that is plainly there. Everything after the prefix is the
/// ordinary absolute path, and it is the one spelling both sides agree on.
///
/// A UNC path (`\\?\UNC\server\share`) is left alone: simplifying it means
/// rewriting rather than trimming, and nothing here runs off a network share.
pub fn simplified(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\")
            && !rest.starts_with("UNC\\")
        {
            return PathBuf::from(rest.to_owned());
        }
    }
    path
}

fn resolve_local_script(root: &Path, src: &str) -> Result<PathBuf, JsError> {
    if src.contains("://") {
        return Err(JsError::new(format!(
            "script src must be relative to the entrypoint: {src}"
        )));
    }
    let root = root
        .canonicalize()
        .map_err(|error| JsError::new(format!("could not resolve {}: {error}", root.display())))?;
    // A leading slash is the application root, not the filesystem's — the same
    // meaning the module resolver gives it inside a shipped executable, and the
    // one `blitsen build` rewrites it to. `root.join("/assets/x.js")` would
    // otherwise replace the root entirely, which is how a stock `vite build`
    // ended up looking for its bundle at the top of the disk.
    let path = root
        .join(src.trim_start_matches('/'))
        .canonicalize()
        .map_err(|error| JsError::new(format!("could not resolve script {src}: {error}")))?;
    if !path.starts_with(&root) {
        return Err(JsError::new(format!(
            "script src escapes the application directory: {src}"
        )));
    }
    // Simplified only after the containment check, which compares two paths in
    // the same canonical spelling.
    Ok(simplified(path))
}
