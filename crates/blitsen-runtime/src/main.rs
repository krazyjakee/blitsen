//! The Phase 2 executable: Blitsen hosts its own JavaScript engine.
//!
//! Same job as the Phase 1 pair of a Bun launcher and a `.node` addon, with
//! nothing of Bun in it. This process owns the outer event loop, the timers and
//! the module graph; QuickJS-ng is linked into it (`LICENSING.md`).
//!
//! Two ways to start, and they run the same code:
//!
//! ```text
//! blitsen-runtime ./dist              # a directory, as `blitsen run` does
//! MyApp                               # the bundle appended to this executable
//! ```
//!
//! An exported application is this binary with its files appended as a section
//! (`blitsen_core::bundle`), read in place rather than unpacked.
//!
//! What this file is, and is not: the console program around the runtime. The
//! runtime itself is the library beside it, because Android's entry point is a
//! `cdylib` calling the same code with no argv and no executable to read
//! (`lib.rs`, issue #142). Argument parsing, the reports, and the process-global
//! memory defaults stay here; everything after "here are the application's
//! files" is [`blitsen_runtime::session`].

mod memory_defaults;
mod report;

use std::path::PathBuf;
use std::process::ExitCode;

use blitsen_core::bundle::AppBundle;
use blitsen_runtime::session;

fn main() -> ExitCode {
    // Before Tokio, wgpu or a driver starts a thread: the environment changes
    // and glibc arena policy below are process-global initialization.
    memory_defaults::apply();
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("blitsen: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode, String> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let executable = std::env::current_exe().map_err(|error| {
        format!("could not find this executable, so its bundle cannot be read: {error}")
    })?;
    let bundle = AppBundle::open(&executable).map_err(|error| error.to_string())?;

    match arguments.first().map(String::as_str) {
        Some("--version") => {
            println!("{}", blitsen_core::runtime_identity());
            Ok(ExitCode::SUCCESS)
        }
        Some("--bundle-report") => {
            report::print(bundle.as_ref(), &executable);
            Ok(ExitCode::SUCCESS)
        }
        Some("--engine-report") => {
            report::print_engine();
            Ok(ExitCode::SUCCESS)
        }
        // The third-party notices this artifact carries (issue #121). Printed by
        // the executable itself, from the section appended to it, so what a
        // recipient reads is what was shipped rather than what a build log said.
        Some("--licenses") => report::print_licenses(bundle.as_ref()),
        // Frame determinism (issue #48) across the host swap: the same trace,
        // replayed at the same fixed timestep, has to produce the same digests
        // on both hosts. The Phase 1 side of this is `replayDocumentFrames` in
        // the addon; this is the same function behind the other engine.
        Some("--replay") => report::replay(&arguments[1..]),
        // Windows starts the registered toast COM local server with the first
        // flag; COM itself may append either conventional spelling. They are
        // launch provenance rather than application options, and the class
        // factory is installed while the bundled session opens (#252).
        Some("--notification-com-server" | "-ToastActivated" | "-Embedding") => match bundle {
            Some(bundle) => session::run_bundle(bundle, &arguments),
            None => Err("the notification COM server carries no application bundle".to_owned()),
        },
        Some(argument) if argument.starts_with("--") => Err(format!("unknown option {argument}")),
        // Proxy mode (#67): a URL is a dev server to read the application from,
        // rather than a directory to read it from. Same session either way.
        Some(url) if url.starts_with("http://") || url.starts_with("https://") => {
            session::run_url(url, &arguments[1..])
        }
        Some(directory) => session::run_directory(PathBuf::from(directory), &arguments[1..]),
        None => match bundle {
            Some(bundle) => session::run_bundle(bundle, &arguments),
            None => Err(
                "this runtime carries no application. Give it a directory of built output, \
                 or export one into it with `blitsen build`."
                    .to_owned(),
            ),
        },
    }
}
