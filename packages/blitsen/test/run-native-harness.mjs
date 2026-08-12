import { join } from "node:path";
import { buildAddon, repository } from "./build-addon.mjs";

const addon = await buildAddon({ purpose: "native harness" });

const harness = Bun.spawnSync({
  cmd: [process.execPath, join(import.meta.dir, "native-harness.mjs"), addon],
  cwd: repository,
  stdout: "inherit",
  stderr: "inherit",
});
process.exitCode = harness.exitCode;
