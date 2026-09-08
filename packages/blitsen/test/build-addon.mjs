// Building the native addon and putting it where a runner can load it.
//
// Every runner in this directory needs this before it can do anything else, and
// they all needed it the same way: cargo names the library per platform, but
// `require` decides by extension, so what cargo built is copied to
// `blitsen.node` rather than loaded where it was left.
//
// The runners' other shared furniture lives here too: the `--flag value`
// lookup, the captured `Bun.spawnSync`, and the demo launcher.
import { copyFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { cargoTargetDirectory } from "../src/android-toolchain.mjs";
import { CARGO_LIBRARIES } from "../src/runtime.mjs";

export const repository = resolve(import.meta.dir, "../../..");

/** One `--name value` off the command line. */
export function argument(name, fallback = null) {
  const at = process.argv.indexOf(`--${name}`);
  return at < 0 ? fallback : process.argv[at + 1];
}

/** Runs `cmd` to completion and returns what it printed, as strings. */
export function capture(cmd, { cwd, env } = {}) {
  const result = Bun.spawnSync({ cmd, cwd, env, stdout: "pipe", stderr: "pipe" });
  return { code: result.exitCode, stdout: result.stdout.toString(), stderr: result.stderr.toString() };
}

/**
 * Builds `blitsen-node` and returns the path to a loadable `blitsen.node`.
 *
 * `purpose` names the caller in the unsupported-platform error, so a runner
 * that cannot run here still says which one it was.
 */
export async function buildAddon({ purpose, release = false, features = [], into } = {}) {
  const libraryName = CARGO_LIBRARIES[process.platform];
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
  const targetRoot = await cargoTargetDirectory(repository, cmd => capture(cmd, { cwd: repository }));
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

/**
 * Builds the addon and opens `examples/<example>` through the CLI, the way
 * every demo runner does. `hints` are printed first, so the person watching the
 * window knows what to expect; the rest of argv is passed through to the CLI.
 */
export async function runExample({ purpose, example, width, height, title, hints = [] }) {
  const addon = await buildAddon({ purpose });
  for (const hint of hints) console.log(hint);
  const application = Bun.spawnSync({
    cmd: [
      process.execPath,
      join(repository, "packages/blitsen/bin/blitsen.mjs"),
      join(repository, "examples", example),
      ...(width === undefined ? [] : ["--width", String(width)]),
      ...(height === undefined ? [] : ["--height", String(height)]),
      ...(title === undefined ? [] : ["--title", title]),
      ...process.argv.slice(2),
    ],
    cwd: repository,
    env: { ...process.env, BLITSEN_NATIVE_PATH: addon },
    stdout: "inherit",
    stderr: "inherit",
  });
  process.exitCode = application.exitCode;
}
