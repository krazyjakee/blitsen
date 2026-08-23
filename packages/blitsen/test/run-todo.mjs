import { join } from "node:path";
import { buildAddon, repository } from "./build-addon.mjs";

const addon = await buildAddon({ purpose: "todo demo" });

console.log("Todo scale demo: 10,000 tasks with a bounded virtual list, search and filters.");
console.log("Resize, scroll quickly, and use the custom borderless window controls. The header,");
console.log("toolbar and status bar should stay fixed while only the visible rows are mounted.");
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
