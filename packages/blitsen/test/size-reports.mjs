import { readFile, stat, writeFile } from "node:fs/promises";
import { gzipSync } from "node:zlib";

const bytes = value => `${(value / 1_000_000).toFixed(1)} MB`;

export const fileSize = async path => (await stat(path)).size;
export const gzippedSize = async path => gzipSync(await readFile(path), { level: 9 }).length;

/** Appends a report to the job summary when there is one to append to. */
export async function appendStepSummary(markdown) {
  if (!process.env.GITHUB_STEP_SUMMARY) return;
  await writeFile(process.env.GITHUB_STEP_SUMMARY, `${markdown}\n\n`, { flag: "a" });
}

export function phase2SizeSummary(record) {
  const components = record.components;
  const rows = [
    ["Phase 2 bare export", record.phase2.bytes, record.phase2.gzip],
    ["Phase 1 bare export", record.phase1.bytes, record.phase1.gzip],
    ["runtime executable", components.runtimeExecutable, null],
    ["application payload", components.appPayload, null],
  ].map(([label, installed, compressed]) =>
    `| ${label} | ${bytes(installed)} | ${compressed === null ? "—" : bytes(compressed)} |`);
  return [
    `### Phase 2 bare-app size — ${record.platform}`,
    "",
    `Commit \`${record.commit ?? "working tree"}\`; runtime pinned by \`BLITSEN_RUNTIME_PATH\`.`,
    "",
    "| measurement | installed | gzip-9 |",
    "| --- | ---: | ---: |",
    ...rows,
    "",
    `Phase 2 is **${record.ratio}× smaller** than Phase 1 on this runner. `
      + "This is report-only; the regression gate remains separate.",
  ].join("\n");
}
