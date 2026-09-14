// A new Bun baseline: never compare it to QuickJS as a regression gate.
import { mkdtemp, mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { gzipSync } from "node:zlib";
import { BARE_APP } from "./bare-app.mjs";
import { argument, buildAddon, capture, repository } from "./build-addon.mjs";
import { buildStandalone } from "../src/export.mjs";
import { appendStepSummary, runtimeSizeSummary } from "./size-reports.mjs";

const directory = await mkdtemp(join(tmpdir(), "blitsen-bun-size-"));
try {
  const addon = await buildAddon({ purpose: "runtime size", release: true });
  const root = join(directory, "app");
  await mkdir(root);
  await writeFile(join(root, "index.html"), BARE_APP);
  const result = await buildStandalone({ root, width: 800, height: 600, title: "Bare",
    outfile: join(directory, "Bare") }, addon);
  const bytes = await readFile(result.outfile);
  const record = {
    recordedAt: new Date().toISOString(),
    commit: capture(["git", "rev-parse", "HEAD"], { cwd: repository }).stdout.trim(),
    platform: `${process.platform}-${process.arch}`, application: "bare", host: "bun", bun: Bun.version,
    runtime: { bytes: bytes.length, gzip: gzipSync(bytes, { level: 9 }).length },
    components: { nativeAddon: (await stat(addon)).size, appPayload: Buffer.byteLength(BARE_APP) },
    boundary: "Standalone executable including Bun and native addon; gzip-9 is a compression proxy.",
  };
  const summary = runtimeSizeSummary(record);
  console.log(summary);
  await appendStepSummary(summary);
  if (argument("out")) await writeFile(argument("out"), JSON.stringify(record, null, 2) + "\n");
} finally { await rm(directory, { recursive: true, force: true }); }
