//! Collecting a document's scripts and running them in order.

use std::path::{Path, PathBuf};

use blitsen_dom::{DomBackend, DomError, DomName};
use blitsen_js::{JsEngine, JsError};

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

/// DOM access needed to collect scripts without copying the tree.
pub trait ScriptDocument {
    /// Returns script elements in document order.
    fn document_scripts(&self) -> Result<Vec<DocumentScript>, DomError>;
}

impl<D: DomBackend> ScriptDocument for D {
    fn document_scripts(&self) -> Result<Vec<DocumentScript>, DomError> {
        self.query_selector_all(self.document(), "script")?
            .into_iter()
            .map(|node| {
                Ok(DocumentScript {
                    source: self.text_content(node)?,
                    src: self.attribute(node, &DomName::attribute("src"))?,
                    script_type: self.attribute(node, &DomName::attribute("type"))?,
                    async_attribute: self
                        .attribute(node, &DomName::attribute("async"))?
                        .is_some(),
                    defer_attribute: self
                        .attribute(node, &DomName::attribute("defer"))?
                        .is_some(),
                })
            })
            .collect()
    }
}

/// Evaluation operations used by the document script runner.
pub trait ScriptEngine {
    /// Engine-specific evaluation result.
    type Value;
    /// Evaluates a classic script.
    fn run_classic(&mut self, source: &str, identifier: &str) -> Result<Self::Value, JsError>;
    /// Starts module evaluation.
    fn run_module(&mut self, source: &str, identifier: &str) -> Result<Self::Value, JsError>;
}

impl<J: JsEngine> ScriptEngine for J {
    type Value = J::Value;

    fn run_classic(&mut self, source: &str, identifier: &str) -> Result<Self::Value, JsError> {
        self.evaluate_script(source, identifier)
    }

    fn run_module(&mut self, source: &str, identifier: &str) -> Result<Self::Value, JsError> {
        self.evaluate_module(source, identifier)
    }
}

/// Executes document scripts after parsing in strict document order.
///
/// v0 deliberately treats `async` and `defer` as document-order execution at
/// this post-parse checkpoint. This deterministic subset preserves dependency
/// order until networking and incremental parsing are introduced.
pub fn execute_document_scripts<D, J>(
    document: &D,
    engine: &mut J,
    entrypoint: &Path,
) -> Result<Vec<J::Value>, JsError>
where
    D: ScriptDocument,
    J: ScriptEngine,
{
    let scripts = document
        .document_scripts()
        .map_err(|error| JsError::new(error.to_string()))?;
    execute_collected_document_scripts(scripts, engine, entrypoint)
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

/// Executes a previously collected document-order script list.
///
/// Hosts with interior-mutable DOM storage use this form to release their tree
/// borrow before evaluation callbacks begin mutating that same tree.
pub fn execute_collected_document_scripts<J>(
    scripts: Vec<DocumentScript>,
    engine: &mut J,
    entrypoint: &Path,
) -> Result<Vec<J::Value>, JsError>
where
    J: ScriptEngine,
{
    execute_collected_document_scripts_from(scripts, engine, entrypoint, &LocalScripts)
}

/// Executes a collected script list, reading `src` scripts through `loader`.
pub fn execute_collected_document_scripts_from<J>(
    scripts: Vec<DocumentScript>,
    engine: &mut J,
    entrypoint: &Path,
    loader: &dyn ScriptLoader,
) -> Result<Vec<J::Value>, JsError>
where
    J: ScriptEngine,
{
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
            // A script the application does not ship is skipped for the same
            // reason a remote one is, and it is the same reason a browser has:
            // one source that does not arrive must not stop every other script
            // on the page. The preflight has already reported it as a
            // subresource the document renders without.
            match loader.load(root, &src) {
                Ok(loaded) => loaded,
                Err(error) => {
                    eprintln!("blitsen: skipping a script that will not load: {}", error.message());
                    continue;
                }
            }
        } else {
            (
                script.source,
                format!("{}#script-{}", entrypoint.display(), index + 1),
            )
        };
        let result = if module {
            engine.run_module(&source, &identifier)
        } else {
            engine.run_classic(&source, &identifier)
        }
        .map_err(|error| {
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

/// Reports a `src` Blitsen will not fetch: another origin, or a server root that
/// has no server behind it.
fn is_remote_script(src: &str) -> bool {
    src.starts_with("//") || src.contains("://")
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
    Ok(path)
}
