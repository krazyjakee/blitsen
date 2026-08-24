import { join } from "node:path";
import { buildAddon, repository } from "./build-addon.mjs";

const addon = await buildAddon({ purpose: "todo demo" });

console.log("Todo example: a persistent task list with priorities, search and filters.");
console.log("Tasks are saved locally between launches. Resize the app and use the custom");
console.log("borderless window controls to exercise its responsive desktop layout.");
const application = Bun.spawnSync({
  cmd: [
    process.execPath,
    join(repository, "packages/blitsen/bin/blitsen.mjs"),
    join(repository, "examples/todo"),
    "--width", "980", "--height", "760", "--title", "Blitsen Tasks",
    ...process.argv.slice(2),
  ],
  cwd: repository,
  env: { ...process.env, BLITSEN_NATIVE_PATH: addon },
  stdout: "inherit",
  stderr: "inherit",
});
process.exitCode = application.exitCode;
