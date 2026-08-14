import { join } from "node:path";
import { buildAddon, repository } from "./build-addon.mjs";

const addon = await buildAddon({ purpose: "hardware demo" });

console.log("blitsen/os: the processor, memory, storage and OS identity of this machine,");
console.log("none of which the web platform can ask for.");
console.log("Expect four tabs. Processor names the real CPU and animates one meter per");
console.log("thread once a second; Memory and Storage report real capacity; System names");
console.log("the kernel and the boot time. The first processor reading is '—' by design:");
console.log("it measures since boot rather than an interval, so it is discarded.");
const application = Bun.spawnSync({
  cmd: [
    process.execPath,
    join(repository, "packages/blitsen/bin/blitsen.mjs"),
    join(repository, "examples/hardware"),
    "--width", "1180", "--height", "820", "--title", "Blitsen Hardware",
    ...process.argv.slice(2),
  ],
  cwd: repository,
  env: { ...process.env, BLITSEN_NATIVE_PATH: addon },
  stdout: "inherit",
  stderr: "inherit",
});
process.exitCode = application.exitCode;
