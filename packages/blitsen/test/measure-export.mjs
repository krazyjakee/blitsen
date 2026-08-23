// One build, every artifact metric: installed and compressed size with a
// component breakdown (issue #55) plus startup timing and resident memory
// (issue #49). Both gates consume the same record so CI builds the export once.
import { mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { buildStandalone } from "../src/export.mjs";
import { buildAddon, repository } from "./build-addon.mjs";
import { pinnedPhase2Runtime } from "./measurement-runtime.mjs";

export { repository } from "./build-addon.mjs";

export const platformKey = `${process.platform}-${process.arch}`;

export function measurementStorageEnvironment(platform, directory) {
  if (platform === "win32") {
    return {
      APPDATA: join(directory, "app-data"),
      LOCALAPPDATA: join(directory, "local-data"),
    };
  }
  if (platform === "darwin") return { HOME: join(directory, "home") };
  return { XDG_DATA_HOME: join(directory, "data") };
}

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

// Level 9 in process rather than a `gzip -9` subprocess: `gzip` is not on PATH
// on a Windows runner, and the old code returned null there, so the download
// figure silently went missing on the platform it was added to measure (#123).
// Bun's deflate is ~0.3% tighter than GNU gzip at the same level, so this is a
// step in the recorded series and not a comparison against docs/PRODUCT.md §9.
async function compressedBytes(outfile) {
  return Bun.gzipSync(new Uint8Array(await Bun.file(outfile).arrayBuffer()), { level: 9 }).length;
}

// Windowed measurement is opt-in rather than display-detected: it is the only
// real (non-proxy) reading of P2 and P3, but it needs a live desktop session,
// so CI and unattended runs must never depend on it.
export async function measureExport({ runs = 5, windowed = false } = {}) {
  if (windowed && !(process.env.DISPLAY || process.env.WAYLAND_DISPLAY)) {
    throw new Error("--windowed needs DISPLAY or WAYLAND_DISPLAY");
  }
  // Size and startup are portable; resident memory is read out of /proc, which
  // is Linux alone. P1 is one size budget across six platforms (#123), so the
  // absence of a memory reading must not cost the size reading its target —
  // this reports `memory: null` off Linux rather than refusing to measure.
  const canSampleResident = process.platform === "linux";
  if (windowed && !canSampleResident) {
    throw new Error("--windowed reads resident memory, which needs /proc");
  }

  // Resolve before building anything. Without this explicit pin an installed
  // @blitsen platform package can outrank target/release and make a local run
  // silently weigh the previous published runtime (#89).
  const measuredRuntime = await pinnedPhase2Runtime();

  const directory = await mkdtemp(join(tmpdir(), "blitsen-measure-"));
  try {
    const addon = await buildAddon({ purpose: "measurement target", release: true, into: directory });
    const root = join(repository, "examples/pong");
    const result = await buildStandalone({
      root, width: 720, height: 520, title: "Blitsen Pong", outfile: join(directory, "pong"),
    }, addon);
    // What was written, not what was asked for: `bun build --compile` appends
    // `.exe` on Windows, so the requested name is a path to nothing there.
    // Spawning the requested one failed with ENOENT on Windows alone.
    const outfile = result.outfile;

    // A do-nothing compiled entrypoint isolates what the Bun runtime itself
    // costs, in bytes and in process start, from what Blitsen adds to it.
    const floorSource = join(directory, "floor.mjs");
    const requestedFloor = join(directory, "floor");
    await writeFile(floorSource, "process.exitCode = 0;\n");
    const floorBuild = await Bun.build({
      entrypoints: [floorSource],
      compile: { outfile: requestedFloor },
    });
    if (!floorBuild.success) throw new Error("failed to compile the Bun runtime floor");
    const floorExecutable = await stat(requestedFloor)
      .then(() => requestedFloor, () => `${requestedFloor}.exe`);

    let applicationBytes = 0;
    for (const asset of result.manifest) {
      applicationBytes += (await stat(join(root, ...asset.path.split("/")))).size;
    }
    // What the export actually links into, which is the largest component and
    // is a different artifact per host: Blitsen's own runtime executable on
    // Phase 2, a copy of Bun plus an embedded addon on Phase 1. Measured from
    // the linked file rather than assumed, so the breakdown adds up either way.
    const hostRuntimeBytes = result.host === "blitsen"
      ? (await stat(measuredRuntime.path)).size
      : (await stat(floorExecutable)).size;
    const nativeAddonBytes = result.host === "blitsen" ? 0 : (await stat(addon)).size;

    // Keep benchmark runs hermetic without blanking the absolute platform data
    // directory durable localStorage now requires. In particular, macOS uses
    // HOME to locate Library/Application Support and rejects HOME="".
    const checkEnvironment = {
      ...measurementStorageEnvironment(process.platform, directory),
      BLITSEN_STANDALONE_CHECK: "1",
      BLITSEN_STANDALONE_CHECK_DELAY: "0",
      PATH: "",
    };
    const headless = timeSpawns([outfile], { cwd: directory, env: checkEnvironment }, runs);
    const floor = timeSpawns([floorExecutable], { cwd: directory, env: { PATH: "" } }, runs);

    // A longer delay keeps the process alive past the raster so the sampler is
    // guaranteed a read; peak comes from VmHWM, so the delay does not inflate it.
    const headlessMemory = canSampleResident
      ? await sampleResident([outfile], {
        cwd: directory,
        env: { ...checkEnvironment, BLITSEN_STANDALONE_CHECK_DELAY: "150" },
      })
      : null;

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
      // Which host linked it: two hosts produce two different artifacts, and a
      // delta across the swap is a migration rather than a regression.
      host: result.host,
      runtime: { path: measuredRuntime.path, source: measuredRuntime.source },
      size: {
        installedBytes: result.bytes,
        compressedBytes: await compressedBytes(outfile),
        components: {
          hostRuntimeBytes,
          nativeAddonBytes,
          applicationBytes,
          packagingBytes: result.bytes - hostRuntimeBytes - nativeAddonBytes - applicationBytes,
        },
      },
      startup: {
        bunRuntimeFloorMs: floor,
        headlessFirstPaintMs: headless,
        windowedFirstFrameMs,
      },
      // Null, not zero: this platform has no /proc, so idle RAM was not read
      // here. A zero would average into the series as a measurement.
      memory: headlessMemory && {
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
