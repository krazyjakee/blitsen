// Size regression gate (issue #55). Compares the exported artifact against the
// committed per-platform baseline and fails on growth beyond the threshold.
// P1 has no numeric target left (M0 withdrew it), so this gate is about drift:
// every megabyte added to the export has to be an argued-for decision.
import { readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { formatBytes, measureExport } from "./measure-export.mjs";

const baselineFile = join(import.meta.dir, "metrics/size-baseline.json");
const COMPONENTS = {
  hostRuntimeBytes: "host runtime",
  nativeAddonBytes: "native addon",
  applicationBytes: "application assets",
  packagingBytes: "packaging",
};

const argv = process.argv.slice(2);
const measurementsIndex = argv.indexOf("--measurements");
const update = argv.includes("--update");

const record = measurementsIndex < 0
  ? await measureExport({ runs: 1 })
  : JSON.parse(await readFile(argv[measurementsIndex + 1], "utf8"));
const baseline = JSON.parse(await readFile(baselineFile, "utf8"));

if (update) {
  baseline.platforms[record.platform] = {
    recordedAt: record.recordedAt,
    commit: record.commit,
    bun: record.bun,
    rustc: record.rustc,
    host: record.host,
    installedBytes: record.size.installedBytes,
    compressedBytes: record.size.compressedBytes,
    components: record.size.components,
  };
  await writeFile(baselineFile, `${JSON.stringify(baseline, null, 2)}\n`);
  console.log(`Recorded ${record.platform} size baseline: `
    + `${record.size.installedBytes} B installed, ${record.size.compressedBytes} B gzip level 9.`);
  process.exit(0);
}

const previous = baseline.platforms[record.platform];
const lines = [];
const delta = (current, before) => {
  if (!before) return { bytes: null, percent: null, text: "no baseline" };
  const bytes = current - before;
  const percent = (bytes / before) * 100;
  return { bytes, percent, text: `${bytes >= 0 ? "+" : "-"}${formatBytes(Math.abs(bytes))} `
    + `(${percent >= 0 ? "+" : ""}${percent.toFixed(2)}%)` };
};

lines.push(`| measurement | baseline | current | delta |`, `| --- | --- | --- | --- |`);
for (const [label, key] of [["installed", "installedBytes"], ["gzip level 9", "compressedBytes"]]) {
  const current = record.size[key];
  const before = previous?.[key] ?? null;
  lines.push(`| **${label}** | ${before === null ? "—" : formatBytes(before)} `
    + `| ${current === null ? "—" : formatBytes(current)} | ${delta(current, before).text} |`);
}
for (const [key, label] of Object.entries(COMPONENTS)) {
  const current = record.size.components[key];
  const before = previous?.components?.[key] ?? null;
  lines.push(`| ${label} | ${before === null ? "—" : formatBytes(before)} `
    + `| ${formatBytes(current)} | ${delta(current, before).text} |`);
}

const notes = [];
if (!previous) {
  notes.push(`No committed baseline for ${record.platform}: the gate is reporting only. `
    + "Record one with `bun test/run-size-gate.mjs --update` on that platform.");
} else if (previous.host !== record.host) {
  // A host swap is a different artifact, not a drift in this one: the gate is
  // measuring two things and the delta between them means nothing.
  notes.push(`Host changed since the baseline (${previous.host ?? "bun"} -> ${record.host}); `
    + "this is a migration, not drift. Re-record the baseline on the new host.");
} else if (previous.bun !== record.bun || previous.rustc !== record.rustc) {
  notes.push(`Toolchain moved since the baseline (bun ${previous.bun} -> ${record.bun}, `
    + `${previous.rustc} -> ${record.rustc}); part of the delta is not Blitsen's.`);
}

const failures = [];
if (previous) {
  for (const [label, key] of [["installed", "installedBytes"], ["gzip level 9", "compressedBytes"]]) {
    const current = record.size[key];
    if (current === null || previous[key] === null) continue;
    const { percent } = delta(current, previous[key]);
    if (percent > baseline.thresholdPercent) {
      failures.push(`${label} size grew ${percent.toFixed(2)}%, over the `
        + `${baseline.thresholdPercent}% threshold`);
    }
    // A large shrink is good news and a stale baseline: it silently widens the
    // headroom the gate allows, so it gets flagged rather than pocketed.
    if (percent < -baseline.thresholdPercent) {
      notes.push(`${label} size fell ${Math.abs(percent).toFixed(2)}% — re-record the baseline `
        + "so the gate keeps its resolution.");
    }
  }
}

const report = [
  `### Export size — ${record.platform} (${record.commit ?? "working tree"})`,
  "",
  ...lines,
  "",
  ...notes.map(note => `> ${note}`),
  ...failures.map(failure => `> **FAIL** ${failure}`),
].join("\n");
console.log(report);
if (process.env.GITHUB_STEP_SUMMARY) {
  await writeFile(process.env.GITHUB_STEP_SUMMARY, `${report}\n\n`, { flag: "a" });
}
if (failures.length > 0) process.exit(1);
console.log(`Size gate passed for ${record.platform}.`);
