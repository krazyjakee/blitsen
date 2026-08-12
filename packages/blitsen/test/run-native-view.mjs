import { join } from "node:path";
import { buildAddon, repository } from "./build-addon.mjs";

const addon = await buildAddon({ purpose: "native-view demo" });

console.log("blitsen-view: an application-drawn surface composited into the DOM frame.");
console.log("Expect an animating gradient with its own rounded corners, the red DOM underlay");
console.log("showing around and behind it, and a DOM chip drawn on top of it.");
const application = Bun.spawnSync({
  cmd: [
    process.execPath,
    join(repository, "packages/blitsen/bin/blitsen.mjs"),
    join(repository, "examples/native-view"),
    "--width", "700", "--height", "500", "--title", "blitsen-view",
    ...process.argv.slice(2),
  ],
  cwd: repository,
  env: { ...process.env, BLITSEN_NATIVE_PATH: addon },
  stdout: "inherit",
  stderr: "inherit",
});
process.exitCode = application.exitCode;
