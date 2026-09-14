// The directory interpreter and exported apps share the same Bun launcher.
import { copyFile, mkdir, readFile, writeFile, rm } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { launcherSource } from "../packages/blitsen/src/export.mjs";
import { cargoTargetDirectory } from "../packages/blitsen/src/cargo.mjs";
import { CARGO_LIBRARIES, hostTarget } from "../packages/blitsen/src/runtime.mjs";

const repository = resolve(import.meta.dirname, "..");
const profile = process.argv.includes("--debug") ? "debug" : "release";
const target = await cargoTargetDirectory(repository);
const directory = join(target, profile);
const output = join(directory, process.platform === "win32" ? "blitsen-runtime.exe" : "blitsen-runtime");
const stage = join(directory, ".bun-runtime");
await mkdir(stage, { recursive: true });
try {
  await copyFile(join(directory, CARGO_LIBRARIES[process.platform]), join(stage, "blitsen.node"));
  const nativeNotices = await readFile(join(directory, "NOTICES.txt"), "utf8").catch(() => null);
  const source = launcherSource([], {
    directory: true, layout: "directory", width: 800, height: 600, title: "Blitsen",
    storageIdentity: "blitsen.development",
    runtime: { target: hostTarget(), source: "repository", version: process.env.BLITSEN_RELEASE_VERSION ?? null },
    notices: nativeNotices,
    bunNotices: await readFile(join(repository, "packages/blitsen/src/BUN-LICENSE.md"), "utf8"),
  });
  const entry = join(stage, "launcher.mjs");
  await writeFile(entry, source);
  const result = await Bun.build({ entrypoints: [entry], compile: { outfile: output } });
  if (!result.success) throw new AggregateError(result.logs, "Bun runtime compilation failed");
  console.log(output);
} finally { await rm(stage, { recursive: true, force: true }); }
