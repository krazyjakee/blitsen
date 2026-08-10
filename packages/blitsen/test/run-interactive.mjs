import { copyFile } from "node:fs/promises";
import { join, resolve } from "node:path";

const repository = resolve(import.meta.dir, "../../..");
const libraryName = {
  linux: "libblitsen_node.so",
  darwin: "libblitsen_node.dylib",
  win32: "blitsen_node.dll",
}[process.platform];

if (!libraryName) throw new Error(`unsupported interactive demo platform: ${process.platform}`);

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

console.log("Interactive: click the control to expand it, then use ← → or Space.");
const application = Bun.spawnSync({
  cmd: [
    process.execPath,
    join(repository, "packages/blitsen/bin/blitsen.mjs"),
    join(repository, "examples/interactive"),
    "--width", "960", "--height", "640", "--title", "Blitsen Interactive",
    ...process.argv.slice(2),
  ],
  cwd: repository,
  env: { ...process.env, BLITSEN_NATIVE_PATH: addon },
  stdout: "inherit",
  stderr: "inherit",
});
process.exitCode = application.exitCode;
