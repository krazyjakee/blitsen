import { strict as assert } from "node:assert";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { buildStandalone } from "../src/export.mjs";
import { buildAddon, repository } from "./build-addon.mjs";


const testDirectory = await mkdtemp(join(tmpdir(), "blitsen-standalone-test-"));
try {
  const addon = await buildAddon({ purpose: "standalone target", release: true,
    into: testDirectory });
  const outfile = join(testDirectory, process.platform === "win32" ? "pong.exe" : "pong");
  const result = await buildStandalone({
    root: join(repository, "examples/pong"),
    width: 720,
    height: 520,
    title: "Blitsen Pong",
    outfile,
  }, addon);
  assert.equal(result.assets, 3);
  const check = Bun.spawnSync({
    cmd: [outfile],
    cwd: testDirectory,
    env: { BLITSEN_STANDALONE_CHECK: "1", PATH: "" },
    stdout: "pipe",
    stderr: "pipe",
  });
  assert.equal(check.exitCode, 0, check.stderr.toString());
  assert.match(check.stdout.toString(), /standalone check passed \(3 embedded assets\)/);

  const sideLoaded = await buildStandalone({
    root: join(repository, "examples/pong"),
    width: 720,
    height: 520,
    title: "Blitsen Pong",
    outfile: join(testDirectory, "pong-side-loaded"),
    assets: "side-loaded",
  }, addon);
  // Named after the executable the export actually produced: a Windows target
  // is `.exe`, so the sidecar is `pong-side-loaded.exe.assets` there. Derived
  // rather than spelled out, which is what made this a Linux-only assertion.
  assert.equal(sideLoaded.assetDirectory, `${sideLoaded.outfile}.assets`);
  assert.ok(sideLoaded.outfile.startsWith(join(testDirectory, "pong-side-loaded")),
    `the export landed at ${sideLoaded.outfile}`);
  // Run from an unrelated working directory: assets resolve beside the executable.
  const sideCheck = Bun.spawnSync({
    cmd: [sideLoaded.outfile],
    cwd: repository,
    env: { BLITSEN_STANDALONE_CHECK: "1", PATH: "" },
    stdout: "pipe",
    stderr: "pipe",
  });
  assert.equal(sideCheck.exitCode, 0, sideCheck.stderr.toString());
  assert.match(sideCheck.stdout.toString(), /standalone check passed \(3 side-loaded assets\)/);

  let nativeCadence = null;
  if (process.env.DISPLAY || process.env.WAYLAND_DISPLAY) {
    const nativeFrames = Bun.spawnSync({
      cmd: [outfile],
      cwd: testDirectory,
      env: { ...process.env, BLITSEN_STANDALONE_FRAMES: "120",
        BLITSEN_STANDALONE_WARMUP_FRAMES: "30", PATH: "" },
      stdout: "pipe",
      stderr: "pipe",
    });
    assert.equal(nativeFrames.exitCode, 0, nativeFrames.stderr.toString());
    const measured = nativeFrames.stdout.toString().match(/120 frames at ([\d.]+) fps/);
    assert(measured, nativeFrames.stdout.toString());
    nativeCadence = Number(measured[1]);
    assert(nativeCadence >= 58, `native frame cadence fell below the 60 Hz budget: ${measured[1]}`);
  }
  console.log(`Standalone Pong verified: ${result.bytes} bytes${nativeCadence === null ? "" : `, ${nativeCadence} fps`}`);
} finally {
  await rm(testDirectory, { recursive: true, force: true });
}
