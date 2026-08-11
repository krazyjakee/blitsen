// One build, every artifact metric: installed and compressed size with a
// component breakdown (issue #55) plus startup timing and resident memory
// (issue #49). Both gates consume the same record so CI builds the export once.
import { copyFile, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { buildStandalone } from "../src/export.mjs";

export const repository = resolve(import.meta.dir, "../../..");

const libraryName = {
  linux: "libblitsen_node.so",
  darwin: "libblitsen_node.dylib",
  win32: "blitsen_node.dll",
}[process.platform];

export const platformKey = `${process.platform}-${process.arch}`;

function commandVersion(cmd) {
  const result = Bun.spawnSync({ cmd, stdout: "pipe", stderr: "ignore" });
  return result.exitCode === 0 ? result.stdout.toString().trim() : null;
}

function median(values) {
  const sorted = [...values].sort((left, right) => left - right);
  const middle = sorted.length >> 1;
  return sorted.length % 2 === 1 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
}

function summarize(values) {
  return {
    runs: values.length,
    min: Math.round(Math.min(...values) * 10) / 10,
    median: Math.round(median(values) * 10) / 10,
    max: Math.round(Math.max(...values) * 10) / 10,
  };
}

// The first spawn of a freshly written 100+ MB executable pays for page cache
// misses that no later run repeats, so it is warm-up rather than a sample.
function timeSpawns(cmd, options, runs) {
  const samples = [];
  for (let index = 0; index <= runs; index += 1) {
    const started = performance.now();
    const result = Bun.spawnSync({ ...options, cmd, stdout: "pipe", stderr: "pipe" });
    const elapsed = performance.now() - started;
    if (result.exitCode !== 0) {
      throw new Error(`${cmd[0]} exited ${result.exitCode}: ${result.stderr.toString()}`);
    }
    if (index > 0) samples.push(elapsed);
  }
  return summarize(samples);
}

// VmHWM is the kernel's own peak, so a single late read is authoritative; VmRSS
// is sampled to find the steady state a long-lived run settles at.
async function sampleResident(cmd, options, durationMs = 0) {
  const child = Bun.spawn({ ...options, cmd, stdout: "pipe", stderr: "pipe" });
  const deadline = durationMs > 0 ? setTimeout(() => child.kill(), durationMs) : null;
  const resident = [];
  let peak = 0;
  let running = true;
  const sampler = (async () => {
    while (running) {
      const status = await readFile(`/proc/${child.pid}/status`, "utf8").catch(() => null);
      if (status) {
        const rss = Number(status.match(/^VmRSS:\s+(\d+) kB/m)?.[1] ?? 0) * 1024;
        const hwm = Number(status.match(/^VmHWM:\s+(\d+) kB/m)?.[1] ?? 0) * 1024;
        if (rss > 0) resident.push({ at: performance.now(), bytes: rss });
        if (hwm > peak) peak = hwm;
      }
      await Bun.sleep(5);
    }
  })();
  const [exitCode, stdout, stderr] = await Promise.all([
    child.exited,
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
  ]);
  running = false;
  if (deadline) clearTimeout(deadline);
  await sampler;
  if (exitCode !== 0 && !deadline) throw new Error(`${cmd[0]} exited ${exitCode}: ${stderr}`);
  return { peak, resident, stdout };
}

// The last quarter of the samples: the window is open and the animation loop is
// running, so allocation from parse and first layout has already settled.
function steadyResident(resident) {
  if (resident.length < 4) return null;
  const tail = resident.slice(Math.floor(resident.length * 0.75)).map(sample => sample.bytes);
  return Math.round(median(tail));
}

async function compressedBytes(outfile) {
  if (!Bun.which("gzip")) return null;
  // gzip -9 for comparability with the sizes recorded in docs/PRODUCT.md §9.
  const result = Bun.spawnSync({
    cmd: ["gzip", "-9", "-c", outfile],
    stdout: "pipe",
    stderr: "pipe",
    maxBuffer: 1024 * 1024 * 1024,
  });
  if (result.exitCode !== 0) throw new Error(`gzip exited ${result.exitCode}`);
  return result.stdout.length;
}

// Windowed measurement is opt-in rather than display-detected: it is the only
// real (non-proxy) reading of P2 and P3, but it needs a live desktop session,
// so CI and unattended runs must never depend on it.
export async function measureExport({ runs = 5, windowed = false } = {}) {
  if (!libraryName) throw new Error(`unsupported measurement target: ${process.platform}`);
  if (process.platform !== "linux") throw new Error("resident memory sampling requires /proc");
  if (windowed && !(process.env.DISPLAY || process.env.WAYLAND_DISPLAY)) {
    throw new Error("--windowed needs DISPLAY or WAYLAND_DISPLAY");
  }

  const build = Bun.spawnSync({
    cmd: ["cargo", "build", "--release", "-p", "blitsen-node"],
    cwd: repository,
    stdout: "inherit",
    stderr: "inherit",
  });
  if (build.exitCode !== 0) throw new Error(`cargo build exited ${build.exitCode}`);

  const directory = await mkdtemp(join(tmpdir(), "blitsen-measure-"));
  try {
    const addon = join(directory, "blitsen.node");
    await copyFile(join(repository, "target", "release", libraryName), addon);
    const outfile = join(directory, "pong");
    const root = join(repository, "examples/pong");
    const result = await buildStandalone({
      root, width: 720, height: 520, title: "Blitsen Pong", outfile,
    }, addon);

    // A do-nothing compiled entrypoint isolates what the Bun runtime itself
    // costs, in bytes and in process start, from what Blitsen adds to it.
    const floorSource = join(directory, "floor.mjs");
    const floorExecutable = join(directory, "floor");
    await writeFile(floorSource, "process.exitCode = 0;\n");
    const floorBuild = await Bun.build({
      entrypoints: [floorSource],
      compile: { outfile: floorExecutable },
    });
    if (!floorBuild.success) throw new Error("failed to compile the Bun runtime floor");

    let applicationBytes = 0;
    for (const asset of result.manifest) {
      applicationBytes += (await stat(join(root, ...asset.path.split("/")))).size;
    }
    const bunRuntimeBytes = (await stat(floorExecutable)).size;
    const nativeAddonBytes = (await stat(addon)).size;

    const checkEnvironment = { BLITSEN_STANDALONE_CHECK: "1", BLITSEN_STANDALONE_CHECK_DELAY: "0", PATH: "" };
    const headless = timeSpawns([outfile], { cwd: directory, env: checkEnvironment }, runs);
    const floor = timeSpawns([floorExecutable], { cwd: directory, env: { PATH: "" } }, runs);

    // A longer delay keeps the process alive past the raster so the sampler is
    // guaranteed a read; peak comes from VmHWM, so the delay does not inflate it.
    const headlessMemory = await sampleResident([outfile], {
      cwd: directory,
      env: { ...checkEnvironment, BLITSEN_STANDALONE_CHECK_DELAY: "150" },
    });

    let windowedFirstFrameMs = null;
    let windowedMemory = null;
    if (windowed) {
      windowedFirstFrameMs = timeSpawns([outfile], {
        cwd: directory,
        env: { ...process.env, BLITSEN_STANDALONE_FRAMES: "1", BLITSEN_STANDALONE_WARMUP_FRAMES: "0" },
      }, runs);
      // Time-bounded rather than frame-bounded: a throttled or occluded window
      // pumps frames arbitrarily slowly, and idle RAM is about seconds, not frames.
      windowedMemory = await sampleResident([outfile], {
        cwd: directory,
        env: { ...process.env },
      }, 5000);
    }

    const revision = Bun.spawnSync({
      cmd: ["git", "rev-parse", "--short", "HEAD"],
      cwd: repository,
      stdout: "pipe",
      stderr: "ignore",
    });

    return {
      recordedAt: new Date().toISOString(),
      commit: revision.exitCode === 0 ? revision.stdout.toString().trim() : null,
      platform: platformKey,
      environment: process.env.CI ? "ci" : "local",
      windowed,
      bun: Bun.version,
      rustc: commandVersion(["rustc", "--version"]),
      size: {
        installedBytes: result.bytes,
        compressedBytes: await compressedBytes(outfile),
        components: {
          bunRuntimeBytes,
          nativeAddonBytes,
          applicationBytes,
          packagingBytes: result.bytes - bunRuntimeBytes - nativeAddonBytes - applicationBytes,
        },
      },
      startup: {
        bunRuntimeFloorMs: floor,
        headlessFirstPaintMs: headless,
        windowedFirstFrameMs,
      },
      memory: {
        headlessPeakBytes: headlessMemory.peak,
        windowedPeakBytes: windowedMemory?.peak ?? null,
        windowedSteadyBytes: windowedMemory ? steadyResident(windowedMemory.resident) : null,
      },
    };
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}

export function formatBytes(bytes) {
  if (Math.abs(bytes) < 1_000) return `${bytes} B`;
  if (Math.abs(bytes) < 1_000_000) return `${(bytes / 1_000).toFixed(1)} kB`;
  return `${(bytes / 1_000_000).toFixed(1)} MB`;
}

if (import.meta.main) {
  const argv = process.argv.slice(2);
  const outIndex = argv.indexOf("--out");
  const runsIndex = argv.indexOf("--runs");
  const record = await measureExport({
    runs: runsIndex < 0 ? undefined : Number(argv[runsIndex + 1]),
    windowed: argv.includes("--windowed"),
  });
  const serialized = `${JSON.stringify(record, null, 2)}\n`;
  if (outIndex < 0) process.stdout.write(serialized);
  else await writeFile(argv[outIndex + 1], serialized);
}
