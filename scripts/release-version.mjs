#!/usr/bin/env bun
// One version boundary for a native release. Cargo's workspace version is an
// implementation detail; the npm manifest is the distribution's identity.

import { readFile } from "node:fs/promises";

async function manifest(path) {
  let parsed;
  try {
    parsed = JSON.parse(await readFile(path, "utf8"));
  } catch (error) {
    throw new Error(`could not read release manifest ${path}: ${error.message}`);
  }
  if (typeof parsed.name !== "string" || typeof parsed.version !== "string"
      || parsed.version.length === 0) {
    throw new Error(`${path} must carry non-empty string name and version fields`);
  }
  return { path, name: parsed.name, version: parsed.version };
}

/** Returns the single version carried by every supplied npm manifest. */
export async function matchingManifestVersion(paths) {
  if (paths.length === 0) throw new Error("at least one release manifest is required");
  const manifests = await Promise.all(paths.map(manifest));
  const expected = manifests[0].version;
  const mismatched = manifests.filter(one => one.version !== expected);
  if (mismatched.length > 0) {
    const found = manifests.map(one => `${one.name}@${one.version} (${one.path})`).join(", ");
    throw new Error(`release manifest version mismatch: expected ${expected}; found ${found}`);
  }
  return expected;
}

async function execute(path) {
  const child = Bun.spawn([path, "--version"], { stdout: "pipe", stderr: "pipe" });
  const [stdout, stderr, exitCode] = await Promise.all([
    new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited,
  ]);
  return { stdout, stderr, exitCode };
}

/** Proves a staged executable reports the version of both packages shipping it. */
export async function checkStagedRuntime({ executable, manifests, run = execute }) {
  const expected = await matchingManifestVersion(manifests);
  const { stdout, stderr, exitCode } = await run(executable);
  if (exitCode !== 0) {
    throw new Error(`${executable} --version exited ${exitCode}: ${stderr.trim() || stdout.trim()}`);
  }
  const reported = stdout.trim();
  const wanted = `blitsen-runtime ${expected}`;
  if (reported !== wanted) {
    throw new Error(`staged runtime version mismatch: ${executable} reported `
      + `${JSON.stringify(reported)}, expected ${JSON.stringify(wanted)} from ${manifests.join(" and ")}`);
  }
  return expected;
}

if (import.meta.main) {
  const [mode, ...args] = process.argv.slice(2);
  try {
    if (mode === "manifests") {
      console.log(await matchingManifestVersion(args));
    } else if (mode === "runtime" && args.length === 3) {
      console.log(await checkStagedRuntime({ executable: args[0], manifests: args.slice(1) }));
    } else {
      throw new Error("usage: release-version.mjs manifests <package.json>... "
        + "| runtime <executable> <main-package.json> <platform-package.json>");
    }
  } catch (error) {
    console.error(`::error::${error.message}`);
    process.exitCode = 1;
  }
}
