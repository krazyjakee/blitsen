// Startup and idle-RAM benchmarks (issue #49), reported as a time series rather
// than a pass/fail snapshot: hosted runners are too noisy for a timing gate, so
// this records and compares, and never fails the build on a measurement.
//
// What is real and what is a proxy:
//   * headless first paint  — PROXY for P2. Spawn to exit of the exported
//     executable under BLITSEN_STANDALONE_CHECK, which loads the addon, unpacks
//     the embedded app, parses it, runs its scripts and rasterizes one frame on
//     the CPU. No window, no GPU, no swapchain — and it also pays for the
//     harness's DOM snapshot and PNG encode, which a real frame does not.
//   * headless peak RSS     — PROXY for P3, from the same run: no window and no
//     GPU allocations, so it is a floor for idle RAM, not idle RAM.
//   * windowed first frame / idle RSS — the real P2 and P3 metrics. They need a
//     desktop session, so they only appear when a human runs `bench:windowed`.
import { appendFile, readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { formatBytes, measureExport } from "./measure-export.mjs";

const historyFile = join(import.meta.dir, "metrics/benchmark-history.jsonl");

const argv = process.argv.slice(2);
const measurementsIndex = argv.indexOf("--measurements");
const record = measurementsIndex < 0
  ? await measureExport({ runs: 5, windowed: argv.includes("--windowed") })
  : JSON.parse(await readFile(argv[measurementsIndex + 1], "utf8"));

const history = (await readFile(historyFile, "utf8").catch(() => ""))
  .split("\n").filter(Boolean).map(line => JSON.parse(line))
  .filter(entry => entry.platform === record.platform && entry.environment === record.environment);
const previous = history.at(-1) ?? null;
const oldest = history.at(0) ?? null;

const drift = (current, before) => {
  if (current === null || before === null || before === undefined) return "—";
  const percent = ((current - before) / before) * 100;
  return `${percent >= 0 ? "+" : ""}${percent.toFixed(1)}%`;
};
const milliseconds = value => (value === null ? "—" : `${value.median.toFixed(1)} ms`);

const rows = [
  ["Bun runtime floor (spawn to exit)", milliseconds(record.startup.bunRuntimeFloorMs),
    drift(record.startup.bunRuntimeFloorMs.median, previous?.startup.bunRuntimeFloorMs?.median),
    drift(record.startup.bunRuntimeFloorMs.median, oldest?.startup.bunRuntimeFloorMs?.median)],
  ["Headless first paint (P2 proxy)", milliseconds(record.startup.headlessFirstPaintMs),
    drift(record.startup.headlessFirstPaintMs.median, previous?.startup.headlessFirstPaintMs?.median),
    drift(record.startup.headlessFirstPaintMs.median, oldest?.startup.headlessFirstPaintMs?.median)],
  ["Headless peak RSS (P3 proxy)", formatBytes(record.memory.headlessPeakBytes),
    drift(record.memory.headlessPeakBytes, previous?.memory.headlessPeakBytes),
    drift(record.memory.headlessPeakBytes, oldest?.memory.headlessPeakBytes)],
];
if (record.startup.windowedFirstFrameMs) {
  rows.push(["Windowed first frame (P2, real)", milliseconds(record.startup.windowedFirstFrameMs),
    drift(record.startup.windowedFirstFrameMs.median, previous?.startup.windowedFirstFrameMs?.median),
    drift(record.startup.windowedFirstFrameMs.median, oldest?.startup.windowedFirstFrameMs?.median)]);
}
if (record.memory.windowedSteadyBytes) {
  rows.push(["Windowed idle RSS (P3, real)", formatBytes(record.memory.windowedSteadyBytes),
    drift(record.memory.windowedSteadyBytes, previous?.memory.windowedSteadyBytes),
    drift(record.memory.windowedSteadyBytes, oldest?.memory.windowedSteadyBytes)]);
}

const notes = [
  `${record.startup.headlessFirstPaintMs.runs} timed runs, first discarded as warm-up; `
  + `history has ${history.length} prior ${record.environment} entr${history.length === 1 ? "y" : "ies"} `
  + `for ${record.platform}.`,
];
if (!record.windowed) {
  notes.push("Windowed P2/P3 were not measured in this run. They need a desktop session: "
    + "run `bun run --cwd packages/blitsen bench:windowed` on a real display for the real numbers.");
}

const report = [
  `### Startup and idle RAM — ${record.platform} (${record.commit ?? "working tree"})`,
  "",
  "| metric | current | vs previous | vs oldest |",
  "| --- | --- | --- | --- |",
  ...rows.map(row => `| ${row.join(" | ")} |`),
  "",
  ...notes.map(note => `> ${note}`),
].join("\n");
console.log(report);
if (process.env.GITHUB_STEP_SUMMARY) {
  await writeFile(process.env.GITHUB_STEP_SUMMARY, `${report}\n\n`, { flag: "a" });
}
// CI cannot push, so the committed series only grows from local runs; CI keeps
// its own copy as a build artifact.
if (argv.includes("--record")) {
  await appendFile(historyFile, `${JSON.stringify(record)}\n`);
  console.log(`Appended to ${historyFile}.`);
}
