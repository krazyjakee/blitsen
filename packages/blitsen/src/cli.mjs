import { access, readFile, realpath } from "node:fs/promises";
import { constants, watch as watchFs } from "node:fs";
import { extname, join, normalize, resolve } from "node:path";
import { doctorApplication, formatDiagnostic } from "./doctor.mjs";

const HELP = `Usage: blitsen <directory> [options]
       blitsen build <directory> [options]
       blitsen doctor <directory> [--json]

Open <directory>/index.html in a native Blitsen window.
Build creates a Phase 1 single-file executable for the current platform.
Doctor checks built static output against the v0 compatibility profile.

Options:
  --width <pixels>   Initial logical width (default: 800)
  --height <pixels>  Initial logical height (default: 600)
  --title <text>     Native window title (default: Blitsen)
  --outfile <path>   Build output path (default: application directory name)
  --include <glob>   Keep an unreferenced output file (repeatable)
  --assets <layout>  embedded (default) or side-loaded next to the executable
  --force            Replace an existing build output
  --json             Emit the doctor report as JSON
  -h, --help         Show help
  -v, --version      Show version`;

// Single source of truth: the published package manifest, not a literal.
export async function packageVersion() {
  const manifest = new URL("../package.json", import.meta.url);
  return JSON.parse(await readFile(manifest, "utf8")).version;
}

const VALUE_OPTIONS = ["--width", "--height", "--title", "--outfile", "--include", "--assets"];
const BUILD_OPTIONS = ["--outfile", "--include", "--assets"];

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
      if (argument === "--title") options.title = value;
      else if (argument === "--outfile") options.outfile = value;
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
  if (options.directory === null) throw new Error("missing application directory");
  return options;
}

function summarize(paths, limit = 5) {
  const shown = paths.slice(0, limit).join(", ");
  return paths.length > limit ? `${shown}, and ${paths.length - limit} more` : shown;
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
      if (!runtime?.build) {
        throw new Error("native build runtime is unavailable; reinstall blitsen for this platform");
      }
      const report = await doctorApplication(application.root);
      for (const diagnostic of report.diagnostics) {
        output.error(`blitsen: build compatibility ${diagnostic.severity}: ${formatDiagnostic(diagnostic)}`);
      }
      if (report.errors > 0) {
        throw new Error(`${report.errors} compatibility `
          + `${report.errors === 1 ? "error blocks" : "errors block"} this build; `
          + "run 'blitsen doctor' for the full report");
      }
      const result = await runtime.build({ ...application, ...options });
      if (result.unreferenced?.length) {
        output.log(`Dropped ${result.unreferenced.length} files unreachable from index.html `
          + `(--include <glob> keeps them): ${summarize(result.unreferenced)}`);
      }
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
