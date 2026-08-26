import { cp, mkdir, rm } from "node:fs/promises";
import { spawn } from "node:child_process";

await rm("dist", { recursive: true, force: true });
await mkdir("dist", { recursive: true });

await new Promise((resolve, reject) => {
  const compiler = spawn(process.platform === "win32" ? "npx.cmd" : "npx", ["tsc"], {
    stdio: "inherit",
    shell: false,
  });
  compiler.on("error", reject);
  compiler.on("close", code => code === 0 ? resolve() : reject(new Error(`TypeScript exited with ${code}`)));
});

await Promise.all([
  cp("src/index.html", "dist/index.html"),
  cp("src/styles.css", "dist/styles.css"),
]);
