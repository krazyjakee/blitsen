// Frame determinism gate: replay one recorded trace twice and compare both runs
// against each other and against the committed golden digests.
//
// Two tiers, because not every digest is portable. The DOM digest holds only
// what the application wrote, so it is compared everywhere. Layout and pixel
// digests depend on the host's fonts and rasterizer, so they are compared only
// when this machine's rendering fingerprint matches the one the golden was
// recorded on; everywhere else the two independent runs still have to agree,
// which is what catches real nondeterminism.
//
// usage: bun run-determinism.mjs [--update] [--dump <dir>] [--debug]
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { buildAddon, repository } from "./build-addon.mjs";

const traceFile = join(import.meta.dir, "replay/pong.trace.json");
const update = process.argv.includes("--update");
const debug = process.argv.includes("--debug");
const dumpIndex = process.argv.indexOf("--dump");
const dumpDirectory = dumpIndex === -1
  ? join(repository, "target/determinism-divergence")
  : resolve(process.argv[dumpIndex + 1]);

const goldenFile = join(import.meta.dir, `replay/pong-${process.platform}-${process.arch}.golden.json`);

const addon = await buildAddon({ purpose: "determinism target", release: !debug });

const workspace = await mkdtemp(join(tmpdir(), "blitsen-determinism-"));
const replay = async (name, extra = []) => {
  const report = join(workspace, `${name}.json`);
  const run = Bun.spawnSync({
    cmd: [process.execPath, join(import.meta.dir, "replay-once.mjs"), addon, traceFile, report, ...extra],
    cwd: repository,
    stdout: "pipe",
    stderr: "pipe",
  });
  if (run.exitCode !== 0) {
    process.stderr.write(run.stderr.toString());
    throw new Error(`replay ${name} exited ${run.exitCode}`);
  }
  return JSON.parse(await readFile(report, "utf8"));
};

const streams = ["dom", "layout", "pixels"];
const diverged = (left, right) => {
  const frames = new Set();
  for (const stream of streams) {
    for (let index = 0; index < Math.max(left[stream].length, right[stream].length); index++)
      if (left[stream][index] !== right[stream][index]) frames.add(index + 1);
  }
  return [...frames].sort((a, b) => a - b);
};

const failures = [];
try {
  const first = await replay("run-a");
  const second = await replay("run-b");

  const unstable = diverged(first, second);
  if (unstable.length > 0)
    failures.push(`two runs of the same trace diverged at frames ${unstable.slice(0, 8).join(", ")}`);

  if (update) {
    if (unstable.length > 0) throw new Error("refusing to record an unstable golden");
    const golden = {
      recorded: new Date().toISOString().slice(0, 10),
      platform: process.platform,
      arch: process.arch,
      trace: "pong.trace.json",
      // Layout and pixel digests are only meaningful on a machine that renders
      // text and shapes byte-for-byte the same way; this is how that is decided.
      fingerprint: first.fingerprint,
      dom: first.dom,
      layout: first.layout,
      pixels: first.pixels,
    };
    await writeFile(goldenFile, `${JSON.stringify(golden, null, 2)}\n`);
    console.log(`recorded ${first.frames} golden frames to ${goldenFile}`);
  } else {
    const golden = JSON.parse(await readFile(goldenFile, "utf8").catch(error => {
      throw error.code === "ENOENT"
        ? new Error(`no golden sequence for ${process.platform}-${process.arch}; record one with `
          + "`bun run --cwd packages/blitsen golden:record`")
        : error;
    }));
    const portable = golden.fingerprint === first.fingerprint;
    const compared = portable ? streams : ["dom"];
    const against = { ...golden };
    if (!portable) for (const stream of streams.slice(1)) against[stream] = first[stream];
    const drifted = diverged(first, against);
    if (drifted.length > 0)
      failures.push(`golden ${compared.join("/")} digests diverged at frames ${drifted.slice(0, 8).join(", ")}`);
    if (!portable) {
      console.log("rendering fingerprint differs from the golden "
        + `(${first.fingerprint} vs ${golden.fingerprint}): layout and pixel digests are `
        + "recorded but not gated on this host; see docs/M3.md");
    }

    if (failures.length > 0) {
      const frames = [...new Set([...unstable, ...drifted])].sort((a, b) => a - b).slice(0, 8);
      await rm(dumpDirectory, { recursive: true, force: true });
      await mkdir(dumpDirectory, { recursive: true });
      await replay("dump", ["--record", dumpDirectory, "--frames", frames.join(",")]);
      await writeFile(join(dumpDirectory, "report.json"),
        `${JSON.stringify({ failures, frames, fingerprint: first.fingerprint, golden: golden.fingerprint }, null, 2)}\n`);
      console.error(`wrote diverging frames ${frames.join(", ")} to ${dumpDirectory}`);
    } else {
      console.log(`Frame determinism verified: ${first.frames} frames, `
        + `${compared.join("/")} digests stable across two processes`
        + `${portable ? " and equal to the committed golden" : ""}.`);
    }
  }
} finally {
  await rm(workspace, { recursive: true, force: true });
}

for (const failure of failures) console.error(`determinism failure: ${failure}`);
if (failures.length > 0) process.exitCode = 1;
