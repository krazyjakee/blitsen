import { access, readFile, realpath } from "node:fs/promises";
import { constants, watch as watchFs } from "node:fs";
import { extname, join, normalize, resolve } from "node:path";
import { loadConfig, runBuildCommand } from "./config.mjs";
import { doctorApplication, formatDiagnostic } from "./doctor.mjs";

const HELP = `Usage: blitsen <directory> [options]
       blitsen build [directory] [options]
       blitsen doctor <directory> [--json]

Open <directory>/index.html in a native Blitsen window.
Build creates a Phase 1 single-file executable for the current platform. With no
directory it reads the "blitsen" config in package.json, runs the configured
build command, and ingests its output directory.
Doctor checks built static output against the v0 compatibility profile.

Options:
  --width <pixels>   Initial logical width (default: 800)
  --height <pixels>  Initial logical height (default: 600)
  --title <text>     Native window title (default: the application name)
  --name <text>      Application name: window title and default output name
  --out <path>       Build output path (default: the application name)
  --outfile <path>   Alias of --out
  --target <triple>  Build target; only the host target is supported (see #72)
  --include <glob>   Keep an unreferenced output file (repeatable)
  --assets <layout>  embedded (default) or side-loaded next to the executable
  --icon <path>      Application icon: PNG, or a platform-native .ico/.icns/.svg
  --bundle-id <id>   macOS CFBundleIdentifier (default: com.blitsen.<title>)
  --app-version <v>  Application version recorded in the platform metadata
  --sign <command>   Signing hook, run with the packaged artifact as its argument
  --force            Replace an existing build output
  --json             Emit the doctor report as JSON
  -h, --help         Show help
  -v, --version      Show version`;

// Single source of truth: the published package manifest, not a literal.
export async function packageVersion() {
  const manifest = new URL("../package.json", import.meta.url);
  return JSON.parse(await readFile(manifest, "utf8")).version;
}

const PACKAGE_OPTIONS = { "--icon": "icon", "--bundle-id": "bundleId", "--app-version": "appVersion", "--sign": "sign" };
const BUILD_OPTIONS = ["--out", "--outfile", "--name", "--target", "--include", "--assets",
  ...Object.keys(PACKAGE_OPTIONS)];
const VALUE_OPTIONS = ["--width", "--height", "--title", ...BUILD_OPTIONS];
// TECH.md §11: one binary package per target. Phase 1 links the host's addon, so
// every other target is refused rather than silently built for the host.
const TARGETS = ["darwin-arm64", "darwin-x64", "linux-arm64", "linux-x64", "win32-arm64", "win32-x64"];
const hostTarget = () => `${process.platform}-${process.arch}`;

function checkTarget(value) {
  if (!TARGETS.includes(value)) {
    throw new Error(`unknown --target ${value} (expected one of: ${TARGETS.join(", ")})`);
  }
  if (value !== hostTarget()) {
    throw new Error(`--target ${value} is not supported yet: this runtime is compiled for `
      + `${hostTarget()} and there are no per-target runtime packages to link against; `
      + "see issue #72 for cross-target export");
  }
}

export function parseArgs(args) {
  if (args.length === 0 || args.includes("--help") || args.includes("-h")) {
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
  // A build with no directory is the configured one; anything else needs a target
  // directory, because the input to Blitsen is a directory of static web output.
  if (options.directory === null && options.command !== "build") {
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
    const location = path ?? join(process.cwd(), "package.json");
    throw new Error("missing application directory: pass one, "
      + `or add a "blitsen" config to ${location}`);
  }
  if (config.build) {
    reportStep(output, { step: "build", detail: `${config.build} (configured in ${path})` });
    await runBuildCommand(config.build, root);
  }
  options.directory = resolve(root, config.output);
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

export async function main(args, output = console, runtime = null) {
  try {
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
      // on an export that cannot link.
      if (!runtime?.build) {
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
      if (report.errors > 0) {
        throw new Error(`${report.errors} compatibility `
          + `${report.errors === 1 ? "error blocks" : "errors block"} this build; `
          + "run 'blitsen doctor' for the full report");
      }
      // Steps ③–⑤ report themselves as they run: only the exporter knows when
      // each one finished, and a long link should not look like a hang.
      const result = await runtime.build({
        ...application,
        ...options,
        outfile: buildOutfile(options),
        progress: event => reportStep(output, event),
      });
      output.log(`Built ${result.outfile} (${result.assets} assets, ${result.bytes} bytes)`);
      if (result.assetDirectory) output.log(`Side-loaded assets: ${result.assetDirectory}`);
      output.log("Phase 1 exports are architecture proofs and are not yet cleared for redistribution.");
      return 0;
    }
    if (!runtime?.openDirectory) {
      throw new Error("native addon is unavailable; reinstall blitsen for this platform");
    }
    await runtime.openDirectory({ ...application, ...options });
    const watcher = runtime.reloadCSS && runtime.reloadDirectory
      ? watchApplication(application.root, runtime, output)
      : null;
    try {
      if (runtime.pumpWindow) {
        const frameInterval = 1000 / 60;
        let nextFrame = performance.now();
        while (runtime.pumpWindow()) {
          nextFrame += frameInterval;
          const now = performance.now();
          if (nextFrame < now - frameInterval) nextFrame = now;
          const delay = Math.max(0, nextFrame - now);
          await (runtime.waitForNextFrame?.(delay)
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
