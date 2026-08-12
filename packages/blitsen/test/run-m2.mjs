import { join } from "node:path";
import { buildAddon, repository } from "./build-addon.mjs";

const addon = await buildAddon({ purpose: "M2 acceptance" });

const acceptance = Bun.spawnSync({
  cmd: [process.execPath, join(import.meta.dir, "m2-acceptance.mjs"), addon],
  cwd: repository,
  stdout: "inherit",
  stderr: "inherit",
});
process.exitCode = acceptance.exitCode;
