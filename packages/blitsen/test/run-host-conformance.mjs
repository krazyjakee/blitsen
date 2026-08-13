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
import { cp, mkdtemp, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, relative } from "node:path";

import { buildAddon, repository } from "./build-addon.mjs";
import { resolvePhase2Runtime } from "../src/runtime.mjs";

const CLI = join(repository, "packages/blitsen/bin/blitsen.mjs");
const HOSTS = [
  { host: "bun", label: "Phase 1 (Bun)" },
  { host: "jsc", label: "Phase 2 (embedded JSC)" },
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

async function buildWith({ host }) {
  const directory = await mkdtemp(join(tmpdir(), `blitsen-hosts-${host}-`));
  await cp(join(repository, "examples/pong"), join(directory, "dist"), { recursive: true });
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
    env: { ...process.env, BLITSEN_STANDALONE_CHECK: "1" },
    stdout: "pipe",
    stderr: "pipe",
  });
  assert.equal(check.exitCode, 0,
    `${host} standalone check failed:\n${check.stdout}\n${check.stderr}`);

  // Inputs the CLI must refuse identically, whichever host is behind it: a
  // value it will not accept, and an output that already exists.
  const rejected = cli(directory, ["build", "--assets", "everywhere"], { BLITSEN_HOST: host });
  const existing = cli(directory, ["build"], { BLITSEN_HOST: host });

  return {
    host,
    directory,
    bytes: (await stat(executable)).size,
    buildOutput: normalise(build.stdout, directory),
    // The runtime line names the host on purpose; it is the one line that is
    // allowed to differ, and it is dropped before the rest is compared.
    checkOutput: check.stdout.toString()
      .split("\n")
      .filter(line => !line.startsWith("Blitsen runtime:"))
      .join("\n")
      .trim(),
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
  const golden = JSON.parse(await readFile(
    join(import.meta.dir, `replay/pong-${process.platform}-${process.arch}.golden.json`), "utf8"));
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
  return { streams, frames: report.frames, portable };
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

  const saved = phase1.bytes - phase2.bytes;
  const ratio = (phase1.bytes / phase2.bytes).toFixed(2);
  assert.ok(saved > 0, `the Phase 2 export is not smaller: ${phase2.bytes} vs ${phase1.bytes}`);
  const goldens = await compareGoldens();
  console.log(`Host conformance passed: identical CLI output, config handling, artifact layout `
    + `and standalone check.`);
  console.log(`  Goldens: ${goldens.frames} replayed frames, `
    + `${goldens.streams.join("/")} digests identical to the Phase 1 recording`
    + `${goldens.portable ? "" : " (layout and pixels not comparable on this rasterizer)"}.`);
  console.log(`  ${HOSTS[0].label}: ${phase1.bytes.toLocaleString()} bytes`);
  console.log(`  ${HOSTS[1].label}: ${phase2.bytes.toLocaleString()} bytes `
    + `(${ratio}× smaller, ${saved.toLocaleString()} bytes saved)`);
} finally {
  for (const result of results) await rm(result.directory, { recursive: true, force: true });
}
