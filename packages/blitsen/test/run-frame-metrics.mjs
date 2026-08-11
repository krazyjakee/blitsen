// P4 evidence: what one Pong frame costs, measured independently of the
// timestamps handed to JavaScript.
//
// The replay feeds the game a fixed timestep, so the run is reproducible, and
// times the frame with the wall clock, so the numbers cannot be produced by the
// clock the game reads. This records; it never fails on a timing threshold,
// because a hosted runner's timings are not a gate.
//
// usage: bun run-frame-metrics.mjs [--out <file>] [--alloc-audit] [--debug]
import { copyFile, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const argv = process.argv.slice(2);
const option = name => {
  const index = argv.indexOf(name);
  return index === -1 ? null : argv[index + 1];
};
const debug = argv.includes("--debug");
// Counting allocations wraps the global allocator, so the audit is a separate
// build: a shipped addon should not pay for a number nobody reads.
const audit = argv.includes("--alloc-audit");
const outFile = option("--out");

const repository = resolve(import.meta.dir, "../../..");
const traceFile = join(import.meta.dir, "replay/pong.trace.json");
const libraryName = {
  linux: "libblitsen_node.so",
  darwin: "libblitsen_node.dylib",
  win32: "blitsen_node.dll",
}[process.platform];
if (!libraryName) throw new Error(`unsupported frame-metrics target: ${process.platform}`);

const build = Bun.spawnSync({
  cmd: ["cargo", "build", ...(debug ? [] : ["--release"]), "-p", "blitsen-node",
    ...(audit ? ["--features", "alloc-audit"] : [])],
  cwd: repository,
  stdout: "inherit",
  stderr: "inherit",
});
if (build.exitCode !== 0) process.exit(build.exitCode);
const target = join(repository, "target", debug ? "debug" : "release");
const addon = join(target, "blitsen.node");
await copyFile(join(target, libraryName), addon);

const run = (script, extra = []) => {
  const result = Bun.spawnSync({
    cmd: [process.execPath, join(import.meta.dir, script), addon, ...extra],
    cwd: repository,
    stdout: "pipe",
    stderr: "pipe",
  });
  if (result.exitCode !== 0) {
    process.stderr.write(result.stderr.toString());
    throw new Error(`${script} exited ${result.exitCode}`);
  }
  return result.stdout.toString();
};

const workspace = await mkdtemp(join(tmpdir(), "blitsen-frame-metrics-"));
let replay;
let properties;
try {
  const reportFile = join(workspace, "replay.json");
  run("replay-once.mjs", [traceFile, reportFile]);
  replay = JSON.parse(await readFile(reportFile, "utf8"));
  properties = JSON.parse(run("property-cost.mjs"));
} finally {
  await rm(workspace, { recursive: true, force: true });
}

const ms = microseconds => (microseconds / 1000).toFixed(3);
const percentiles = histogram =>
  [histogram.count, ms(histogram.p50Us), ms(histogram.p95Us), ms(histogram.p99Us),
    ms(histogram.maxUs), histogram.overBudget];

const allocations = replay.records.map(record => record.allocations).filter(Boolean);
const quantile = (values, fraction) =>
  [...values].sort((left, right) => left - right)[Math.floor(values.length * fraction)];
const allocationRow = allocations.length === 0
  ? "not measured — rebuild with `--alloc-audit` to count them"
  : `${quantile(allocations.map(a => a.allocations), 0.5)} allocations `
    + `(${quantile(allocations.map(a => a.bytes), 0.5)} B) at the median frame, `
    + `${Math.max(...allocations.map(a => a.allocations))} at the worst; `
    + `${quantile(allocations.map(a => a.deallocations), 0.5)} frees`;

const profile = debug ? "debug" : "release";
const report = [
  `### Pong frame cost — ${replay.application} at ${replay.width}x${replay.height}, `
  + `${replay.frames} frames at ${(1000 / replay.frameDurationMs).toFixed(0)} Hz `
  + `(${profile}, ${process.platform}-${process.arch}${audit ? ", allocation audit" : ""})`,
  "",
  "| window | frames | p50 ms | p95 ms | p99 ms | max ms | over 16.7 ms |",
  "| --- | --- | --- | --- | --- | --- | --- |",
  `| all | ${percentiles(replay.histogram).join(" | ")} |`,
  `| after ${replay.warmupFrames}-frame warm-up | ${percentiles(replay.steady).join(" | ")} |`,
  "",
  `| bucket (ms) | ${replay.steady.buckets.map(b => b.upperMs ? `<= ${b.upperMs}` : "over").join(" | ")} |`,
  `| --- |${" --- |".repeat(replay.steady.buckets.length)}`,
  `| frames | ${replay.steady.buckets.map(b => b.frames).join(" | ")} |`,
  "",
  `| stage | mean ms | p95 ms | max ms | share |${audit ? " allocations |" : ""}`,
  `| --- | --- | --- | --- | --- |${audit ? " --- |" : ""}`,
  ...replay.stages.map(stage => `| ${stage.stage} | ${ms(stage.meanUs)} | ${ms(stage.p95Us)} `
    + `| ${ms(stage.maxUs)} | ${(stage.share * 100).toFixed(1)}% |`
    + (audit ? ` ${stage.allocations} |` : "")),
  `| — of which display list | ${ms(replay.displayList.meanUs)} | ${ms(replay.displayList.p95Us)} `
  + `| ${ms(replay.displayList.maxUs)} | |${audit ? " |" : ""}`,
  "",
  "| DOM operation | µs/call |",
  "| --- | --- |",
  ...Object.entries(properties)
    .filter(([key]) => key !== "iterations" && key !== "batches")
    .map(([operation, cost]) => `| ${operation} | ${cost} |`),
  "",
  `> Per-frame heap allocations: ${allocationRow}.`,
  "> Frame cost is the sum of the pipeline's stages: input, callbacks, style and layout, "
  + "display list and CPU rasterization. It excludes the harness's own digest and PNG work "
  + `(${ms(quantile(replay.records.map(r => r.digestUs), 0.5))} ms at the median frame).`,
  "> Headless and CPU-rasterized: a windowed frame trades the rasterizer for GPU submit and "
  + "present, which this cannot measure without a display.",
  "> Recorded, never gated: hosted runners are too noisy to fail a build on timing.",
].join("\n");

console.log(report);
if (process.env.GITHUB_STEP_SUMMARY) {
  await writeFile(process.env.GITHUB_STEP_SUMMARY, `${report}\n\n`, { flag: "a" });
}
if (outFile) {
  await writeFile(outFile, `${JSON.stringify({
    recorded: new Date().toISOString(),
    platform: `${process.platform}-${process.arch}`,
    profile,
    allocationAudit: audit,
    fingerprint: replay.fingerprint,
    frames: replay.frames,
    histogram: replay.histogram,
    steady: replay.steady,
    displayList: replay.displayList,
    stages: replay.stages,
    records: replay.records,
    propertyCostUs: properties,
  }, null, 2)}\n`);
  console.log(`\nWrote ${outFile}.`);
}
