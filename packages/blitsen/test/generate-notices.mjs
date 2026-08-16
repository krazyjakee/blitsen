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
import { repository } from "./build-addon.mjs";

const RUST_TARGETS = {
  "linux-x64": "x86_64-unknown-linux-gnu",
  "linux-arm64": "aarch64-unknown-linux-gnu",
  "darwin-x64": "x86_64-apple-darwin",
  "darwin-arm64": "aarch64-apple-darwin",
  "win32-x64": "x86_64-pc-windows-msvc",
  "win32-arm64": "aarch64-pc-windows-msvc",
  // Not npm platform packages, and deliberately not in `TARGETS` — #148's
  // decision 1 is that Android is not a seventh row in TECH.md §11. They are
  // here because an APK owes the same notices any other artifact does (#121)
  // and has nowhere else to get them: there is no platform package to carry
  // them, so `blitsen build --android` reads `BLITSEN_NOTICES_PATH` and says
  // the artifact is not cleared for redistribution when it is unset. This is
  // what sets it. The names are `doctor --target`'s, which already grades an
  // application against Android (#147), so the vocabulary is one thing.
  "android-arm64": "aarch64-linux-android",
  "android-x64": "x86_64-linux-android",
};

/// The roots whose notices are not a runtime's. `blitsen-node` is the addon a
/// carrying export links into Bun and goes in a subdirectory beside the
/// runtime's; `blitsen-android` is the whole artifact on its platform, so its
/// notices are the top-level ones there.
const ADDON_ROOTS = ["blitsen-node"];

function argument(name, fallback = null) {
  const at = process.argv.indexOf(`--${name}`);
  return at < 0 ? fallback : process.argv[at + 1];
}

const run = async (command, args) => {
  const result = Bun.spawnSync({ cmd: [command, ...args], cwd: repository, stdout: "pipe", stderr: "pipe" });
  return {
    code: result.exitCode,
    stdout: result.stdout.toString(),
    stderr: result.stderr.toString(),
  };
};

const target = argument("target", hostTarget());
if (!(target in RUST_TARGETS)) {
  console.error(`unknown --target ${target} (expected one of: ${Object.keys(RUST_TARGETS).join(", ")})`);
  process.exit(1);
}
// Two roots, because there are two hosts and they link different trees: an
// export links the runtime, and an export carrying a `.node` addon links the
// addon into Bun instead. The runtime's notices are the ones a default export
// carries; the addon's are collected so the audit can see both.
const roots = argument("root") ? [argument("root")] : ["blitsen-runtime", "blitsen-node"];
const out = argument("out", join(repository, "packages/platforms", target));
await mkdir(out, { recursive: true });
const version = await packageVersion();

for (const root of roots) {
  const collected = await collectNotices({ target: RUST_TARGETS[target], root, run });
  const directory = ADDON_ROOTS.includes(root) ? join(out, "addon") : out;
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
