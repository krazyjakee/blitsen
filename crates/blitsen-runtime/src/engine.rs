//! Which JavaScript engine this runtime hosts, and the two things that differ.
//!
//! Everything else in this executable — and all of `blitsen-host` beneath it —
//! is generic over [`blitsen_js::JsEngine`]. Only loading the engine and
//! pointing its module loader at the registry are engine-specific, so only
//! those two live here. Selecting the engine is therefore this file and a
//! feature flag, which is the claim `spikes/s8` set out to test.

/// The engine this build hosts.
#[cfg(not(feature = "quickjs"))]
pub type Engine = blitsen_jsc::JavaScriptCore;
/// The engine this build hosts.
#[cfg(feature = "quickjs")]
pub type Engine = blitsen_quickjs::QuickJs;

/// What `--engine-report` and the standalone check call this build.
#[cfg(not(feature = "quickjs"))]
pub const NAME: &str = "JavaScriptCore";
/// How the engine reaches this process, which decides whether anything has to
/// ship beside the executable.
#[cfg(not(feature = "quickjs"))]
pub const LINKAGE: &str = "dynamic";
/// What `--engine-report` and the standalone check call this build.
#[cfg(feature = "quickjs")]
pub const NAME: &str = "QuickJS-ng";
/// How the engine reaches this process, which decides whether anything has to
/// ship beside the executable.
#[cfg(feature = "quickjs")]
pub const LINKAGE: &str = "static";

/// Loads the engine, however this build obtains one.
#[cfg(not(feature = "quickjs"))]
pub fn load() -> Result<Engine, String> {
    blitsen_jsc::JavaScriptCore::load().map_err(|error| error.to_string())
}

/// Loads the engine, however this build obtains one.
///
/// Nothing to find: the engine is inside the executable.
#[cfg(feature = "quickjs")]
pub fn load() -> Result<Engine, String> {
    blitsen_quickjs::QuickJs::new().map_err(|error| error.to_string())
}

/// Points the engine's module loader at the resolver, when it can be pointed.
///
/// A JavaScriptCore without the hook still runs an application whose scripts
/// are classic, which is every acceptance fixture and most hand-written pages.
/// It refuses at the first `import` instead of silently rendering a blank
/// window, and says which library to use. QuickJS needs no such caveat: module
/// loading is in its stock public API.
#[cfg(not(feature = "quickjs"))]
pub fn install_module_loader(engine: &mut Engine) -> Result<(), String> {
    use blitsen_js::JsEngine;
    if !engine.supports_modules() {
        if std::env::var_os("BLITSEN_REQUIRE_MODULES").is_some() {
            return Err("this JavaScriptCore library cannot link a module graph; use Blitsen's \
                 pinned build (docs/JSC.md)"
                .to_owned());
        }
        return Ok(());
    }
    let resolve = engine
        .evaluate_script(
            "globalThis.__blitsenModuleResolve",
            "blitsen:module-resolve",
        )
        .map_err(|error| error.to_string())?;
    let fetch = engine
        .evaluate_script("globalThis.__blitsenModuleSource", "blitsen:module-source")
        .map_err(|error| error.to_string())?;
    engine
        .set_module_loader(&resolve, &fetch)
        .map_err(|error| error.to_string())
}

/// Points the engine's module loader at the resolver.
#[cfg(feature = "quickjs")]
pub fn install_module_loader(engine: &mut Engine) -> Result<(), String> {
    engine.install_module_loader();
    Ok(())
}

/// Whether this build can evaluate module scripts, for `--engine-report`.
#[cfg(not(feature = "quickjs"))]
pub fn supports_modules(engine: &Engine) -> bool {
    engine.supports_modules()
}

/// Whether this build can evaluate module scripts, for `--engine-report`.
#[cfg(feature = "quickjs")]
pub fn supports_modules(engine: &Engine) -> bool {
    engine.supports_modules()
}

/// The engine library this build was pointed at, when it loads one at all.
#[cfg(not(feature = "quickjs"))]
pub fn library_override() -> Option<String> {
    std::env::var(blitsen_jsc::LIBRARY_ENV).ok()
}

/// The engine library this build was pointed at, when it loads one at all.
///
/// Always `None`: there is no library to override, which is the whole point.
#[cfg(feature = "quickjs")]
pub fn library_override() -> Option<String> {
    None
}

/// Which of `names` this engine does not define.
///
/// Asked of the engine rather than assumed, because the whole point of the
/// report is to be checkable against what the compatibility profile claims.
pub fn absent_globals(engine: &mut Engine, names: &[&str]) -> Vec<String> {
    use blitsen_js::JsEngine;
    names
        .iter()
        .filter(|name| {
            engine
                .evaluate_script(&format!("typeof {name}"), "blitsen:engine-globals")
                .ok()
                .and_then(|value| engine.to_string(&value).ok())
                .is_none_or(|kind| kind == "undefined")
        })
        .map(|name| (*name).to_owned())
        .collect()
}
