#!/usr/bin/env bun
// Hash and compare the two native files a release ships (#71). This runs before
// code signing: RFC 3161 and Apple secure timestamps are deliberately outside
// the reproducibility boundary and must not be normalised away.
import { createHash } from "node:crypto";
import { appendFile, readFile } from "node:fs/promises";
import { join } from "node:path";

const digest = bytes => createHash("sha256").update(bytes).digest("hex");

function firstDifference(first, second) {
  const shared = Math.min(first.length, second.length);
  for (let offset = 0; offset < shared; offset += 1) {
    if (first[offset] !== second[offset]) return offset;
  }
  return first.length === second.length ? null : shared;
}

function context(bytes, offset) {
  const start = Math.max(0, offset - 8);
  return `${start}: ${bytes.subarray(start, start + 24).toString("hex")}`;
}

async function artifact(path, name) {
  const bytes = await readFile(path);
  return { name, path, bytes, hash: digest(bytes) };
}

async function summary(lines, path = process.env.GITHUB_STEP_SUMMARY) {
  if (path) await appendFile(path, `${lines.join("\n")}\n`);
}

/** Records the unsigned hashes for both native artifacts on every target. */
export async function hashReleaseArtifacts({
  root, library, executable, output = console.log,
  summaryPath = process.env.GITHUB_STEP_SUMMARY,
}) {
  const files = await Promise.all([
    artifact(join(root, library), "blitsen.node"),
    artifact(join(root, executable), executable),
  ]);
  for (const file of files) output(`unsigned ${file.name}: ${file.hash} (${file.bytes.length} B)`);
  await summary([
    "| Unsigned artifact | SHA-256 | Bytes |",
    "| --- | --- | ---: |",
    ...files.map(file => `| \`${file.name}\` | \`${file.hash}\` | ${file.bytes.length} |`),
  ], summaryPath);
  return files.map(({ name, hash, bytes }) => ({ name, hash, bytes: bytes.length }));
}

/** Fails with byte-level diagnostics unless both clean builds are identical. */
export async function compareReleaseBuilds({
  firstRoot, secondRoot, library, executable, output = console.log,
  summaryPath = process.env.GITHUB_STEP_SUMMARY,
}) {
  const names = [[library, "blitsen.node"], [executable, executable]];
  const compared = [];
  const differences = [];
  for (const [path, name] of names) {
    const [first, second] = await Promise.all([
      artifact(join(firstRoot, path), name), artifact(join(secondRoot, path), name),
    ]);
    const differentAt = firstDifference(first.bytes, second.bytes);
    output(`clean build A ${name}: ${first.hash} (${first.bytes.length} B)`);
    output(`clean build B ${name}: ${second.hash} (${second.bytes.length} B)`);
    if (differentAt !== null) {
      differences.push(`unsigned ${name} is not reproducible: first differing byte ${differentAt}; `
        + `A=${first.hash}/${first.bytes.length}B [${context(first.bytes, differentAt)}], `
        + `B=${second.hash}/${second.bytes.length}B [${context(second.bytes, differentAt)}]`);
      continue;
    }
    compared.push({ name, hash: first.hash, bytes: first.bytes.length });
  }
  if (differences.length > 0) throw new Error(differences.join("; "));
  await summary([
    "| Reproducible unsigned artifact | SHA-256 | Bytes |",
    "| --- | --- | ---: |",
    ...compared.map(file => `| \`${file.name}\` | \`${file.hash}\` | ${file.bytes} |`),
  ], summaryPath);
  return compared;
}

if (import.meta.main) {
  const [mode, ...args] = process.argv.slice(2);
  try {
    if (mode === "--hash" && args.length === 3) {
      await hashReleaseArtifacts({ root: args[0], library: args[1], executable: args[2] });
    } else if (mode === "--compare" && args.length === 4) {
      await compareReleaseBuilds({
        firstRoot: args[0], secondRoot: args[1], library: args[2], executable: args[3],
      });
    } else {
      throw new Error("usage: compare-release-builds.mjs --hash <root> <library> <executable> "
        + "| --compare <first-root> <second-root> <library> <executable>");
    }
  } catch (error) {
    console.error(`::error::${error.message}`);
    process.exitCode = 1;
  }
}
