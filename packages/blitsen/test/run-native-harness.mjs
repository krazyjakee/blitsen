import { join } from "node:path";
import { buildAddon, repository } from "./build-addon.mjs";

const addon = await buildAddon({ purpose: "native harness" });

const harness = Bun.spawnSync({
  cmd: [process.execPath, join(import.meta.dir, "native-harness.mjs"), addon],
  cwd: repository,
  // Audio renders offline here, so the harness asserts on samples and no test
  // run opens an output device. Set in the environment rather than on
  // `process.env` in the child, because the bridge reads it natively when it
  // installs and a JavaScript assignment does not reach that.
  env: { ...process.env, BLITSEN_AUDIO_OFFLINE: "1" },
  stdout: "inherit",
  stderr: "inherit",
});
process.exitCode = harness.exitCode;
