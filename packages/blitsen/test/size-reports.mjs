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

export function runtimeSizeSummary(record) {
  return [
    `### Bun bare-app size — ${record.platform}`,
    "",
    `Bun ${record.bun}; commit \`${record.commit ?? "working tree"}\`.`,
    "",
    "| measurement | installed | gzip-9 |",
    "| --- | ---: | ---: |",
    `| standalone executable | ${bytes(record.runtime.bytes)} | ${bytes(record.runtime.gzip)} |`,
    `| native addon (included above) | ${bytes(record.components.nativeAddon)} | — |`,
    "",
    "New runtime baseline; historical QuickJS measurements are not a regression threshold.",
  ].join("\n");
}
