//! The Phase 2 executable: Blitsen hosts JavaScriptCore.
//!
//! Same job as the Phase 1 pair of a Bun launcher and a `.node` addon, with
//! nothing of Bun in it. This process owns the outer event loop, the timers and
//! the module graph; JavaScriptCore is a library it loads (`docs/JSC.md`).
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

mod loop_pacing;
mod report;
mod session;

use std::path::PathBuf;
use std::process::ExitCode;

use blitsen_core::bundle::AppBundle;

fn main() -> ExitCode {
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
            println!("blitsen-runtime {}", env!("CARGO_PKG_VERSION"));
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
        // Frame determinism (issue #48) across the host swap: the same trace,
        // replayed at the same fixed timestep, has to produce the same digests
        // on both hosts. The Phase 1 side of this is `replayDocumentFrames` in
        // the addon; this is the same function behind the other engine.
        Some("--replay") => report::replay(&arguments[1..]),
        Some(argument) if argument.starts_with("--") => Err(format!("unknown option {argument}")),
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
