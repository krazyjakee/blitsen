//! Booting an application, and the outer event loop that keeps it running.
//!
//! Phase 1 leaves both to Bun: a JavaScript launcher opens the window through
//! the addon and pumps it from a task on Bun's loop (TECH.md §3, S1 option 1).
//! Here there is no other loop to be inside, so this is it.

use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;

use blitsen_core::bundle::AppBundle;
use blitsen_host::app::AppFiles;
use blitsen_host::modules::ModuleRegistry;
use blitsen_host::runtime_services::RuntimeServices;
use blitsen_host::{OpenDirectoryOptions, WindowSession, native_window};
use blitsen_js::JsEngine;
use blitsen_jsc::JavaScriptCore;

use crate::loop_pacing::Pacer;

/// Window settings an application carries, written into the bundle at export.
///
/// The same keys `blitsen.config.json` uses, so the CLI copies them across
/// rather than translating them (structural constraint 7).
const DEFAULT_ENTRYPOINT: &str = "index.html";

struct Settings {
    width: u32,
    height: u32,
    title: String,
    /// How the export laid its assets out, echoed by the standalone check.
    layout: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            width: 1024,
            height: 768,
            title: "Blitsen".to_owned(),
            layout: "embedded".to_owned(),
        }
    }
}

impl Settings {
    fn read(files: &AppFiles, arguments: &[String]) -> Result<Self, String> {
        let mut settings = Self::default();
        if let Some(bytes) = files.source().read(blitsen_host::app::RUNTIME_CONFIG) {
            let config: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|error| format!("blitsen.runtime.json is not valid JSON: {error}"))?;
            if let Some(width) = config.get("width").and_then(serde_json::Value::as_u64) {
                settings.width = width as u32;
            }
            if let Some(height) = config.get("height").and_then(serde_json::Value::as_u64) {
                settings.height = height as u32;
            }
            if let Some(title) = config.get("title").and_then(serde_json::Value::as_str) {
                settings.title = title.to_owned();
            }
            if let Some(layout) = config.get("layout").and_then(serde_json::Value::as_str) {
                settings.layout = layout.to_owned();
            }
        }
        let mut arguments = arguments.iter();
        while let Some(argument) = arguments.next() {
            let mut value = || {
                arguments
                    .next()
                    .cloned()
                    .ok_or_else(|| format!("{argument} needs a value"))
            };
            match argument.as_str() {
                "--width" => {
                    settings.width = value()?
                        .parse()
                        .map_err(|_| "--width must be a whole number of pixels".to_owned())?;
                }
                "--height" => {
                    settings.height = value()?
                        .parse()
                        .map_err(|_| "--height must be a whole number of pixels".to_owned())?;
                }
                "--title" => settings.title = value()?,
                other => return Err(format!("unknown option {other}")),
            }
        }
        Ok(settings)
    }
}

/// Runs a directory of built output, as `blitsen run` does.
pub fn run_directory(directory: PathBuf, arguments: &[String]) -> Result<ExitCode, String> {
    let entrypoint = if directory.is_dir() {
        directory.join(DEFAULT_ENTRYPOINT)
    } else {
        directory.clone()
    };
    if !entrypoint.is_file() {
        return Err(format!(
            "{} has no {DEFAULT_ENTRYPOINT}",
            directory.display()
        ));
    }
    let files = AppFiles::directory(&entrypoint).map_err(|error| error.to_string())?;
    run(files, arguments)
}

/// Runs the application appended to this executable.
pub fn run_bundle(bundle: AppBundle, arguments: &[String]) -> Result<ExitCode, String> {
    let files = AppFiles::bundle(bundle, DEFAULT_ENTRYPOINT).map_err(|error| error.to_string())?;
    run(files, arguments)
}

fn run(files: AppFiles, arguments: &[String]) -> Result<ExitCode, String> {
    let settings = Settings::read(&files, arguments)?;
    let mut engine = JavaScriptCore::load().map_err(|error| error.to_string())?;

    // Order matters. The services install the timers and the console the DOM
    // bootstrap captures as it loads, and the module loader has to be in place
    // before the first `<script type=module>` runs.
    let services = RuntimeServices::install(&mut engine).map_err(|error| error.to_string())?;
    let modules = Rc::new(ModuleRegistry::new(files.source()));
    modules
        .install(&mut engine)
        .map_err(|error| error.to_string())?;
    install_module_loader(&mut engine)?;

    if blitsen_host::standalone::requested() {
        // Answering the check is the whole of this run: no window opens, and
        // the process exits as soon as it has said what it found.
        blitsen_host::standalone::run(
            &mut engine,
            &services,
            &files,
            &blitsen_host::standalone::Reported {
                width: settings.width,
                height: settings.height,
                assets: files.asset_count(),
                layout: &settings.layout,
                runtime: concat!("blitsen-runtime ", env!("CARGO_PKG_VERSION")),
            },
        )
        .map_err(|error| error.to_string())?;
        return Ok(ExitCode::SUCCESS);
    }

    let options = OpenDirectoryOptions {
        root: match &files {
            AppFiles::Directory { root, .. } => root.to_string_lossy().into_owned(),
            AppFiles::Bundle { .. } => blitsen_host::modules::APP_ORIGIN.to_owned(),
        },
        entrypoint: files.entrypoint_name(),
        width: settings.width,
        height: settings.height,
        title: settings.title,
        directory: files.entrypoint_name(),
    };
    let mut session =
        WindowSession::open(&mut engine, files, options).map_err(|error| error.to_string())?;

    let mut pacer = Pacer::from_environment();
    loop {
        // A macrotask turn, then the frame it may have dirtied. Animation
        // frames and microtask draining happen inside the pump, where the
        // window's redraw already sequences them (TECH.md §6).
        services
            .run_expired_timers(&mut engine)
            .map_err(|error| error.to_string())?;
        if !session.pump().map_err(|error| error.to_string())? {
            break;
        }
        if pacer.finished() {
            break;
        }
        pacer.wait(services.next_timer_delay());
    }
    native_window::release_window();
    pacer.report();
    Ok(ExitCode::SUCCESS)
}

/// Points the engine's module loader at the resolver, when it can be pointed.
///
/// A build against a JavaScriptCore without the hook still runs an application
/// whose scripts are classic, which is every acceptance fixture and most hand
/// written pages. It refuses at the first `import` instead of silently
/// rendering a blank window, and says which library to use.
fn install_module_loader(engine: &mut JavaScriptCore) -> Result<(), String> {
    if !engine.supports_modules() {
        if std::env::var_os("BLITSEN_REQUIRE_MODULES").is_some() {
            return Err(
                "this JavaScriptCore library cannot link a module graph; use Blitsen's \
                 pinned build (docs/JSC.md)"
                    .to_owned(),
            );
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
