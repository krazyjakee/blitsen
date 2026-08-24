#!/usr/bin/env node

import process from "node:process";

function parseVersion(value, context) {
  if (typeof value !== "string") {
    throw new Error(`${context} is not a string`);
  }

  // Cargo accepts a two-component Rust release (for example `1.95`) while
  // dependency metadata may use full SemVer. Normalize the missing patch to 0
  // and retain prerelease identifiers so their ordering remains SemVer-correct.
  const match = value.match(
    /^(0|[1-9]\d*)\.(0|[1-9]\d*)(?:\.(0|[1-9]\d*))?(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/,
  );
  if (!match) {
    throw new Error(`${context} has invalid version ${JSON.stringify(value)}`);
  }

  const prerelease = match[4]?.split(".") ?? [];
  for (const identifier of prerelease) {
    if (/^\d+$/.test(identifier) && identifier.length > 1 && identifier[0] === "0") {
      throw new Error(`${context} has invalid numeric prerelease ${JSON.stringify(identifier)}`);
    }
  }

  return {
    core: [BigInt(match[1]), BigInt(match[2]), BigInt(match[3] ?? "0")],
    prerelease,
  };
}

function compareVersions(left, right) {
  for (let index = 0; index < left.core.length; index += 1) {
    if (left.core[index] < right.core[index]) return -1;
    if (left.core[index] > right.core[index]) return 1;
  }

  if (left.prerelease.length === 0) return right.prerelease.length === 0 ? 0 : 1;
  if (right.prerelease.length === 0) return -1;

  const length = Math.max(left.prerelease.length, right.prerelease.length);
  for (let index = 0; index < length; index += 1) {
    const leftPart = left.prerelease[index];
    const rightPart = right.prerelease[index];
    if (leftPart === undefined) return -1;
    if (rightPart === undefined) return 1;

    const leftNumeric = /^\d+$/.test(leftPart);
    const rightNumeric = /^\d+$/.test(rightPart);
    if (leftNumeric && rightNumeric) {
      const leftNumber = BigInt(leftPart);
      const rightNumber = BigInt(rightPart);
      if (leftNumber < rightNumber) return -1;
      if (leftNumber > rightNumber) return 1;
    } else if (leftNumeric !== rightNumeric) {
      return leftNumeric ? -1 : 1;
    } else if (leftPart !== rightPart) {
      return leftPart < rightPart ? -1 : 1;
    }
  }
  return 0;
}

const declared = process.argv[2];
if (!declared) {
  throw new Error("usage: check-rust-version.mjs <declared-rust-version>");
}

let input = "";
for await (const chunk of process.stdin) input += chunk;

const metadata = JSON.parse(input);
if (!Array.isArray(metadata.packages)) {
  throw new Error("cargo metadata output has no packages array");
}

const floor = parseVersion(declared, "workspace rust-version");
const declaredPackages = metadata.packages.filter(({ rust_version: version }) => version !== null);
const violations = declaredPackages.filter((pkg) => {
  const version = parseVersion(pkg.rust_version, `${pkg.name} ${pkg.version} rust_version`);
  return compareVersions(version, floor) > 0;
});

if (violations.length > 0) {
  console.error(`Resolved packages require Rust newer than the declared ${declared} floor:`);
  for (const pkg of violations.sort((left, right) => left.name.localeCompare(right.name))) {
    console.error(`- ${pkg.name} ${pkg.version}: Rust ${pkg.rust_version}`);
  }
  process.exitCode = 1;
} else {
  console.log(
    `Rust ${declared} covers ${declaredPackages.length} declared package floors `
      + `across ${metadata.packages.length} resolved packages.`,
  );
}
