import { copyFile } from "node:fs/promises";
import { join, resolve } from "node:path";

const repository = resolve(import.meta.dir, "../../..");
const libraryName = {
  linux: "libblitsen_node.so",
  darwin: "libblitsen_node.dylib",
  win32: "blitsen_node.dll",
}[process.platform];

if (!libraryName) throw new Error(`unsupported native harness platform: ${process.platform}`);

const build = Bun.spawnSync({
  cmd: ["cargo", "build", "-p", "blitsen-node"],
  cwd: repository,
  stdout: "inherit",
  stderr: "inherit",
});
if (build.exitCode !== 0) process.exit(build.exitCode);

const target = join(repository, "target", "debug");
const addon = join(target, "blitsen.node");
await copyFile(join(target, libraryName), addon);

const harness = Bun.spawnSync({
  cmd: [process.execPath, join(import.meta.dir, "native-harness.mjs"), addon],
  cwd: repository,
  stdout: "inherit",
  stderr: "inherit",
});
process.exitCode = harness.exitCode;
