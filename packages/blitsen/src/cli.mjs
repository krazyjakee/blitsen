import { access, realpath } from "node:fs/promises";
import { constants, watch as watchFs } from "node:fs";
import { extname, join, normalize, resolve } from "node:path";
import { loadConfig, runBuildCommand } from "./config.mjs";
import { doctorApplication, formatDiagnostic } from "./doctor.mjs";
import { buildStandalone } from "./export.mjs";
import { describeRuntime, hostTarget, openRuntime, packageVersion, resolveRuntime, TARGETS }
  from "./runtime.mjs";

const HELP = `Usage: blitsen [directory] [options]
       blitsen build [directory] [options]
       blitsen doctor <directory> [--json]

Open <directory>/index.html in a native Blitsen window, defaulting to the
current directory.
Build creates a single-file executable: Blitsen's own runtime with the
application appended to it. With no directory it reads the "blitsen" config in
package.json, runs the configured build command, and ingests its output
directory.
Doctor checks built static output against the v1 compatibility profile.

Options:
  --width <pixels>   Initial logical width (default: 800)
  --height <pixels>  Initial logical height (default: 600)
  --title <text>     Native window title (default: the application name)
  --name <text>      Application name: window title and default output name
  --out <path>       Build output path (default: the application name)
  --outfile <path>   Alias of --out
  --target <triple>  Build for another platform; its runtime is fetched and cached
  --include <glob>   Keep an unreferenced output file (repeatable)
  --addon <path>     Carry a .node addon into the export (repeatable)
  --accept-errors    Export despite compatibility errors, accepting what they cost
  --assets <layout>  embedded (default) or side-loaded next to the executable
  --icon <path>      Application icon: PNG, or a platform-native .ico/.icns/.svg
  --bundle-id <id>   macOS CFBundleIdentifier (default: com.blitsen.<title>)
  --app-version <v>  Application version recorded in the platform metadata
  --sign <command>   Signing hook, run with the packaged artifact as its argument
  --force            Replace an existing build output
  --json             Emit the doctor report as JSON
  -h, --help         Show help
  -v, --version      Show version`;

// The resolver owns it now, because the version pin is checked there; still on this
// module's surface, which is where callers ask for it.
export { packageVersion };

const PACKAGE_OPTIONS = { "--icon": "icon", "--bundle-id": "bundleId", "--app-version": "appVersion", "--sign": "sign" };
const BUILD_OPTIONS = ["--out", "--outfile", "--name", "--target", "--include", "--addon", "--assets",
  ...Object.keys(PACKAGE_OPTIONS)];
const VALUE_OPTIONS = ["--width", "--height", "--title", ...BUILD_OPTIONS];
// A build-only switch: doctor's own exit code must keep meaning what it says.
const BUILD_FLAGS = ["--accept-errors"];
// TECH.md §11: one binary package per target (src/runtime.mjs). A cross-target
// build links that target's runtime, fetched on demand (#72), and compiles the
// launcher for that target's Bun. What it cannot do is sign or notarise for a
// platform it is not running on — see the note in the build path.
function checkTarget(value) {
  if (!TARGETS.includes(value)) {
    throw new Error(`unknown --target ${value} (expected one of: ${TARGETS.join(", ")})`);
  }
}

export function parseArgs(args) {
  if (args.includes("--help") || args.includes("-h")) {
    return { help: true };
  }
  if (args.includes("--version") || args.includes("-v")) {
    return { version: true };
  }
  const command = ["build", "doctor"].includes(args[0]) ? args[0] : "run";
  const options = { command, directory: null, width: 800, height: 600, title: "Blitsen" };
  for (let index = command === "run" ? 0 : 1; index < args.length; index += 1) {
    const argument = args[index];
    if (VALUE_OPTIONS.includes(argument)) {
      const value = args[++index];
      if (value === undefined) throw new Error(`${argument} requires a value`);
      if (command === "doctor") throw new Error(`${argument} is not valid with doctor`);
      if (BUILD_OPTIONS.includes(argument) && command !== "build") {
        throw new Error(`${argument} is only valid with build`);
      }
      if (PACKAGE_OPTIONS[argument]) options[PACKAGE_OPTIONS[argument]] = value;
      else if (argument === "--title") options.title = value;
      else if (argument === "--name") options.name = value;
      else if (argument === "--out" || argument === "--outfile") options.outfile = value;
      else if (argument === "--target") {
        checkTarget(value);
        options.target = value;
      }
      else if (argument === "--include") options.include = [...options.include ?? [], value];
      // Resolved here rather than in the exporter: the path is the user's, and it
      // usually points outside the directory being ingested.
      else if (argument === "--addon") options.addons = [...options.addons ?? [], resolve(value)];
      else if (argument === "--assets") {
        if (!["embedded", "side-loaded"].includes(value))
          throw new Error("--assets must be embedded or side-loaded");
        options.assets = value;
      }
      else {
        const pixels = Number(value);
        if (!Number.isInteger(pixels) || pixels <= 0)
          throw new Error(`${argument} must be a positive integer`);
        options[argument.slice(2)] = pixels;
      }
    } else if (argument === "--force") {
      if (command !== "build") throw new Error("--force is only valid with build");
      options.force = true;
    } else if (BUILD_FLAGS.includes(argument)) {
      if (command !== "build") throw new Error(`${argument} is only valid with build`);
      options.acceptErrors = true;
    } else if (argument === "--json") {
      if (command !== "doctor") throw new Error("--json is only valid with doctor");
      options.json = true;
    } else if (argument.startsWith("-")) {
      throw new Error(`unknown option: ${argument}`);
    } else if (options.directory === null) {
      options.directory = argument;
    } else {
      throw new Error(`unexpected argument: ${argument}`);
    }
  }
  // A build with no directory is the configured one, and a run with no directory
  // is the one you are standing in — the input to Blitsen is a directory of
  // static web output, and the working directory is a fair guess at which.
  // Doctor is pointed rather than guessed: it grades build output, and defaulting
  // it to wherever the shell happens to be would grade the wrong tree in silence.
  if (options.directory === null && options.command === "run") options.directory = ".";
  if (options.directory === null && options.command === "doctor") {
    throw new Error("missing application directory");
  }
  applyName(options);
  return options;
}

// The window title follows the application name unless --title says otherwise;
// the two are only distinguishable when the title is still its default.
function applyName(options) {
  if (options.name !== undefined && options.title === "Blitsen") options.title = options.name;
}

const STEPS = { build: "⓪", ingest: "①", scan: "②", collect: "③", link: "④", package: "⑤" };
const NOTE_INDENT = " ".repeat(10);

function reportStep(output, { step, detail, notes = [] }) {
  output.log(`${STEPS[step]} ${step.padEnd(7)} ${detail}`);
  for (const note of notes) output.log(`${NOTE_INDENT}${note}`);
}

// --out wins, then the application name, then the exporter's directory-name default.
function buildOutfile(options) {
  if (options.outfile !== undefined) return options.outfile;
  return options.name === undefined ? undefined : resolve(process.cwd(), options.name);
}

async function applyConfiguration(options, output) {
  const { path, root, config } = await loadConfig();
  if (!config) {
    // A directory of static output is already an application: there is no build
    // command to configure, and `blitsen` opens this same directory with no
    // argument. Only when there is nothing here to build does the config matter,
    // and then the message is about the config rather than about the entrypoint
    // — a bundler project whose config is missing must not quietly export its
    // source directory instead.
    const here = join(process.cwd(), "index.html");
    if (await access(here, constants.R_OK).then(() => true, () => false)) {
      options.directory = process.cwd();
      return;
    }
    const location = path ?? join(process.cwd(), "package.json");
    throw new Error("missing application directory: pass one, or add an index.html here, "
      + `or add a "blitsen" config to ${location}`);
  }
  if (config.build) {
    reportStep(output, { step: "build", detail: `${config.build} (configured in ${path})` });
    await runBuildCommand(config.build, root);
  }
  options.directory = resolve(root, config.output);
  options.addons = [...config.addons?.map(addon => resolve(root, addon)) ?? [], ...options.addons ?? []];
  options.name ??= config.name;
  applyName(options);
}

export async function resolveApplication(directory) {
  const root = await realpath(resolve(directory)).catch(() => {
    throw new Error(`application directory does not exist: ${directory}`);
  });
  const entrypoint = join(root, "index.html");
  await access(entrypoint, constants.R_OK).catch(() => {
    throw new Error(`missing or unreadable entrypoint: ${entrypoint}`);
  });
  return { root, entrypoint };
}

export function createReloadCoordinator(runtime, output = console, debounceMs = 100) {
  let pending = new Set();
  let timer = null;
  let closed = false;
  let reloads = Promise.resolve();
  const flush = () => {
    timer = null;
    const changed = pending;
    pending = new Set();
    if (changed.size === 0 || closed) return;
    reloads = reloads.then(async () => {
      if ([...changed].every(file => extname(file).toLowerCase() === ".css")) {
        // reloadCSS reports false when no <link rel=stylesheet> resolves to the
        // file: an @import target, or a sheet added since the document loaded.
        // Those still affect the render, so fall back to a document reload.
        let swapped = 0;
        for (const file of changed) swapped += await runtime.reloadCSS(file) ? 1 : 0;
        if (swapped === 0) await runtime.reloadDirectory();
      } else {
        await runtime.reloadDirectory();
      }
    }).catch(error => output.error(`blitsen: reload failed: ${error.message}`));
  };
  return {
    notify(file) {
      if (closed || !file) return;
      pending.add(normalize(String(file)));
      if (timer !== null) clearTimeout(timer);
      timer = setTimeout(flush, debounceMs);
    },
    close() {
      closed = true;
      pending.clear();
      if (timer !== null) clearTimeout(timer);
      timer = null;
    },
    settled() { return reloads; },
  };
}

export function watchApplication(root, runtime, output = console, debounceMs = 100) {
  const coordinator = createReloadCoordinator(runtime, output, debounceMs);
  const watcher = watchFs(root, { recursive: true }, (_event, file) => coordinator.notify(file));
  watcher.on("error", error => output.error(`blitsen: watcher failed: ${error.message}`));
  return {
    close() {
      watcher.close();
      coordinator.close();
    },
  };
}

// bin/blitsen.mjs loads whatever addon it can see without resolution work:
// BLITSEN_NATIVE_PATH, or one staged beside the package. Everything else — the
// platform package npm installed, an addon this checkout built — resolves here,
// and naming the platform it wanted is the point of failing here rather than there.
async function hostRuntime() {
  const resolved = await resolveRuntime();
  return { ...openRuntime(resolved), build: options => buildStandalone(options, resolved) };
}

// A build for another target links that target's runtime, so it resolves its own
// rather than reusing the host's — and it is never opened, because it cannot run
// here. `fetch` is on for exactly this path: a cross-target build is the only
// one allowed to reach the network for a runtime it does not have.
async function targetRuntime(target) {
  if (target === undefined || target === hostTarget()) return hostRuntime();
  const resolved = await resolveRuntime({ target, fetch: true });
  return { build: options => buildStandalone(options, resolved) };
}

export async function main(args, output = console, runtime = null) {
  try {
    let active = runtime;
    const options = parseArgs(args);
    if (options.help) {
      output.log(HELP);
      return 0;
    }
    if (options.version) {
      output.log(await packageVersion());
      return 0;
    }
    if (options.command === "build") {
      // Checked before anything runs: the user's build command must not be spent
      // on an export that cannot link. For a cross-target build that includes
      // fetching the target's runtime, so a target that cannot be built for
      // fails before the build command rather than after it.
      active ??= await targetRuntime(options.target);
      if (!active?.build) {
        throw new Error("native build runtime is unavailable; reinstall blitsen for this platform");
      }
      if (options.directory === null) await applyConfiguration(options, output);
    }
    const application = await resolveApplication(options.directory);
    if (options.command === "doctor") {
      const report = await doctorApplication(application.root);
      if (options.json) output.log(JSON.stringify(report, null, 2));
      else {
        for (const diagnostic of report.diagnostics) {
          const writer = diagnostic.severity === "error" ? output.error : output.log;
          writer.call(output, formatDiagnostic(diagnostic));
        }
        output.log(`Doctor scanned ${report.files} files: ${report.errors} errors, ${report.warnings} warnings.`);
      }
      return report.errors === 0 ? 0 : 1;
    }
    if (options.command === "build") {
      reportStep(output, { step: "ingest", detail: application.entrypoint });
      const report = await doctorApplication(application.root);
      reportStep(output, {
        step: "scan",
        detail: `${report.files} files, ${report.errors} errors, ${report.warnings} warnings`,
        notes: report.diagnostics.filter(item => item.severity !== "error").map(formatDiagnostic),
      });
      // Errors go to stderr so a CI log shows the blocking file without the noise.
      for (const diagnostic of report.diagnostics.filter(item => item.severity === "error")) {
        output.error(`${NOTE_INDENT}${formatDiagnostic(diagnostic)}`);
      }
      if (report.errors > 0 && !options.acceptErrors) {
        throw new Error(`${report.errors} compatibility `
          + `${report.errors === 1 ? "error blocks" : "errors block"} this build; `
          + "run 'blitsen doctor' for the full report, "
          + "or --accept-errors to export anyway with the reported behaviour missing");
      }
      // Steps ③–⑤ report themselves as they run: only the exporter knows when
      // each one finished, and a long link should not look like a hang.
      const result = await active.build({
        ...application,
        ...options,
        outfile: buildOutfile(options),
        progress: event => reportStep(output, event),
      });
      output.log(`Built ${result.outfile} (${result.assets} assets, ${result.bytes} bytes)`);
      // Issue #73: the export records the runtime it linked, so the line that
      // announces the artifact names it too.
      if (result.runtime) output.log(`Runtime: ${describeRuntime(result.runtime)}`);
      if (result.assetDirectory) output.log(`Side-loaded assets: ${result.assetDirectory}`);
      // Not "Phase 1 exports": a Phase 2 export printed the same line and named
      // the wrong host. What is true of both is the part that matters — the
      // redistribution gate in LICENSING.md is not implemented (#121).
      output.log("Exports are architecture proofs and are not yet cleared for redistribution.");
      return 0;
    }
    active ??= await hostRuntime();
    if (!active?.openDirectory) {
      throw new Error("native addon is unavailable; reinstall blitsen for this platform");
    }
    await active.openDirectory({ ...application, ...options });
    const watcher = active.reloadCSS && active.reloadDirectory
      ? watchApplication(application.root, active, output)
      : null;
    try {
      if (active.pumpWindow) {
        const frameInterval = 1000 / 60;
        let nextFrame = performance.now();
        while (active.pumpWindow()) {
          nextFrame += frameInterval;
          const now = performance.now();
          if (nextFrame < now - frameInterval) nextFrame = now;
          const delay = Math.max(0, nextFrame - now);
          await (active.waitForNextFrame?.(delay)
            ?? new Promise(resolve => setTimeout(resolve, delay)));
        }
      }
    } finally {
      watcher?.close();
    }
    return 0;
  } catch (error) {
    output.error(`blitsen: ${error.message}`);
    return 1;
  }
}
