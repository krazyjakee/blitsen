import { join } from "node:path";
import { buildAddon, repository } from "./build-addon.mjs";

const addon = await buildAddon({ purpose: "todo demo" });

console.log("Todo: type a task and press Enter, click a row to complete it, and use the");
console.log("filters. Expect the shell and its bands to arrive in sequence, a tick to draw");
console.log("and a line to cross the label as a row completes, rows to slide in and collapse");
console.log("out, and the progress fill and the filter thumb to travel rather than jump.");
const application = Bun.spawnSync({
  cmd: [
    process.execPath,
    join(repository, "packages/blitsen/bin/blitsen.mjs"),
    join(repository, "examples/todo"),
    "--width", "620", "--height", "720", "--title", "Blitsen Todo",
    ...process.argv.slice(2),
  ],
  cwd: repository,
  env: { ...process.env, BLITSEN_NATIVE_PATH: addon },
  stdout: "inherit",
  stderr: "inherit",
});
process.exitCode = application.exitCode;
