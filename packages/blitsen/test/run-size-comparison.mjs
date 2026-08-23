// Issue #89: put Blitsen, Electron and Tauri on the same runner and package the
// exact same bare HTML document. This measures installed runnable output, not an
// installer: Blitsen and Tauri produce one executable, while Electron needs its
// complete packaged directory because Chromium is part of the application.
import { createHash } from "node:crypto";
import { createRequire } from "node:module";
import { cp, lstat, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { gzipSync } from "node:zlib";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { BARE_APP } from "./bare-app.mjs";
import { formatBytes } from "./measure-export.mjs";

export const comparisonFixture = join(import.meta.dir, "fixtures/size-comparison");

export async function footprint(path) {
  const entry = await lstat(path);
  if (entry.isSymbolicLink()) return { installedBytes: entry.size, compressedBytes: 0, files: 0 };
  if (entry.isFile()) {
    const contents = await readFile(path);
    return {
      installedBytes: entry.size,
      compressedBytes: gzipSync(contents, { level: 9 }).length,
      files: 1,
    };
  }
  if (!entry.isDirectory()) return { installedBytes: 0, compressedBytes: 0, files: 0 };
  const total = { installedBytes: 0, compressedBytes: 0, files: 0 };
  for (const child of await readdir(path)) {
    const measured = await footprint(join(path, child));
    total.installedBytes += measured.installedBytes;
    total.compressedBytes += measured.compressedBytes;
    total.files += measured.files;
  }
  return total;
}

function run(command, options = {}) {
  const result = Bun.spawnSync({ cmd: command, stdout: "inherit", stderr: "inherit", ...options });
  if (result.exitCode !== 0) throw new Error(`${command[0]} exited ${result.exitCode}`);
}

async function electronBuild(directory) {
  const source = join(directory, "electron-source");
  await cp(join(comparisonFixture, "electron"), source, { recursive: true });
  await writeFile(join(source, "index.html"), BARE_APP);
  const fixtureRequire = createRequire(join(comparisonFixture, "package.json"));
  const { packager } = fixtureRequire("@electron/packager");
  const [output] = await packager({
    dir: source,
    out: join(directory, "electron-output"),
    name: "BareElectron",
    appVersion: "0.0.0",
    electronVersion: "43.4.1",
    platform: process.platform,
    arch: process.arch,
    asar: true,
    overwrite: true,
    prune: true,
  });
  return output;
}

async function tauriBuild(directory) {
  // Keep Cargo's target beside the fixture so a failed local comparison can be
  // retried without recompiling Tauri's whole graph. It is build output and is
  // ignored; CI still starts clean and measures only the final executable.
  const target = join(comparisonFixture, "tauri/target");
  const cli = join(comparisonFixture, "node_modules/@tauri-apps/cli/tauri.js");
  run([process.execPath, cli, "build", "--ci", "--no-bundle", "--", "--locked"], {
    cwd: join(comparisonFixture, "tauri"),
    env: { ...process.env, CARGO_TARGET_DIR: target },
  });
  return join(target, "release", `blitsen-size-tauri${process.platform === "win32" ? ".exe" : ""}`);
}

export function comparisonSummary(record) {
  const rows = Object.entries(record.frameworks).map(([name, measurement]) =>
    `| ${name} | ${formatBytes(measurement.installedBytes)} `
      + `| ${formatBytes(measurement.compressedBytes)} | ${measurement.files} |`);
  return [
    `### Bare desktop size comparison — ${record.platform}`,
    "",
    "One HTML document, one 800×600 native window, release builds on this runner. Installed is the",
    "complete runnable output; compressed is the sum of gzip-9 over its regular files, not an installer.",
    "",
    "| framework | installed | filewise gzip-9 | regular files |",
    "| --- | ---: | ---: | ---: |",
    ...rows,
    "",
    "> Tauri uses the operating system WebView, whose bytes are not in its executable. Electron ships",
    "> Chromium in its directory; Blitsen ships its renderer and JavaScript engine in its executable.",
  ].join("\n");
}

export async function measureComparison(blitsenRecord) {
  if (blitsenRecord.application !== "bare") throw new Error("the Blitsen input is not the bare app");
  const directory = await mkdtemp(join(tmpdir(), "blitsen-size-comparison-"));
  try {
    const [electron, tauri] = await Promise.all([
      electronBuild(directory),
      Promise.resolve().then(() => tauriBuild(directory)),
    ]);
    return {
      recordedAt: new Date().toISOString(),
      commit: blitsenRecord.commit ?? null,
      platform: `${process.platform}-${process.arch}`,
      application: "bare",
      applicationSha256: createHash("sha256").update(BARE_APP).digest("hex"),
      boundary: "installed runnable output; filewise gzip-9 is a compression proxy",
      versions: { blitsen: "checkout", electron: "43.4.1", tauriCli: "2.11.4", tauri: "2.11.5" },
      frameworks: {
        blitsen: {
          installedBytes: blitsenRecord.phase2.bytes,
          compressedBytes: blitsenRecord.phase2.gzip,
          files: 1,
        },
        electron: await footprint(electron),
        tauri: await footprint(tauri),
      },
      caveats: {
        electron: "The packaged directory includes Chromium and Electron.",
        tauri: "The executable relies on the operating system WebView; those shared system bytes are excluded.",
        blitsen: "The executable includes Blitsen's renderer and QuickJS-ng.",
      },
    };
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}

if (import.meta.main) {
  const argv = process.argv.slice(2);
  const inputIndex = argv.indexOf("--blitsen");
  const outIndex = argv.indexOf("--out");
  if (inputIndex < 0 || !argv[inputIndex + 1]) {
    throw new Error("size:compare requires --blitsen <phase2-size.json>");
  }
  const record = await measureComparison(JSON.parse(await readFile(argv[inputIndex + 1], "utf8")));
  const serialized = `${JSON.stringify(record, null, 2)}\n`;
  if (outIndex < 0) process.stdout.write(serialized);
  else await writeFile(argv[outIndex + 1], serialized);
  const summary = comparisonSummary(record);
  console.log(summary);
  if (process.env.GITHUB_STEP_SUMMARY) {
    await writeFile(process.env.GITHUB_STEP_SUMMARY, `${summary}\n\n`, { flag: "a" });
  }
}
