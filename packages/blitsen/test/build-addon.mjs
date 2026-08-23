// Building the native addon and putting it where a runner can load it.
//
// Every runner in this directory needs this before it can do anything else, and
// they all needed it the same way: cargo names the library per platform, but
// `require` decides by extension, so what cargo built is copied to
// `blitsen.node` rather than loaded where it was left.
import { copyFile } from "node:fs/promises";
import { join, resolve } from "node:path";

export const repository = resolve(import.meta.dir, "../../..");

const LIBRARY_NAMES = {
  linux: "libblitsen_node.so",
  darwin: "libblitsen_node.dylib",
  win32: "blitsen_node.dll",
};

/**
 * Builds `blitsen-node` and returns the path to a loadable `blitsen.node`.
 *
 * `purpose` names the caller in the unsupported-platform error, so a runner
 * that cannot run here still says which one it was.
 */
export async function buildAddon({ purpose, release = false, features = [], into } = {}) {
  const libraryName = LIBRARY_NAMES[process.platform];
  if (!libraryName) throw new Error(`unsupported ${purpose} platform: ${process.platform}`);

  const build = Bun.spawnSync({
    cmd: ["cargo", "build", ...(release ? ["--release"] : []), "-p", "blitsen-node",
      ...(features.length > 0 ? ["--features", features.join(",")] : [])],
    cwd: repository,
    stdout: "inherit",
    stderr: "inherit",
  });
  if (build.exitCode !== 0) process.exit(build.exitCode);

  // Cargo may take its target directory from a user or CI config. Asking it is
  // what keeps the harness from copying a stale checkout-local library after a
  // successful build somewhere else.
  const metadata = Bun.spawnSync({
    cmd: ["cargo", "metadata", "--format-version", "1", "--no-deps"],
    cwd: repository,
    stdout: "pipe",
    stderr: "inherit",
  });
  if (metadata.exitCode !== 0) process.exit(metadata.exitCode);
  const targetRoot = JSON.parse(metadata.stdout.toString()).target_directory;
  if (!targetRoot) throw new Error("cargo metadata named no target directory for the native harness");
  const target = join(targetRoot, release ? "release" : "debug");
  const addon = join(into ?? target, "blitsen.node");
  await copyFile(join(target, libraryName), addon);
  return addon;
}

/**
 * Builds the Phase 2 runtime an acceptance run is about to drive.
 *
 * The addon has `buildAddon` for the same reason: a runner that silently used
 * the last build is a runner that can pass against code nobody is running.
 */
export function buildRuntime({ release = true } = {}) {
  const build = Bun.spawnSync({
    cmd: ["cargo", "build", ...(release ? ["--release"] : []), "-p", "blitsen-runtime"],
    cwd: repository,
    stdout: "inherit",
    stderr: "inherit",
  });
  if (build.exitCode !== 0) process.exit(build.exitCode);
}
