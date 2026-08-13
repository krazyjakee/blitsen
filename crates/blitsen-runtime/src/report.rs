//! What the runtime can say about itself without opening a window.
//!
//! These back the build-time checks: `--bundle-report` proves an exported
//! executable carries what the CLI thinks it wrote, and `--engine-report` says
//! which JavaScriptCore this binary found and what that library can do.

use std::path::Path;
use std::process::ExitCode;

use blitsen_core::bundle::AppBundle;
use blitsen_core::replay::InputTrace;
use blitsen_host::runtime_services::RuntimeServices;
use crate::engine;
use serde_json::json;

/// Prints the bundle appended to this executable, as JSON.
pub fn print(bundle: Option<&AppBundle>, executable: &Path) {
    let report = match bundle {
        None => json!({
            "executable": executable.to_string_lossy(),
            "bundled": false,
        }),
        Some(bundle) => {
            let files: Vec<_> = bundle
                .paths()
                .map(|path| {
                    let entry = bundle.entry(path).expect("path came from this bundle");
                    json!({ "path": path, "bytes": entry.length })
                })
                .collect();
            json!({
                "executable": executable.to_string_lossy(),
                "bundled": true,
                "formatVersion": blitsen_core::bundle::FORMAT_VERSION,
                "payloadBytes": bundle.byte_length(),
                "digest": bundle.digest(),
                "verified": bundle.verify().is_ok(),
                "files": files,
            })
        }
    };
    println!("{report}");
}

/// Language-level globals the compatibility profile makes a claim about.
///
/// These come from the engine rather than from the DOM bridge, so
/// `api-manifest.json` cannot derive them the way it derives everything else
/// and declares them instead. Reporting them here is what lets
/// `cli-doctor.test.mjs` fail when the declaration and the engine disagree.
const ENGINE_GLOBALS: &[&str] = &["Intl", "WebAssembly"];

/// Prints which JavaScriptCore was loaded and what it supports.
pub fn print_engine() {
    let report = match engine::load() {
        Ok(mut loaded) => json!({
            "loaded": true,
            "engine": engine::NAME,
            "modules": engine::supports_modules(&loaded),
            "absentGlobals": engine::absent_globals(&mut loaded, ENGINE_GLOBALS),
            // Present only when the engine is one the process loads at run
            // time. A statically linked engine has nothing to override, and
            // omitting the key says that better than a permanent null.
            "libraryOverride": engine::library_override(),
            "linkage": engine::LINKAGE,
        }),
        Err(error) => json!({ "loaded": false, "engine": engine::NAME, "error": error }),
    };
    println!("{report}");
}

/// Replays a recorded input trace and prints the report as JSON.
///
/// `--replay <entrypoint.html> <trace.json>`. The trace carries its own
/// viewport and timestep, so nothing here reads a clock JavaScript can see.
pub fn replay(arguments: &[String]) -> Result<ExitCode, String> {
    let [entrypoint, trace] = arguments else {
        return Err("--replay needs an HTML entrypoint and a recorded trace".to_owned());
    };
    let trace = std::fs::read_to_string(trace)
        .map_err(|error| format!("could not read {trace}: {error}"))?;
    let trace = InputTrace::from_json(&trace).map_err(|error| error.to_string())?;
    let mut engine = engine::load()?;
    // The replay runs its own fixed-timestep loop, but the document below it
    // still expects a host: a `setTimeout` inside it must resolve to something.
    let _services = RuntimeServices::install(&mut engine).map_err(|error| error.to_string())?;
    let report = blitsen_host::replay::replay(engine, Path::new(entrypoint), trace, None, &[])
        .map_err(|error| error.to_string())?;
    println!(
        "{}",
        serde_json::to_string(&report).map_err(|error| error.to_string())?
    );
    Ok(ExitCode::SUCCESS)
}
