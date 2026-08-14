// Issue #90: the npm surface is unchanged across the Phase 1 → Phase 2 host swap.
//
// Structural constraint 7 says users must experience that migration as a smaller
// binary and nothing else. This runs one project through the CLI twice — once
// linking into Bun, once into Blitsen's own runtime — and diffs what a user can
// observe: the CLI's own output, how the config was handled, the artifact layout
// beside the executable, and what the exported application prints when it checks
// itself. The only difference it allows is size, and it reports that.
//
//     bun run --cwd packages/blitsen test:hosts
import { strict as assert } from "node:assert";
import { cp, mkdir, mkdtemp, readFile, readdir, realpath, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, join, relative } from "node:path";

import { buildAddon, repository } from "./build-addon.mjs";
import { resolvePhase2Runtime } from "../src/runtime.mjs";

const CLI = join(repository, "packages/blitsen/bin/blitsen.mjs");
const HOSTS = [
  { host: "bun", label: "Phase 1 (Bun)" },
  { host: "blitsen", label: "Blitsen runtime (QuickJS-ng)" },
];

// Everything in the CLI's output that is allowed to differ between two runs of
// the same build: the directory it happened in, and how many bytes came out.
function normalise(text, directory) {
  return text
    .replaceAll(directory, "<build>")
    .replace(/\b\d+(\.\d+)?\s?(B|KB|MB|GB)\b/g, "<size>")
    .replace(/\b\d{4,}\b/g, "<number>")
    .trim();
}

async function layout(directory) {
  const entries = [];
  const walk = async at => {
    for (const entry of await readdir(at, { withFileTypes: true })) {
      const path = join(at, entry.name);
      if (entry.isDirectory()) await walk(path);
      else entries.push(relative(directory, path).split(/[\\/]/).join("/"));
    }
  };
  await walk(directory);
  return entries.sort();
}

// The checkout has no installed platform package, so the addon this build made
// stands in for one — the same way every other acceptance script drives the CLI.
const addon = await buildAddon({ purpose: "host conformance", release: true });

function cli(directory, args, env = {}) {
  const result = Bun.spawnSync({
    cmd: [process.execPath, CLI, ...args],
    cwd: directory,
    env: { ...process.env, BLITSEN_NATIVE_PATH: addon, ...env },
    stdout: "pipe",
    stderr: "pipe",
  });
  return {
    code: result.exitCode,
    stdout: result.stdout.toString(),
    stderr: result.stderr.toString(),
  };
}

// A module application, which the Pong fixture is not: its scripts are classic,
// so nothing here exercised a `<script type="module">` while issue #126 was open
// — and that is the shape where the two hosts could most easily disagree. What
// it prints is the whole question: the identifier a module runs under has to be
// an absolute URL, an asset resolved against it has to name the file the
// application shipped, and reading that file has to work.
//
// Written into the same `dist` the CLI ingests, beside Pong, so it is one build
// and one comparison rather than two.
const MODULE_PROBE = `<script type="module">
  globalThis.__probe = { inline: import.meta.url };
</script>
<script type="module" src="./probe.js"></script>`;

const MODULE_PROBE_SCRIPT = `globalThis.__probe.external = import.meta.url;
globalThis.__probe.asset = new URL("./probe.json", import.meta.url).href;
globalThis.__probe.read = "pending";
fetch(globalThis.__probe.asset)
  .then(response => response.json())
  .then(value => { globalThis.__probe.read = value.shipped; },
    error => { globalThis.__probe.read = "failed: " + error.message; });
`;

// What the application saw, reduced to the properties that have to hold on both
// hosts. The URLs themselves are *not* compared, because they are allowed to
// differ: the two hosts address the same application on different origins —
// `file:` where Bun's loader is the filesystem's, `blitsen://app/` inside an
// executable that has no filesystem — and what issue #126 is about is that both
// are absolute URLs, that an asset resolves to a sibling of the module that
// named it, and that reading it works. They are printed on their own line,
// which the comparison drops and the assertions below read.
const MODULE_PROBE_ASSERT = `(() => {
  const probe = globalThis.__probe;
  const sibling = new URL("./probe.json", probe.external).href;
  console.log("probe " + JSON.stringify({
    inline: URL.canParse(probe.inline) && new URL(probe.inline).pathname.endsWith("/index.html"),
    external: URL.canParse(probe.external) && new URL(probe.external).pathname.endsWith("/probe.js"),
    asset: probe.asset === sibling,
    read: probe.read,
  }));
  console.log("probe-detail " + JSON.stringify(probe));
})()`;

// The engine-level globals a bare context does not bring with it. Phase 1
// borrows Bun's; Phase 2 installs its own (`runtime_services/bootstrap.js`), and
// for a while it installed neither — which is how an export of the Monaco
// example came out a white window while the same application ran in development,
// since Monaco calls all of these on its way to the first frame. So both what
// the two hosts do here and whether they agree about it are asserted.
//
// Only booleans are printed. The values themselves are random by construction,
// and the comparison is over the output.
const PLATFORM_PROBE_ASSERT = `(() => {
  const filled = new Uint8Array(32);
  const returned = crypto.getRandomValues(filled);
  const text = "hello — ✓ 𝄞";
  const encoded = new TextEncoder().encode(text);
  const half = encoded.length >> 1;
  const streaming = new TextDecoder();
  // Split inside a multi-byte sequence on purpose: a decoder that keeps no
  // state between chunks answers this with two replacement characters.
  const streamed = streaming.decode(encoded.subarray(0, half), { stream: true })
    + streaming.decode(encoded.subarray(half));
  // What Monaco's string builder does to turn its buffer into a line, with no
  // feature test in front of it.
  const units = Uint16Array.from({ length: text.length }, (_, index) => text.charCodeAt(index));
  console.log("platform " + JSON.stringify({
    randomFilled: returned === filled && filled.some(byte => byte !== 0),
    randomUUID: /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/
      .test(crypto.randomUUID()),
    encoding: new TextEncoder().encoding + "/" + new TextDecoder().encoding,
    utf8: new TextDecoder().decode(encoded) === text,
    utf8Streamed: streamed === text,
    utf16: new TextDecoder("UTF-16LE").decode(units) === text,
  }));
})()`;

async function buildWith({ host }) {
  // Realpathed, because everything below compares two hosts' output *as text*
  // and `normalise` only replaces the directory it is given. The temporary
  // directory is spelled one way by `tmpdir()` and another by the CLI, which
  // reports resolved paths: `C:\Users\RUNNER~1\…` against
  // `C:\Users\runneradmin\…` on Windows, `/var` against `/private/var` on
  // macOS. Neither host's path was replaced, so the two differed by the only
  // thing they are allowed to differ by (#123).
  const directory = await realpath(await mkdtemp(join(tmpdir(), `blitsen-hosts-${host}-`)));
  await cp(join(repository, "examples/pong"), join(directory, "dist"), { recursive: true });
  const entrypoint = join(directory, "dist", "index.html");
  await writeFile(entrypoint,
    (await readFile(entrypoint, "utf8")).replace("</body>", `${MODULE_PROBE}\n</body>`));
  await writeFile(join(directory, "dist", "probe.js"), MODULE_PROBE_SCRIPT);
  await writeFile(join(directory, "dist", "probe.json"), `{"shipped":"read"}\n`);
  // One config, used by both builds and never edited between them: how the CLI
  // reads it is part of what is being compared.
  await writeFile(join(directory, "package.json"), `${JSON.stringify({
    name: "host-conformance",
    private: true,
    blitsen: { output: "dist", name: "MyApp" },
  }, null, 2)}\n`);

  const build = cli(directory, ["build", "--width", "720", "--height", "520"],
    { BLITSEN_HOST: host });
  assert.equal(build.code, 0, `${host} build failed:\n${build.stdout}\n${build.stderr}`);

  const executable = join(directory, process.platform === "win32" ? "MyApp.exe" : "MyApp");
  const check = Bun.spawnSync({
    cmd: [executable],
    cwd: directory,
    env: {
      ...process.env,
      BLITSEN_STANDALONE_CHECK: "1",
      // Long enough for the probe's own fetch of a shipped file to land on a
      // frame turn; the check settles twice around the assertion.
      BLITSEN_STANDALONE_CHECK_DELAY: "250",
      BLITSEN_STANDALONE_CHECK_ASSERT: `${MODULE_PROBE_ASSERT};\n${PLATFORM_PROBE_ASSERT}`,
    },
    stdout: "pipe",
    stderr: "pipe",
  });
  assert.equal(check.exitCode, 0,
    `${host} standalone check failed:\n${check.stdout}\n${check.stderr}`);

  // Inputs the CLI must refuse identically, whichever host is behind it: a
  // value it will not accept, and an output that already exists.
  const rejected = cli(directory, ["build", "--assets", "everywhere"], { BLITSEN_HOST: host });
  const existing = cli(directory, ["build"], { BLITSEN_HOST: host });

  // Issue #127: a source tree, which one host used to render and the other
  // refuses. `doctor` grades it an error, so the export needs `--accept-errors`
  // to exist at all — which is exactly what makes the runtime's own refusal
  // observable, and comparable between the two.
  const sourceTree = join(directory, "src-tree");
  await mkdir(join(sourceTree, "src"), { recursive: true });
  await writeFile(join(sourceTree, "index.html"),
    `<!doctype html><html><body><div id="root"></div>\n`
    + `<script type="module" src="/src/main.jsx"></script>\n</body></html>\n`);
  await writeFile(join(sourceTree, "src", "main.jsx"),
    `import React from "react";\nexport default React;\n`);
  const unbuilt = cli(directory, ["build", "src-tree", "--out", "Unbuilt"], { BLITSEN_HOST: host });
  const unbuiltForced = cli(directory,
    ["build", "src-tree", "--out", "Unbuilt", "--accept-errors"], { BLITSEN_HOST: host });
  assert.equal(unbuiltForced.code, 0,
    `${host} could not export the source tree even with --accept-errors:\n${unbuiltForced.stderr}`);
  const unbuiltRun = Bun.spawnSync({
    cmd: [join(directory, process.platform === "win32" ? "Unbuilt.exe" : "Unbuilt")],
    cwd: directory,
    env: { ...process.env, BLITSEN_STANDALONE_CHECK: "1" },
    stdout: "pipe",
    stderr: "pipe",
  });

  return {
    host,
    directory,
    bytes: (await stat(executable)).size,
    // The redistribution line is the one thing in the build output that is
    // allowed to differ, and it differs because the two hosts differ in what
    // they can truthfully claim: a Phase 2 export carries the notices it owes
    // (#121), and a Phase 1 export carries a copy of Bun whose own LGPL notice
    // flow is not automated here. Compared per host below rather than dropped.
    buildOutput: normalise(build.stdout, directory)
      .split("\n")
      .filter(line => !line.startsWith("Third-party notices:") && !line.startsWith("This export is not cleared"))
      .join("\n"),
    redistribution: build.stdout.split("\n")
      .find(line => line.startsWith("Third-party notices:") || line.startsWith("This export is not cleared"))
      ?? "",
    // The runtime line names the host on purpose; it is the one line that is
    // allowed to differ, and it is dropped before the rest is compared.
    checkOutput: check.stdout.toString()
      .split("\n")
      .filter(line => !line.startsWith("Blitsen runtime:") && !line.startsWith("probe-detail "))
      .join("\n")
      .trim(),
    probeDetail: check.stdout.toString()
      .split("\n")
      .find(line => line.startsWith("probe-detail ")) ?? "",
    unbuiltCode: unbuilt.code,
    unbuiltError: normalise(unbuilt.stderr, directory),
    unbuiltRunCode: unbuiltRun.exitCode,
    unbuiltRunError: normalise(unbuiltRun.stderr.toString(), directory),
    rejectedCode: rejected.code,
    rejectedError: normalise(rejected.stderr, directory),
    existingCode: existing.code,
    existingError: normalise(existing.stderr, directory),
    layout: (await layout(directory)).filter(path => path !== "MyApp" && path !== "MyApp.exe"),
  };
}

/**
 * The frame-determinism goldens, replayed on the Phase 2 host.
 *
 * "Golden-image corpus passes identically" is the sharpest claim in issue #90,
 * and the committed digests are the sharpest way to check it: the same trace at
 * the same fixed timestep, compared frame by frame with what Phase 1 recorded.
 * The DOM stream holds only what the application wrote and is compared always;
 * layout and pixels depend on this machine's fonts and rasterizer, so they are
 * compared only when its fingerprint matches the golden's — the same two tiers
 * `run-determinism.mjs` uses.
 */
async function compareGoldens() {
  // A golden is a recording, and only the platform it was recorded on has one.
  // Rather than skip the sharpest check in #90 everywhere else, fall back to the
  // reference recording and let the fingerprint tier below do its job: the DOM
  // stream holds only what the application wrote, so it is comparable from any
  // host, while layout and pixels are already gated on a fingerprint that a
  // different machine will not match. Falling back is what makes "identical DOM
  // digests" a cross-platform claim instead of a Linux one.
  const own = join(import.meta.dir, `replay/pong-${process.platform}-${process.arch}.golden.json`);
  const reference = join(import.meta.dir, "replay/pong-linux-x64.golden.json");
  const goldenPath = await Bun.file(own).exists() ? own : reference;
  const golden = JSON.parse(await readFile(goldenPath, "utf8"));
  const tracePath = join(import.meta.dir, "replay/pong.trace.json");
  const trace = JSON.parse(await readFile(tracePath, "utf8"));
  const runtime = await resolvePhase2Runtime();
  const run = Bun.spawnSync({
    cmd: [runtime.path, "--replay", join(repository, trace.application, "index.html"), tracePath],
    cwd: repository,
    stdout: "pipe",
    stderr: "pipe",
  });
  if (run.exitCode !== 0) {
    process.stderr.write(run.stderr.toString());
    throw new Error(`Phase 2 replay exited ${run.exitCode}`);
  }
  const report = JSON.parse(run.stdout.toString());
  const portable = report.fingerprint === golden.fingerprint;
  const streams = portable ? ["dom", "layout", "pixels"] : ["dom"];
  for (const stream of streams) {
    const diverged = golden[stream]
      .map((digest, index) => (digest === report[stream][index] ? null : index + 1))
      .filter(frame => frame !== null);
    assert.deepEqual(diverged, [],
      `the Phase 2 host produced different ${stream} digests at frames ${diverged.join(", ")}`);
  }
  return { streams, frames: report.frames, portable, golden: basename(goldenPath) };
}

const results = [];
try {
  for (const target of HOSTS) results.push(await buildWith(target));
  const [phase1, phase2] = results;

  assert.equal(phase2.buildOutput, phase1.buildOutput,
    "the CLI reported a different build on the two hosts");
  assert.equal(phase2.checkOutput, phase1.checkOutput,
    "the exported application reported differently on the two hosts");
  assert.equal(phase2.rejectedCode, phase1.rejectedCode,
    "the CLI accepted or refused the same config differently");
  assert.equal(phase2.rejectedError, phase1.rejectedError,
    "the CLI explained the same refusal differently");
  assert.equal(phase2.existingCode, phase1.existingCode,
    "the CLI treated an existing output differently");
  assert.equal(phase2.existingError, phase1.existingError,
    "the CLI explained an existing output differently");
  assert.deepEqual(phase2.layout, phase1.layout,
    "the two hosts produced a different artifact layout");
  assert.match(phase2.redistribution, /^Third-party notices: embedded/,
    `the Phase 2 export did not carry its notices: ${phase2.redistribution}`);
  assert.match(phase1.redistribution, /not cleared for redistribution/,
    `the Phase 1 export claimed notices it does not carry: ${phase1.redistribution}`);
  assert.equal(phase2.unbuiltCode, phase1.unbuiltCode,
    "the CLI accepted or refused a source tree differently");
  assert.equal(phase2.unbuiltError, phase1.unbuiltError,
    "the CLI explained a source tree differently");
  assert.equal(phase2.unbuiltRunCode, phase1.unbuiltRunCode,
    "one host ran an exported source tree and the other refused it");
  assert.equal(phase2.unbuiltRunError, phase1.unbuiltRunError,
    "the two hosts explained an exported source tree differently");
  for (const result of results) {
    assert.notEqual(result.unbuiltCode, 0, `${result.host} exported a source tree without --accept-errors`);
    assert.match(result.unbuiltError, /HTML_SOURCE_ENTRY/,
      `${result.host} did not name the source-tree entrypoint: ${result.unbuiltError}`);
    assert.notEqual(result.unbuiltRunCode, 0,
      `${result.host} ran an application that loads source`);
    assert.match(result.unbuiltRunError, /vite build/,
      `${result.host} refused a source tree without naming the build: ${result.unbuiltRunError}`);
  }

  // Issue #126: identical is not enough on its own — both hosts agreeing that a
  // module is named by a bare path would pass the comparison above and still
  // break `new URL(…, import.meta.url)`. So each property is asserted, not just
  // the agreement.
  for (const result of results) {
    const line = result.checkOutput.split("\n").find(text => text.startsWith("probe "));
    assert.ok(line, `${result.host} printed no module probe:\n${result.checkOutput}`);
    const probe = JSON.parse(line.slice("probe ".length));
    const detail = result.probeDetail;
    assert.equal(probe.inline, true,
      `${result.host} named an inline module something other than the document: ${detail}`);
    assert.equal(probe.external, true,
      `${result.host} named an external module something other than its own URL: ${detail}`);
    assert.equal(probe.asset, true,
      `${result.host} resolved an asset against the module to somewhere else: ${detail}`);
    assert.equal(probe.read, "read",
      `${result.host} could not read a file the application shipped: ${detail}`);
  }

  // The same reasoning for the platform globals: two hosts that both lacked
  // `crypto` would agree, and the application would still not start.
  for (const result of results) {
    const line = result.checkOutput.split("\n").find(text => text.startsWith("platform "));
    assert.ok(line, `${result.host} printed no platform probe:\n${result.checkOutput}`);
    const platform = JSON.parse(line.slice("platform ".length));
    assert.equal(platform.randomFilled, true,
      `${result.host} did not fill the array crypto.getRandomValues was given`);
    assert.equal(platform.randomUUID, true,
      `${result.host} did not produce a version 4 UUID`);
    assert.equal(platform.encoding, "utf-8/utf-8",
      `${result.host} named its default text encoding ${platform.encoding}`);
    assert.equal(platform.utf8, true, `${result.host} did not round-trip UTF-8`);
    assert.equal(platform.utf8Streamed, true,
      `${result.host} lost a character across a streaming UTF-8 decode`);
    assert.equal(platform.utf16, true, `${result.host} did not decode UTF-16LE`);
  }

  const saved = phase1.bytes - phase2.bytes;
  const ratio = (phase1.bytes / phase2.bytes).toFixed(2);
  assert.ok(saved > 0, `the Phase 2 export is not smaller: ${phase2.bytes} vs ${phase1.bytes}`);
  const goldens = await compareGoldens();
  console.log(`Host conformance passed: identical CLI output, config handling, artifact layout `
    + `and standalone check.`);
  console.log(`  Goldens: ${goldens.frames} replayed frames against ${goldens.golden}, `
    + `${goldens.streams.join("/")} digests identical to the Phase 1 recording`
    + `${goldens.portable ? "" : " (layout and pixels not comparable on this rasterizer)"}.`);
  console.log(`  ${HOSTS[0].label}: ${phase1.bytes.toLocaleString()} bytes`);
  console.log(`  ${HOSTS[1].label}: ${phase2.bytes.toLocaleString()} bytes `
    + `(${ratio}× smaller, ${saved.toLocaleString()} bytes saved)`);
} finally {
  for (const result of results) await rm(result.directory, { recursive: true, force: true });
}
