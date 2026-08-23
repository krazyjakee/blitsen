//! Module resolution for the shipped binary (issue #86).
//!
//! # The decision
//!
//! A **runtime resolver over the application's own files**, not a pre-bundled
//! single graph.
//!
//! The simpler option was to require one module per document and evaluate it
//! whole. It was rejected on evidence: the output Blitsen is being handed is
//! already split. Vite emits an entry chunk plus vendor chunks joined by static
//! `import`, and every router in the audience — React Router, TanStack Router,
//! Vue Router, SvelteKit — documents route-level `import()` as the way to code
//! split. Requiring a single graph would mean telling those users to turn code
//! splitting off, and a runtime that only runs deoptimised builds is not a
//! target for their toolchain.
//!
//! It also stays on the right side of structural constraint 6: Blitsen resolves
//! and loads a graph the user's bundler already produced. It does not parse,
//! transform, or link it. Nothing here rewrites a byte of application source.
//!
//! # The application origin
//!
//! Modules need absolute URLs — `import.meta.url` is one by definition, and a
//! specifier is resolved against one. Inside a shipped executable there is no
//! directory to name, so the application gets an origin of its own:
//!
//! ```text
//! blitsen://app/assets/index-a1b2c3.js
//! ```
//!
//! This is a narrower thing than the internal origin TECH.md §17.9 rejected.
//! That decision was about *subresources referenced from HTML and CSS*, which
//! are rewritten to document-relative paths at ingest and never need an origin.
//! Modules are the case where relative rewriting cannot do the job, because the
//! language hands the URL to the application. The same origin is used whether
//! the application is a directory being run or a bundle inside an executable,
//! so `blitsen run ./dist` and the exported binary resolve identically — which
//! is the property issue #90 is about.
//!
//! # Where the graph is linked
//!
//! Resolution and source are the host's. *Linking* — instantiating the records,
//! wiring live bindings, ordering evaluation, breaking cycles — is the engine's,
//! and no JavaScript engine exposes it to be reimplemented from outside. The
//! shipped engine exposes the seam instead: `JS_SetModuleLoaderFunc` takes a
//! normaliser and a loader, and the resolver below is what that pair calls back
//! into. That it is stock public API rather than a patch is part of why the
//! engine was swapped — JavaScriptCore's public C API has no module loader hook
//! at all, which is what `MODULES.md` and `JSC.md` record.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use base64::Engine as _;
use blitsen_core::bundle::AppBundle;
use blitsen_js::{JsEngine, JsError};

use crate::dom_bridge::argument;
use crate::source_maps::SourceMap;

/// The origin an application's own files are addressed by.
pub const APP_ORIGIN: &str = "blitsen://app/";

/// Where a running application's files come from.
///
/// Four implementations, and the resolver cannot tell them apart: a directory
/// being run during development, the section appended to the executable, a dev
/// server answering over HTTP (#67), and an APK's `assets/` read in place
/// (#144). A path in, bytes out, is the whole of it — which is what let the
/// fourth be added without anything downstream learning a new shape.
pub trait AppSource: Send + Sync {
    /// Reads one application-relative path, or `None` when it is not there.
    fn read(&self, path: &str) -> Option<Vec<u8>>;

    /// Whether the path exists, without reading it.
    fn contains(&self, path: &str) -> bool {
        self.read(path).is_some()
    }
}

/// An application laid out as a directory on disk.
pub struct DirectorySource {
    root: PathBuf,
}

impl DirectorySource {
    /// Serves files below `root`, and nothing outside it.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl AppSource for DirectorySource {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        // `resolve` has already refused `..`, but the root is canonicalised and
        // rechecked here too: this is the only place a path becomes a real one.
        let root = self.root.canonicalize().ok()?;
        let target = root.join(Path::new(file_of(path))).canonicalize().ok()?;
        target
            .starts_with(&root)
            .then(|| std::fs::read(target).ok())?
    }
}

impl AppSource for AppBundle {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        AppBundle::read(self, file_of(path)).ok()
    }

    fn contains(&self, path: &str) -> bool {
        AppBundle::contains(self, file_of(path))
    }
}

/// Turns a specifier and its referrer into an application URL.
///
/// Pure, and the whole of the resolution policy. Both hosts and every test go
/// through this function, so there is one answer to "what does this import
/// mean" rather than one per call site.
pub fn resolve(referrer: &str, specifier: &str) -> Result<String, JsError> {
    let specifier = specifier.trim();
    if specifier.is_empty() {
        return Err(JsError::new("an import specifier cannot be empty"));
    }
    if let Some(path) = specifier.strip_prefix(APP_ORIGIN) {
        return normalise("", path).map(|path| format!("{APP_ORIGIN}{path}"));
    }
    if specifier.starts_with("//") || specifier.contains("://") {
        return Err(JsError::new(format!(
            "Blitsen does not fetch modules over the network: {specifier}. \
             Bundle it into the application instead."
        )));
    }
    // Node and Bun builtins are the specifiers most likely to be reached for,
    // and the generic answer below would be wrong about why they fail: the
    // shipped runtime does not implement them at all, and the alternative has a
    // name. See COMPATIBILITY.md, "Node compatibility in the shipped runtime".
    if let Some(builtin) = specifier
        .strip_prefix("node:")
        .or_else(|| specifier.strip_prefix("bun:"))
    {
        return Err(JsError::new(format!(
            concat!(
                "the shipped Blitsen runtime implements no {specifier}. ",
                "Node and Bun builtins are the toolchain's, not the runtime's; ",
                "use the `blitsen/*` native modules for system access, or bundle ",
                "a browser-targeted replacement for {builtin}."
            ),
            specifier = specifier,
            builtin = builtin,
        )));
    }
    if !specifier.starts_with("./") && !specifier.starts_with("../") && !specifier.starts_with('/')
    {
        return Err(JsError::new(format!(
            "{specifier:?} is a bare module specifier, which only a bundler can resolve. \
             Blitsen loads the graph your bundler already produced, so build the \
             application before running it."
        )));
    }

    let base = if specifier.starts_with('/') {
        String::new()
    } else {
        let referrer = referrer
            .strip_prefix(APP_ORIGIN)
            .ok_or_else(|| JsError::new(format!("{referrer} is not an application URL")))?;
        referrer
            .rsplit_once('/')
            .map(|(directory, _)| directory.to_owned())
            .unwrap_or_default()
    };
    normalise(&base, specifier).map(|path| format!("{APP_ORIGIN}{path}"))
}

/// Returns the application-relative path an application URL addresses.
pub fn path_of(url: &str) -> Option<&str> {
    url.strip_prefix(APP_ORIGIN)
}

/// Returns the application URL for a path already known to be inside the app.
pub fn url_of(path: &str) -> String {
    format!("{APP_ORIGIN}{path}")
}

/// Joins and normalises, refusing anything that leaves the application.
fn normalise(base: &str, specifier: &str) -> Result<String, JsError> {
    // A fragment names a place inside a document rather than a file, and never
    // reaches a source. A query does reach one: `/src/main.jsx` and
    // `/src/main.jsx?t=1738` are two different responses from a dev server, and
    // proxy mode (#67) needs the second to be asked for as written. It is kept
    // on the resolved URL and dropped by whichever source is file-backed, which
    // is exactly what a file server would have done with it.
    let (specifier, query) = specifier
        .split_once('#')
        .map_or((specifier, ""), |(head, _)| (head, ""));
    let (path, query) = match specifier.split_once('?') {
        Some((head, tail)) => (head, format!("?{tail}{query}")),
        None => (specifier, query.to_owned()),
    };
    let path = path.trim_start_matches('/');
    let mut segments: Vec<&str> = if base.is_empty() {
        Vec::new()
    } else {
        base.split('/')
            .filter(|segment| !segment.is_empty())
            .collect()
    };
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if segments.pop().is_none() {
                    return Err(JsError::new(format!(
                        "{specifier} resolves outside the application"
                    )));
                }
            }
            segment => segments.push(segment),
        }
    }
    if segments.is_empty() {
        return Err(JsError::new(format!("{specifier} does not name a file")));
    }
    Ok(format!("{}{query}", segments.join("/")))
}

/// The file a resolved application path names, without what a server reads.
///
/// A file-backed source is asked for `assets/app.js?v=2` and opens
/// `assets/app.js`, because that is the file, and the query was for whoever was
/// serving it.
pub fn file_of(path: &str) -> &str {
    path.split('?').next().unwrap_or(path)
}

/// The module graph as the engine's loader sees it.
///
/// Holds the application's files and the source of every module already handed
/// out, so a second import of the same URL is the same record rather than a
/// second evaluation.
pub struct ModuleRegistry {
    source: Arc<dyn AppSource>,
    loaded: RefCell<HashMap<String, LoadedModule>>,
}

struct LoadedModule {
    source: Rc<String>,
    source_map: Option<SourceMap>,
}

impl ModuleRegistry {
    /// Serves modules out of `source`.
    pub fn new(source: Arc<dyn AppSource>) -> Self {
        Self {
            source,
            loaded: RefCell::new(HashMap::new()),
        }
    }

    /// Resolves a specifier against its referrer.
    pub fn resolve(&self, referrer: &str, specifier: &str) -> Result<String, JsError> {
        let url = resolve(referrer, specifier)?;
        let path = path_of(&url).expect("resolve returns application URLs");
        if !self.source.contains(path) {
            return Err(JsError::new(format!(
                "the application has no module at {path} (imported as {specifier:?} \
                 from {referrer})"
            )));
        }
        Ok(url)
    }

    /// Returns a module's source, reading it once and remembering it.
    pub fn source(&self, url: &str) -> Result<Rc<String>, JsError> {
        if let Some(module) = self.loaded.borrow().get(url) {
            return Ok(Rc::clone(&module.source));
        }
        let path =
            path_of(url).ok_or_else(|| JsError::new(format!("{url} is not an application URL")))?;
        let bytes = self
            .source
            .read(path)
            .ok_or_else(|| JsError::new(format!("the application has no module at {path}")))?;
        let source = String::from_utf8(bytes)
            .map_err(|_| JsError::new(format!("the module at {path} is not UTF-8")))?;
        let source = Rc::new(source);
        let source_map = source_mapping_url(&source)
            .and_then(|reference| self.load_source_map(url, reference).ok());
        self.loaded.borrow_mut().insert(
            url.to_owned(),
            LoadedModule {
                source: Rc::clone(&source),
                source_map,
            },
        );
        Ok(source)
    }

    fn load_source_map(&self, module_url: &str, reference: &str) -> Result<SourceMap, String> {
        if reference.starts_with("data:") {
            let bytes = decode_data_url(reference)?;
            return SourceMap::parse(&bytes, module_url);
        }
        // Unlike an ECMAScript import, a URL reference need not start `./`.
        // `app.js.map` and `../maps/app.js.map` are equally ordinary here.
        let relative;
        let reference = if reference.starts_with('/')
            || reference.starts_with("./")
            || reference.starts_with("../")
            || reference.starts_with(APP_ORIGIN)
        {
            reference
        } else {
            relative = format!("./{reference}");
            &relative
        };
        let map_url = resolve(module_url, reference).map_err(|error| error.to_string())?;
        let path = path_of(&map_url).expect("resolve returns an application URL");
        let bytes = self
            .source
            .read(path)
            .ok_or_else(|| format!("the application has no source map at {path}"))?;
        SourceMap::parse(&bytes, &map_url)
    }

    /// Applies cached mappings to the application frames of an uncaught error.
    /// Missing and malformed maps leave their frames untouched: diagnostics
    /// must never be hidden by optional debugging metadata.
    pub fn remap_error(&self, error: &JsError) -> JsError {
        let Some(stack) = error.stack() else {
            return error.clone();
        };
        JsError::with_stack(error.message(), self.remap_diagnostic(stack))
    }

    /// Remaps application frames in a diagnostic already rendered by
    /// JavaScript, as worker `reportError` messages are.
    pub fn remap_diagnostic(&self, diagnostic: &str) -> String {
        diagnostic
            .lines()
            .map(|line| self.remap_stack_line(line))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn remap_stack_line(&self, line: &str) -> String {
        let Some(location) = frame_location(line) else {
            return line.to_owned();
        };
        let loaded = self.loaded.borrow();
        let Some(source_map) = loaded
            .get(&line[location.url_start..location.url_end])
            .and_then(|module| module.source_map.as_ref())
        else {
            return line.to_owned();
        };
        let Some((source, original_line, original_column)) =
            source_map.original_position(location.line, location.column)
        else {
            return line.to_owned();
        };
        format!(
            "{}{source}:{original_line}:{original_column}{}",
            &line[..location.url_start],
            &line[location.end..]
        )
    }

    /// Forgets every loaded module, so a reload re-reads them.
    pub fn reset(&self) {
        self.loaded.borrow_mut().clear();
    }

    /// Reads any application file, for asset URLs rather than modules.
    pub fn read(&self, url: &str) -> Result<Vec<u8>, JsError> {
        let path =
            path_of(url).ok_or_else(|| JsError::new(format!("{url} is not an application URL")))?;
        self.source
            .read(path)
            .ok_or_else(|| JsError::new(format!("the application has no file at {path}")))
    }

    /// Installs the entry points the engine's module loader calls back into.
    ///
    /// The engine resolves and fetches through these rather than through a
    /// filesystem, which is what lets an exported executable serve its own
    /// module graph without unpacking anything.
    pub fn install<E: JsEngine + 'static>(self: &Rc<Self>, engine: &mut E) -> Result<(), JsError> {
        let resolver = Rc::clone(self);
        engine.define_global_function(
            "__blitsenModuleResolve",
            Box::new(move |call| {
                let mut engine = E::from_value(&call.this);
                let referrer = argument(&mut engine, &call, 0, "importing module")?;
                let specifier = argument(&mut engine, &call, 1, "import specifier")?;
                engine.string(&resolver.resolve(&referrer, &specifier)?)
            }),
        )?;

        let reader = Rc::clone(self);
        engine.define_global_function(
            "__blitsenModuleSource",
            Box::new(move |call| {
                let mut engine = E::from_value(&call.this);
                let url = argument(&mut engine, &call, 0, "module url")?;
                engine.string(&reader.source(&url)?)
            }),
        )?;

        let cache = Rc::clone(self);
        engine.define_global_function(
            "__blitsenModuleReset",
            Box::new(move |call| {
                cache.reset();
                Ok(call.this)
            }),
        )?;

        let origin = engine.string(APP_ORIGIN)?;
        engine.set_global("__blitsenAppOrigin", &origin)
    }
}

struct FrameLocation {
    url_start: usize,
    url_end: usize,
    end: usize,
    line: u32,
    column: u32,
}

fn frame_location(frame: &str) -> Option<FrameLocation> {
    let url_start = frame.find(APP_ORIGIN)?;
    let rest = &frame[url_start..];
    for (first_colon, _) in rest.match_indices(':').rev() {
        let after_line = &rest[first_colon + 1..];
        let line_digits = after_line.bytes().take_while(u8::is_ascii_digit).count();
        if line_digits == 0 || after_line.as_bytes().get(line_digits) != Some(&b':') {
            continue;
        }
        let column_start = first_colon + 1 + line_digits + 1;
        let column_digits = rest[column_start..]
            .bytes()
            .take_while(u8::is_ascii_digit)
            .count();
        if column_digits == 0 {
            continue;
        }
        let line = rest[first_colon + 1..first_colon + 1 + line_digits]
            .parse()
            .ok()?;
        let column = rest[column_start..column_start + column_digits]
            .parse()
            .ok()?;
        return Some(FrameLocation {
            url_start,
            url_end: url_start + first_colon,
            end: url_start + column_start + column_digits,
            line,
            column,
        });
    }
    None
}

fn source_mapping_url(source: &str) -> Option<&str> {
    let mut found = None;
    for (offset, _) in source.match_indices("sourceMappingURL=") {
        let before = &source[..offset];
        let line_start = before.rfind(['\n', '\r']).map_or(0, |index| index + 1);
        let line_prefix = &source[line_start..offset];
        if let Some(comment) = line_prefix.rfind("//") {
            let marker = line_prefix[comment + 2..].trim();
            if marker == "#" || marker == "@" {
                let value_start = offset + "sourceMappingURL=".len();
                let value_end = source[value_start..]
                    .find(['\n', '\r'])
                    .map_or(source.len(), |index| value_start + index);
                found = nonempty(&source[value_start..value_end]);
                continue;
            }
        }
        if let Some(comment) = before.rfind("/*") {
            if before[comment + 2..].trim() == "#" || before[comment + 2..].trim() == "@" {
                let value_start = offset + "sourceMappingURL=".len();
                if let Some(end) = source[value_start..].find("*/") {
                    found = nonempty(&source[value_start..value_start + end]);
                }
            }
        }
    }
    found
}

fn nonempty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn decode_data_url(url: &str) -> Result<Vec<u8>, String> {
    let (metadata, payload) = url
        .strip_prefix("data:")
        .and_then(|url| url.split_once(','))
        .ok_or_else(|| "invalid source-map data URL".to_owned())?;
    if metadata
        .split(';')
        .any(|part| part.eq_ignore_ascii_case("base64"))
    {
        base64::engine::general_purpose::STANDARD
            .decode(payload.trim())
            .map_err(|error| format!("invalid base64 source map: {error}"))
    } else {
        percent_decode(payload)
    }
}

fn percent_decode(payload: &str) -> Result<Vec<u8>, String> {
    let bytes = payload.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = bytes.get(index + 1).and_then(|byte| hex(*byte));
            let low = bytes.get(index + 2).and_then(|byte| hex(*byte));
            let (Some(high), Some(low)) = (high, low) else {
                return Err("invalid percent escape in source-map data URL".to_owned());
            };
            decoded.push(high << 4 | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    Ok(decoded)
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use blitsen_quickjs::QuickJs;

    use super::*;

    struct Fixture(Vec<&'static str>);

    impl AppSource for Fixture {
        fn read(&self, path: &str) -> Option<Vec<u8>> {
            self.0
                .contains(&path)
                .then(|| format!("// {path}").into_bytes())
        }
    }

    struct ContentFixture(HashMap<&'static str, &'static str>);

    impl AppSource for ContentFixture {
        fn read(&self, path: &str) -> Option<Vec<u8>> {
            self.0.get(path).map(|source| source.as_bytes().to_vec())
        }
    }

    fn registry(files: Vec<&'static str>) -> Rc<ModuleRegistry> {
        Rc::new(ModuleRegistry::new(Arc::new(Fixture(files))))
    }

    #[test]
    fn relative_specifiers_resolve_against_the_importing_module() {
        let entry = url_of("assets/index-a1b2c3.js");
        assert_eq!(
            resolve(&entry, "./chunk-d4e5.js").unwrap(),
            url_of("assets/chunk-d4e5.js")
        );
        assert_eq!(
            resolve(&entry, "../vendor/react.js").unwrap(),
            url_of("vendor/react.js")
        );
        assert_eq!(resolve(&entry, "/main.js").unwrap(), url_of("main.js"));
        assert_eq!(
            resolve(&entry, &url_of("other.js")).unwrap(),
            url_of("other.js")
        );
    }

    #[test]
    fn a_query_is_kept_and_a_fragment_is_not() {
        let entry = url_of("assets/index.js");
        // The query survives resolution, because a server answers it: proxy
        // mode (#67) asks for `/src/main.jsx?t=1738` as written, and two
        // versions of one module are two modules.
        assert_eq!(
            resolve(&entry, "./worker.js?worker&url").unwrap(),
            url_of("assets/worker.js?worker&url")
        );
        // A file-backed source opens the file the URL names, which is what a
        // file server would have done with the query.
        assert_eq!(
            file_of(path_of(&resolve(&entry, "./worker.js?worker&url").unwrap()).unwrap()),
            "assets/worker.js"
        );
        // A fragment names a place inside a document and never reaches a source.
        assert_eq!(
            resolve(&entry, "./styles.css#layer").unwrap(),
            url_of("assets/styles.css")
        );
    }

    #[test]
    fn a_node_or_bun_builtin_is_told_it_is_not_implemented() {
        let entry = url_of("assets/index.js");
        for (specifier, builtin) in [("node:fs", "fs"), ("bun:sqlite", "sqlite")] {
            let error = resolve(&entry, specifier).unwrap_err();
            assert_eq!(
                error.message(),
                format!(
                    "the shipped Blitsen runtime implements no {specifier}. Node and Bun builtins \
                     are the toolchain's, not the runtime's; use the `blitsen/*` native modules \
                     for system access, or bundle a browser-targeted replacement for {builtin}."
                )
            );
        }
    }

    #[test]
    fn the_builtin_diagnostic_survives_the_public_module_loader_path_exactly() {
        let mut engine = QuickJs::new().expect("a QuickJS runtime");
        registry(Vec::new())
            .install(&mut engine)
            .expect("the application registry installs");
        engine.install_module_loader();

        let error = engine
            .evaluate_module(r#"import "node:fs";"#, &url_of("index.js"))
            .err()
            .expect("the unsupported builtin rejects module evaluation");
        assert_eq!(
            error.message(),
            "Error: the shipped Blitsen runtime implements no node:fs. Node and Bun builtins are \
             the toolchain's, not the runtime's; use the `blitsen/*` native modules for system \
             access, or bundle a browser-targeted replacement for fs."
        );
    }

    #[test]
    fn what_only_a_bundler_could_resolve_says_so() {
        let entry = url_of("assets/index.js");
        let bare = resolve(&entry, "react").unwrap_err();
        assert!(bare.message().contains("bare module specifier"));
        assert!(bare.message().contains("build the application"));

        let remote = resolve(&entry, "https://esm.sh/react").unwrap_err();
        assert!(
            remote
                .message()
                .contains("does not fetch modules over the network")
        );
        assert!(
            resolve(&entry, "//esm.sh/react")
                .unwrap_err()
                .message()
                .contains("network")
        );
    }

    #[test]
    fn nothing_resolves_outside_the_application() {
        let entry = url_of("index.js");
        assert!(
            resolve(&entry, "../../../etc/passwd")
                .unwrap_err()
                .message()
                .contains("outside the application")
        );
        assert!(resolve(&entry, "").is_err());
        assert!(resolve(&entry, "./").is_err());
        // Deep enough to come back inside is still inside.
        assert_eq!(
            resolve(&url_of("a/b/c.js"), "../../d.js").unwrap(),
            url_of("d.js")
        );
    }

    #[test]
    fn a_missing_module_names_the_import_that_wanted_it() {
        let registry = registry(vec!["assets/index.js"]);
        let entry = url_of("assets/index.js");
        assert_eq!(registry.resolve(&entry, "./index.js").unwrap(), entry);
        let error = registry.resolve(&entry, "./missing.js").unwrap_err();
        assert!(error.message().contains("assets/missing.js"));
        assert!(error.message().contains("\"./missing.js\""));
        assert!(error.message().contains(&entry));
    }

    #[test]
    fn a_module_is_read_once_and_forgotten_on_reset() {
        let registry = registry(vec!["a.js"]);
        let url = url_of("a.js");
        let first = registry.source(&url).unwrap();
        let second = registry.source(&url).unwrap();
        assert!(Rc::ptr_eq(&first, &second));
        assert_eq!(*first, "// a.js");
        registry.reset();
        assert!(!Rc::ptr_eq(&first, &registry.source(&url).unwrap()));
    }

    #[test]
    fn external_maps_follow_the_last_directive_and_keep_map_queries() {
        let registry = ModuleRegistry::new(Arc::new(ContentFixture(HashMap::from([
            (
                "generated.js?v=7",
                "throw Error('mapped');\n//# sourceMappingURL=old.map\n/*# sourceMappingURL=generated.js.map?v=7 */",
            ),
            (
                "generated.js.map?v=7",
                r#"{"version":3,"sourceRoot":"source","sources":["original.ts"],"mappings":"AAoBK"}"#,
            ),
        ]))));
        let generated = url_of("generated.js?v=7");
        registry.source(&generated).unwrap();
        let error = JsError::with_stack("Error: mapped", format!("    at fail ({generated}:1:18)"));
        assert_eq!(
            registry.remap_error(&error).stack(),
            Some("    at fail (blitsen://app/source/original.ts:21:6)")
        );
    }

    #[test]
    fn inline_base64_and_percent_encoded_maps_are_consumed() {
        let json = r#"{"version":3,"sources":["original.ts"],"mappings":"AAAA"}"#;
        let base64 = base64::engine::general_purpose::STANDARD.encode(json);
        let encoded = format!(
            "throw Error('inline');\n//# sourceMappingURL=data:application/json;base64,{base64}"
        );
        let percent = "throw Error('percent');\n//# sourceMappingURL=data:application/json,%7B%22version%22%3A3%2C%22sources%22%3A%5B%22percent.ts%22%5D%2C%22mappings%22%3A%22AAAA%22%7D";
        let registry = ModuleRegistry::new(Arc::new(ContentFixture(HashMap::from([
            (
                "inline.js",
                Box::leak(encoded.into_boxed_str()) as &'static str,
            ),
            ("percent.js", percent),
        ]))));
        for (module, original) in [("inline.js", "original.ts"), ("percent.js", "percent.ts")] {
            let generated = url_of(module);
            registry.source(&generated).unwrap();
            let error = JsError::with_stack("Error", format!("at {generated}:1:1"));
            assert_eq!(
                registry.remap_error(&error).stack(),
                Some(format!("at blitsen://app/{original}:1:1").as_str())
            );
        }
    }

    #[test]
    fn missing_and_malformed_maps_leave_diagnostics_usable() {
        let registry = ModuleRegistry::new(Arc::new(ContentFixture(HashMap::from([
            (
                "missing.js",
                "throw 1;\n//# sourceMappingURL=missing.js.map",
            ),
            (
                "malformed.js",
                "throw 2;\n//# sourceMappingURL=malformed.js.map",
            ),
            ("malformed.js.map", "not JSON"),
        ]))));
        for module in ["missing.js", "malformed.js"] {
            let generated = url_of(module);
            registry.source(&generated).unwrap();
            let stack = format!("at {generated}:1:1");
            let error = JsError::with_stack("Error", &stack);
            assert_eq!(registry.remap_error(&error).stack(), Some(stack.as_str()));
        }
    }

    #[test]
    fn directive_discovery_only_accepts_comments() {
        assert_eq!(
            source_mapping_url(
                "const fake = 'sourceMappingURL=no.map';\n//@ sourceMappingURL=first.map\n//# sourceMappingURL=last.map"
            ),
            Some("last.map")
        );
        assert_eq!(
            source_mapping_url("code(); /*# sourceMappingURL=block.map */"),
            Some("block.map")
        );
        assert_eq!(
            source_mapping_url("const x = 'sourceMappingURL=no.map'"),
            None
        );
    }

    #[test]
    fn a_directory_source_serves_only_what_is_under_its_root() {
        let root = std::env::temp_dir().join(format!("blitsen-modules-{}", std::process::id()));
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join("assets/index.js"), "export default 1").unwrap();
        let source = DirectorySource::new(&root);
        assert_eq!(source.read("assets/index.js").unwrap(), b"export default 1");
        assert!(source.read("assets/missing.js").is_none());
        assert!(source.read("../../../etc/passwd").is_none());
        std::fs::remove_dir_all(&root).ok();
    }
}
