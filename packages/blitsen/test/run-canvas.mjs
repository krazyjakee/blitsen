import { join } from "node:path";
import { buildAddon, repository } from "./build-addon.mjs";

const addon = await buildAddon({ purpose: "canvas demo" });

console.log("canvas 2D: paths, gradients, patterns, text, images and compositing, drawn into");
console.log("the same frame as the DOM. Expect six orbiting discs inside a dashed ring, a");
console.log("patterned band clipped to a rounded rectangle, three labels at three text anchors,");
console.log("a DOM chip on top reporting a pixel read back with getImageData, and a thumbnail");
console.log("below the stage that is the canvas encoded through toDataURL.");
const application = Bun.spawnSync({
  cmd: [
    process.execPath,
    join(repository, "packages/blitsen/bin/blitsen.mjs"),
    join(repository, "examples/canvas"),
    "--width", "700", "--height", "620", "--title", "canvas 2D",
    ...process.argv.slice(2),
  ],
  cwd: repository,
  env: { ...process.env, BLITSEN_NATIVE_PATH: addon },
  stdout: "inherit",
  stderr: "inherit",
});
process.exitCode = application.exitCode;
