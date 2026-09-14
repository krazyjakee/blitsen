// Generates the third-party notices a platform package ships (issue #121).
//
//     bun run --cwd packages/blitsen notices            # this platform
//     bun run --cwd packages/blitsen notices --target linux-x64 --out <dir>
//     bun run --cwd packages/blitsen notices --target android-arm64 \
//       --root blitsen-android --out <dir>          # what an APK carries
//
// Run where the runtime is built: this checkout, and the release job, which
// stages the pair beside the executable it just compiled. A user's machine never
// runs this — it consumes what the platform package already carries.
import { mkdir } from "node:fs/promises";
import { join } from "node:path";

import { collectNotices, writeNotices } from "../src/notices.mjs";
import { hostTarget, packageVersion } from "../src/runtime.mjs";
import { argument, capture, repository } from "./build-addon.mjs";

export const RUST_TARGETS = {
  "linux-x64": "x86_64-unknown-linux-gnu",
  "linux-arm64": "aarch64-unknown-linux-gnu",
  "darwin-x64": "x86_64-apple-darwin",
  "darwin-arm64": "aarch64-apple-darwin",
  "win32-x64": "x86_64-pc-windows-msvc",
  "win32-arm64": "aarch64-pc-windows-msvc",
};

const run = (command, args) => capture([command, ...args], { cwd: repository });

// Guarded so the redistribution gate can import the target table above without
// generating anything.
if (import.meta.main) {
  const target = argument("target", hostTarget());
  if (!(target in RUST_TARGETS)) {
    console.error(`unknown --target ${target} (expected one of: ${Object.keys(RUST_TARGETS).join(", ")})`);
    process.exit(1);
  }
  const roots = argument("root") ? [argument("root")] : ["blitsen-node"];
  const out = argument("out", join(repository, "packages/platforms", target));
  await mkdir(out, { recursive: true });
  const version = await packageVersion();

  for (const root of roots) {
    const collected = await collectNotices({ target: RUST_TARGETS[target], root, run });
    const directory = out;
    await mkdir(directory, { recursive: true });
    const written = await writeNotices(directory, collected, { version });
    console.log(`${root}: ${written.packages} packages -> ${written.text}`);
    for (const problem of written.problems) console.log(`  unresolved: ${problem}`);
    if (written.problems.length > 0) {
      console.error(`${root}: ${written.problems.length} package(s) whose terms cannot be honoured; `
        + "the export gate refuses these");
      process.exitCode = 1;
    }
  }
}
