// Runs one fixed-timestep replay in a process of its own and writes the report.
//
// A process per run is the point: two runs that agree from separate processes
// agree about more than one that reuses a warm heap, a warm JIT and a warm
// font cache.
//
// usage: bun replay-once.mjs <addon.node> <trace.json> <report.json> [--record <dir>] [--frames 1,2]
import { readFile, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { join, resolve } from "node:path";

const [addonPath, tracePath, reportPath, ...rest] = process.argv.slice(2);
if (!addonPath || !tracePath || !reportPath)
  throw new Error("usage: bun replay-once.mjs <addon.node> <trace.json> <report.json>");

const option = name => {
  const index = rest.indexOf(name);
  return index === -1 ? null : rest[index + 1];
};
const recordInto = option("--record");
const recordFrames = option("--frames")?.split(",").map(Number) ?? null;

const native = createRequire(import.meta.url)(resolve(addonPath));
const trace = await readFile(tracePath, "utf8");
const repository = resolve(import.meta.dir, "../../..");
const entrypoint = join(repository, JSON.parse(trace).application, "index.html");
const report = native.replayDocumentFrames(entrypoint, trace, recordInto, recordFrames);
await writeFile(reportPath, report);
