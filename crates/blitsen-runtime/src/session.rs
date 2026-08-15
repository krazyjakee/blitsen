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

use crate::engine;
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
    /// The runtime this application was linked against (#73), as the exporter
    /// recorded it. A directory run has no record and reports this executable.
    runtime: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            width: 1024,
            height: 768,
            title: "Blitsen".to_owned(),
            layout: "embedded".to_owned(),
            runtime: concat!("blitsen-runtime ", env!("CARGO_PKG_VERSION")).to_owned(),
        }
    }
}

/// The recorded runtime, worded exactly as `describeRuntime` in the CLI words
/// it — the same export prints the same line whichever host it was linked into.
fn describe_runtime(record: &serde_json::Value) -> Option<String> {
    let text = |key| record.get(key).and_then(serde_json::Value::as_str);
    match (text("package"), text("version")) {
        (Some(package), Some(version)) => Some(format!("{package}@{version}")),
        _ => Some(format!(
            "{} (unversioned, from {})",
            text("target")?,
            text("source")?
        )),
    }
}

impl Settings {
    fn read(files: &AppFiles, arguments: &[String]) -> Result<Self, String> {
        let mut settings = Self::default();
        // Only an export carries one. Asking a dev server for it would be a
        // request per start for a file no dev server has (issue #67).
        let configured = match files {
            AppFiles::Server { .. } => None,
            _ => files.source().read(blitsen_host::app::RUNTIME_CONFIG),
        };
        if let Some(bytes) = configured {
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
            if let Some(runtime) = config.get("runtime").and_then(describe_runtime) {
                settings.runtime = runtime;
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

/// Runs what a development server is serving (issue #67).
///
/// `blitsen http://localhost:5173`: the window replaces the browser tab, and the
/// user's own dev server goes on transforming, hot-reloading and source-mapping
/// exactly as it did. Nothing is built, ingested or copied.
pub fn run_url(url: &str, arguments: &[String]) -> Result<ExitCode, String> {
    let files = AppFiles::server(url).map_err(|error| error.to_string())?;
    run(files, arguments)
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
    // Before anything can construct a `Worker`, and once for the process: which
    // engine a worker thread runs is a property of this executable.
    blitsen_host::worker::register_launcher(Box::new(engine::Workers));
    let mut engine = engine::load()?;

    // Order matters. The services install the timers and the console the DOM
    // bootstrap captures as it loads, and the module loader has to be in place
    // before the first `<script type=module>` runs.
    let services = RuntimeServices::install(&mut engine).map_err(|error| error.to_string())?;
    let modules = Rc::new(ModuleRegistry::new(files.source()));
    modules
        .install(&mut engine)
        .map_err(|error| error.to_string())?;
    engine::install_module_loader(&mut engine)?;

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
                runtime: &settings.runtime,
            },
        )
        .map_err(|error| error.to_string())?;
        return Ok(ExitCode::SUCCESS);
    }

    let options = OpenDirectoryOptions {
        root: match &files {
            AppFiles::Directory { root, .. } => root.to_string_lossy().into_owned(),
            AppFiles::Bundle { .. } | AppFiles::Server { .. } | AppFiles::Assets { .. } => {
                blitsen_host::modules::APP_ORIGIN.to_owned()
            }
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
    let mut pump_timeout = Some(std::time::Duration::ZERO);
    loop {
        // A macrotask turn, then the frame it may have dirtied. Animation
        // frames and microtask draining happen inside the pump, where the
        // window's redraw already sequences them (TECH.md §6).
        let timers_ran = services
            .run_expired_timers(&mut engine)
            .map_err(|error| error.to_string())?;
        if timers_ran > 0 {
            session.request_redraw();
        }
        if !session
            .pump_for(pump_timeout)
            .map_err(|error| error.to_string())?
        {
            break;
        }
        if pacer.finished() {
            break;
        }
        let next_timer = services.next_timer_delay();
        if pacer.forcing_frames() || session.animation_frames_pending() {
            pacer.wait(next_timer);
            pump_timeout = Some(std::time::Duration::ZERO);
        } else {
            // Winit's proxy wakes this wait for network, worker, and platform
            // events. A finite timeout wakes it when the next JS timer is due.
            pump_timeout = next_timer;
        }
    }
    native_window::release_window();
    pacer.report();
    Ok(ExitCode::SUCCESS)
}
