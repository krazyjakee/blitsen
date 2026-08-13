// Issue #89: measure the Phase 2 bare app and justify every megabyte.
//
// The Phase 1 gate in `run-size-gate.mjs` guards a tracked baseline against
// regression. This is the other question: where does the Phase 2 export's size
// actually go, and what does Phase 3 have left to take out of it? It builds the
// same bare application on both hosts, breaks the result into parts, and reports
// what each Phase 3 lever is worth as a measurement rather than an estimate.
//
//     bun run --cwd packages/blitsen size:phase2 [--out measurements.json]
import { strict as assert } from "node:assert";
import { execFile } from "node:child_process";
import { cp, mkdtemp, mkdir, rm, stat, writeFile } from "node:fs/promises";
import { gzipSync } from "node:zlib";
import { readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";

import { buildAddon, repository } from "./build-addon.mjs";
import { buildStandalone } from "../src/export.mjs";
import { resolvePhase2Runtime } from "../src/runtime.mjs";

const run = promisify(execFile);
const outIndex = process.argv.indexOf("--out");
const outFile = outIndex === -1 ? null : process.argv[outIndex + 1];

const bytes = async path => (await stat(path)).size;
const gzipped = async path => gzipSync(await readFile(path), { level: 9 }).length;
const mb = value => `${(value / 1e6).toFixed(1)} MB`;

// The bare app P1 is written against: an HTML file that renders and nothing
// else. Anything larger measures the application rather than the runtime.
const BARE_APP = `<!doctype html><html><head><meta charset="utf-8"><title>Bare</title>
<style>html,body{margin:0;height:100%}body{display:grid;place-items:center;background:#101820;color:#f5f7fa;font:16px sans-serif}</style>
</head><body><main id="app">bare</main>
<script>document.querySelector("#app").textContent = "ready";</script>
</body></html>
`;

async function bareApp() {
  const directory = await mkdtemp(join(tmpdir(), "blitsen-phase2-size-"));
  await mkdir(join(directory, "dist"), { recursive: true });
  await writeFile(join(directory, "dist/index.html"), BARE_APP);
  return directory;
}

async function exportWith(host, directory, addon) {
  const outfile = join(directory, `bare-${host}`);
  const previous = process.env.BLITSEN_HOST;
  process.env.BLITSEN_HOST = host;
  try {
    const result = await buildStandalone({
      root: join(directory, "dist"),
      width: 800,
      height: 600,
      title: "Bare",
      outfile,
    }, addon);
    return { ...result, bytes: await bytes(result.outfile), gzip: await gzipped(result.outfile) };
  } finally {
    if (previous === undefined) delete process.env.BLITSEN_HOST;
    else process.env.BLITSEN_HOST = previous;
  }
}

// What the Phase 2 executable is made of, before an application is appended.
// The engine is deliberately absent from this list: production loads a
// replaceable JavaScriptCore shared library (LICENSING.md), so its size is
// beside the executable rather than inside it, and reporting a total that
// quietly omits it would overstate the result.
async function componentBreakdown(runtimePath) {
  const stripped = `${runtimePath}.stripped`;
  await cp(runtimePath, stripped);
  let strippedBytes = null;
  try {
    await run("strip", [stripped]);
    strippedBytes = await bytes(stripped);
  } catch {
    // No `strip` on this machine; the measurement is simply unavailable.
  } finally {
    await rm(stripped, { force: true });
  }

  let sections = null;
  try {
    const { stdout } = await run("size", ["-A", runtimePath]);
    sections = Object.fromEntries(stdout.split("\n")
      .map(line => line.trim().split(/\s+/))
      .filter(parts => parts.length >= 2 && parts[0].startsWith("."))
      .map(parts => [parts[0], Number(parts[1])])
      .filter(([, size]) => Number.isFinite(size)));
  } catch {
    // `binutils` is not everywhere; the totals above stand without it.
  }
  return { bytes: await bytes(runtimePath), stripped: strippedBytes, sections };
}

// The engine an exported application has to carry. On a development machine
// this is the system library the loader found; a release carries Blitsen's
// pinned build, which is a different and larger artifact (docs/JSC.md).
async function engineLibrary() {
  const runtime = await resolvePhase2Runtime();
  const { stdout } = await run(runtime.path, ["--engine-report"]);
  const report = JSON.parse(stdout);
  if (!report.loaded) return { loaded: false, error: report.error };
  const candidates = [
    process.env.BLITSEN_JSC_LIBRARY,
    "/lib/x86_64-linux-gnu/libjavascriptcoregtk-6.0.so.1",
    "/usr/lib/x86_64-linux-gnu/libjavascriptcoregtk-6.0.so.1",
  ].filter(Boolean);
  for (const path of candidates) {
    const size = await stat(path).then(entry => entry.size, () => null);
    if (size !== null) return { loaded: true, path, bytes: size, modules: report.modules };
  }
  return { loaded: true, path: null, bytes: null, modules: report.modules };
}

const directory = await bareApp();
try {
  const addon = await buildAddon({ purpose: "Phase 2 size", release: true });
  const runtime = await resolvePhase2Runtime();
  const phase1 = await exportWith("bun", directory, addon);
  const phase2 = await exportWith("jsc", directory, addon);
  assert.ok(phase2.bytes < phase1.bytes, "the Phase 2 export is not smaller");

  const minimised = join(repository, "target/release-min/blitsen-runtime");
  const breakdown = await componentBreakdown(runtime.path);
  const engine = await engineLibrary();
  const payload = phase2.bytes - breakdown.bytes;

  const measurements = {
    platform: `${process.platform}-${process.arch}`,
    application: "bare",
    phase1: { bytes: phase1.bytes, gzip: phase1.gzip, assets: phase1.assets },
    phase2: { bytes: phase2.bytes, gzip: phase2.gzip, assets: phase2.assets },
    ratio: Number((phase1.bytes / phase2.bytes).toFixed(2)),
    components: {
      runtimeExecutable: breakdown.bytes,
      runtimeExecutableStripped: breakdown.stripped,
      appPayload: payload,
      engineLibrary: engine.bytes,
      engineLibraryPath: engine.path,
    },
    sections: breakdown.sections,
    // What Phase 3 has left, measured rather than estimated. `release-min` is
    // the workspace profile that turns on fat LTO, one codegen unit, size-first
    // optimisation and symbol stripping together; build it with
    // `cargo build --profile release-min -p blitsen-runtime` and this reports
    // what it was worth. Panic strategy is deliberately not among the levers:
    // the native callback boundary turns a panic into a JavaScript exception
    // with `catch_unwind`, and `panic = "abort"` would take the process down.
    phase3: {
      strippingSaves: breakdown.stripped === null ? null : breakdown.bytes - breakdown.stripped,
      releaseMin: await bytes(minimised).catch(() => null),
    },
  };

  console.log(`Phase 2 bare app on ${measurements.platform}`);
  console.log(`  Phase 1 (Bun)            ${mb(phase1.bytes)}  (gzip ${mb(phase1.gzip)})`);
  console.log(`  Phase 2 (embedded JSC)   ${mb(phase2.bytes)}  (gzip ${mb(phase2.gzip)})`);
  console.log(`  ratio                    ${measurements.ratio}x smaller`);
  console.log("  components");
  console.log(`    runtime executable     ${mb(breakdown.bytes)}`);
  if (breakdown.stripped !== null) {
    console.log(`      stripped             ${mb(breakdown.stripped)}  `
      + `(${mb(breakdown.bytes - breakdown.stripped)} of debug symbols)`);
  }
  console.log(`    appended application   ${mb(payload)}`);
  if (engine.bytes !== null) {
    console.log(`    JavaScriptCore         ${mb(engine.bytes)}  `
      + `(${engine.path}; loaded dynamically, so beside the binary and not in it)`);
    console.log(`    shipped total          ${mb(phase2.bytes + engine.bytes)}  `
      + "executable + engine library");
  } else {
    console.log("    JavaScriptCore         unmeasured on this machine");
  }
  if (measurements.phase3.releaseMin !== null) {
    const saved = breakdown.bytes - measurements.phase3.releaseMin;
    console.log(`  phase 3 (release-min)    ${mb(measurements.phase3.releaseMin)}  `
      + `(${mb(saved)} off the runtime executable: fat LTO, one codegen unit, `
      + "size-first optimisation and stripped symbols)");
  }
  if (breakdown.sections) {
    const largest = Object.entries(breakdown.sections)
      .sort(([, left], [, right]) => right - left)
      .slice(0, 5);
    console.log(`  largest sections         ${largest
      .map(([name, size]) => `${name} ${mb(size)}`).join(", ")}`);
  }

  if (outFile) {
    await writeFile(outFile, `${JSON.stringify(measurements, null, 2)}\n`);
    console.log(`  written to               ${outFile}`);
  }
} finally {
  await rm(directory, { recursive: true, force: true });
}
