import { execFile } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

import { linkBundle } from "../src/bundle.mjs";
import { machoFixture } from "./fixtures/macho.mjs";

const run = promisify(execFile);
const repository = dirname(dirname(dirname(dirname(fileURLToPath(import.meta.url)))));
const directory = await mkdtemp(join(tmpdir(), "blitsen-macho-conformance-"));

try {
  for (const cpu of [0x01000007, 0x0100000c]) {
    const suffix = cpu === 0x0100000c ? "arm64" : "x86_64";
    const runtime = join(directory, `runtime-${suffix}`);
    const rustOutput = join(directory, `rust-${suffix}`);
    const javascriptOutput = join(directory, `javascript-${suffix}`);
    await writeFile(runtime, machoFixture(cpu).executable, { mode: 0o755 });
    await run("cargo", [
      "run", "--quiet", "--locked", "--release", "-p", "blitsen-core",
      "--features", "test-support", "--example", "bundle-reference", "--",
      runtime, rustOutput,
    ], { cwd: repository });
    await linkBundle({ runtime, output: javascriptOutput, files: new Map() });
    const rust = await readFile(rustOutput);
    const javascript = await readFile(javascriptOutput);
    if (!rust.equals(javascript)) {
      throw new Error(`${suffix} Mach-O writers differ (${rust.length} vs ${javascript.length} bytes)`);
    }
  }
  console.log("Rust and JavaScript Mach-O writers agree for x86_64 and arm64");
} finally {
  await rm(directory, { recursive: true, force: true });
}
