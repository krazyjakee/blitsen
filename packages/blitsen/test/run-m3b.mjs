import { mkdtemp, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { buildAddon, repository } from "./build-addon.mjs";

const example = join(repository, "examples/vite-react");

for (const command of [
  [process.execPath, "install", "--frozen-lockfile"],
  [process.execPath, "run", "build"],
]) {
  const result = Bun.spawnSync({ cmd: command, cwd: example, stdout: "inherit", stderr: "inherit" });
  if (result.exitCode !== 0) process.exit(result.exitCode);
}

const dist = join(example, "dist");
const addon = await buildAddon({ purpose: "M3b", release: true });
const temporary = await mkdtemp(join(tmpdir(), "blitsen-m3b-"));
const executable = join(temporary, process.platform === "win32" ? "ReactAcceptance.exe" : "ReactAcceptance");
const cli = join(example, "node_modules/blitsen/bin/blitsen.mjs");
const storageEnvironment = process.platform === "win32"
  ? { APPDATA: join(temporary, "app-data"), LOCALAPPDATA: join(temporary, "local-data") }
  : process.platform === "darwin"
    ? { HOME: join(temporary, "home") }
    : { XDG_DATA_HOME: join(temporary, "data") };

try {
  const cliEnvironment = { ...process.env, BLITSEN_NATIVE_PATH: addon };
  const doctor = Bun.spawnSync({
    cmd: [process.execPath, cli, "doctor", dist, "--json"],
    cwd: example,
    env: cliEnvironment,
    stdout: "pipe",
    stderr: "pipe",
  });
  if (doctor.exitCode !== 0) {
    process.stderr.write(doctor.stderr.toString());
    throw new Error(`blitsen doctor exited ${doctor.exitCode}`);
  }
  const report = JSON.parse(doctor.stdout.toString());
  if (report.errors > 0) throw new Error(`React output has ${report.errors} compatibility errors`);

  const buildExport = Bun.spawnSync({
    cmd: [process.execPath, cli, "build", dist, "--outfile", executable],
    cwd: example,
    env: cliEnvironment,
    stdout: "pipe",
    stderr: "pipe",
  });
  if (buildExport.exitCode !== 0) {
    process.stderr.write(buildExport.stderr.toString());
    throw new Error(`blitsen build exited ${buildExport.exitCode}`);
  }
  const run = Bun.spawnSync({
    cmd: [executable],
    cwd: temporary,
    env: {
      ...storageEnvironment,
      PATH: "",
      BLITSEN_STANDALONE_CHECK: "1",
      BLITSEN_STANDALONE_CHECK_DELAY: "250",
      BLITSEN_STANDALONE_CHECK_SCRIPT: `(() => {
        const shell = document.querySelector('[data-react-ready="true"]');
        const button = document.getElementById('increment');
        if (!shell || !button) throw new Error('React did not mount from the Vite bundle');
        button.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
      })()`,
      BLITSEN_STANDALONE_CHECK_ASSERT: `(() => {
        const clicked = document.getElementById('increment')?.getAttribute('data-clicked');
        const value = document.getElementById('count')?.textContent;
        if (value !== '1') throw new Error('React delegated click state was ' + String(value) +
          ' (handler=' + String(clicked) + ')');
      })()`,
    },
    stdout: "pipe",
    stderr: "pipe",
  });
  if (run.exitCode !== 0) {
    process.stderr.write(run.stderr.toString());
    throw new Error(`React standalone acceptance exited ${run.exitCode}`);
  }
  const output = run.stdout.toString();
  if (!output.includes("standalone check passed")) throw new Error("standalone check did not finish");
  const bytes = (await stat(executable)).size;
  console.log(`M3b passed: Vite output had ${report.errors} errors/${report.warnings} warnings; `
    + `React mounted and handled input from a 3-asset, ${bytes}-byte `
    + "standalone export.");
} finally {
  await rm(temporary, { recursive: true, force: true });
}
