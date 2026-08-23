#!/usr/bin/env bun
// Hash and compare the two native files a release ships (#71). This runs before
// code signing: RFC 3161 and Apple secure timestamps are deliberately outside
// the reproducibility boundary and must not be normalised away.
import { createHash } from "node:crypto";
import { appendFile, readFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";

const digest = bytes => createHash("sha256").update(bytes).digest("hex");

function digestWithZeroedRange(bytes, offset, length) {
  return createHash("sha256")
    .update(bytes.subarray(0, offset))
    .update(Buffer.alloc(length))
    .update(bytes.subarray(offset + length))
    .digest("hex");
}

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

function peImage(bytes) {
  if (bytes.length < 0x40 || bytes.toString("ascii", 0, 2) !== "MZ") return null;
  const header = bytes.readUInt32LE(0x3c);
  if (header + 24 > bytes.length || bytes.toString("ascii", header, header + 4) !== "PE\0\0") {
    return null;
  }
  const numberOfSections = bytes.readUInt16LE(header + 6);
  const timestampOffset = header + 8;
  const sectionTable = header + 24 + bytes.readUInt16LE(header + 20);
  if (sectionTable + numberOfSections * 40 > bytes.length) return null;
  const sections = [];
  for (let index = 0; index < numberOfSections; index += 1) {
    const offset = sectionTable + index * 40;
    const name = bytes.subarray(offset, offset + 8).toString("ascii").replace(/\0.*$/, "");
    const size = bytes.readUInt32LE(offset + 16);
    const pointer = bytes.readUInt32LE(offset + 20);
    const valid = pointer <= bytes.length && size <= bytes.length - pointer;
    sections.push({
      name: name || `<section-${index}>`, size, pointer,
      hash: valid ? digest(bytes.subarray(pointer, pointer + size)) : "invalid-range",
    });
  }
  return {
    timestamp: bytes.readUInt32LE(timestampOffset),
    timestampNeutralHash: digestWithZeroedRange(bytes, timestampOffset, 4),
    sections,
  };
}

function sourceRootOccurrences(bytes, root) {
  const spellings = new Set([root, root.replaceAll("\\", "/"), root.replaceAll("/", "\\")]);
  const occurrences = [];
  for (const spelling of spellings) {
    for (const [encoding, needle] of [
      ["utf8", Buffer.from(spelling)], ["utf16le", Buffer.from(spelling, "utf16le")],
    ]) {
      const offset = bytes.indexOf(needle);
      if (offset !== -1) occurrences.push(`${encoding}:${JSON.stringify(spelling)}@${offset}`);
    }
  }
  return occurrences;
}

function peDifference(first, second, firstSourceRoot, secondSourceRoot) {
  const firstPe = peImage(first);
  const secondPe = peImage(second);
  if (!firstPe || !secondPe) return null;
  const details = [
    `PE COFF TimeDateStamp A=0x${firstPe.timestamp.toString(16).padStart(8, "0")}, `
      + `B=0x${secondPe.timestamp.toString(16).padStart(8, "0")}`,
    `timestamp-neutral SHA-256 A=${firstPe.timestampNeutralHash}, `
      + `B=${secondPe.timestampNeutralHash}`,
  ];
  const sectionCount = Math.max(firstPe.sections.length, secondPe.sections.length);
  const changedSections = [];
  for (let index = 0; index < sectionCount; index += 1) {
    const a = firstPe.sections[index];
    const b = secondPe.sections[index];
    if (!a || !b || a.name !== b.name || a.size !== b.size || a.hash !== b.hash) {
      changedSections.push(`${a?.name ?? "<missing>"}(${a?.size ?? 0}B) -> `
        + `${b?.name ?? "<missing>"}(${b?.size ?? 0}B)`);
    }
  }
  details.push(changedSections.length > 0
    ? `changed PE sections: ${changedSections.join(", ")}`
    : "no PE section payload differences (metadata or overlay only)");
  const firstLeaks = sourceRootOccurrences(first, firstSourceRoot);
  const secondLeaks = sourceRootOccurrences(second, secondSourceRoot);
  if (firstLeaks.length > 0 || secondLeaks.length > 0) {
    details.push(`checkout-root occurrences A=[${firstLeaks.join(", ")}], `
      + `B=[${secondLeaks.join(", ")}]`);
  }
  return details.join("; ");
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
  const firstSourceRoot = dirname(dirname(resolve(firstRoot)));
  const secondSourceRoot = dirname(dirname(resolve(secondRoot)));
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
      const pe = peDifference(first.bytes, second.bytes, firstSourceRoot, secondSourceRoot);
      differences.push(`unsigned ${name} is not reproducible: first differing byte ${differentAt}; `
        + `A=${first.hash}/${first.bytes.length}B [${context(first.bytes, differentAt)}], `
        + `B=${second.hash}/${second.bytes.length}B [${context(second.bytes, differentAt)}]`
        + `${pe ? `; ${pe}` : ""}`);
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
