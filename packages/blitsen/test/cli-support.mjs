// Fixtures and helpers shared by the CLI test files.
//
// A plain module rather than a `.test.` one, so the runner does not collect it
// as a suite of its own.
import { copyFile, cp, mkdir, mkdtemp, readFile, readdir, realpath, rm, stat, writeFile }
  from "node:fs/promises";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { pathToFileURL } from "node:url";
import { packageVersion } from "../src/cli.mjs";

export const viteBase = join(import.meta.dir, "fixtures/vite-base");
export const configFixtures = join(import.meta.dir, "fixtures/config");
export const addonFixtures = join(import.meta.dir, "fixtures/addons");
export const icon = join(import.meta.dir, "fixtures/icons/app-256.png");
export const signHook = `sh ${join(import.meta.dir, "fixtures/sign/record-artifact.sh")}`;

// Nothing about the addon path can be proven with a stand-in file: dlopen is what
// decides. The fixtures are C, so a host compiler is what gates these tests, and
// the Windows toolchain needs an import library the fixture deliberately lacks.
export const compiler = process.platform === "win32" ? null : (Bun.which("cc") ?? Bun.which("gcc"));
export const engineAddon = join(import.meta.dir, "../../../target/release", {
  linux: "libblitsen_node.so", darwin: "libblitsen_node.dylib", win32: "blitsen_node.dll",
}[process.platform] ?? "missing");
// Proving a real load needs a real engine to launch. `bun run test:standalone`
// builds it; without it the exported-executable test declares itself skipped
// rather than pretending the export path was exercised.
export const engineBuilt = await Bun.file(engineAddon).exists();
export const platformPackages = join(import.meta.dir, "../../platforms");
export const cliVersion = await packageVersion();

// A real node_modules tree, so resolution is proven through the resolver every package
// manager writes for, against the committed manifests rather than a copy of them.
export async function withPlatformPackages(installed, run) {
  const directory = await mkdtemp(join(tmpdir(), "blitsen-runtime-"));
  try {
    for (const [target, { version, binary = true }] of Object.entries(installed)) {
      const packaged = join(directory, "node_modules/@blitsen", target);
      await mkdir(packaged, { recursive: true });
      const manifest = JSON.parse(
        await readFile(join(platformPackages, target, "package.json"), "utf8"));
      await writeFile(join(packaged, "package.json"), JSON.stringify({ ...manifest, version }));
      if (binary) await writeFile(join(packaged, "blitsen.node"), "// placeholder addon\n");
    }
    return await run({ directory, require: createRequire(join(directory, "resolve.mjs")) });
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}

export function compileAddon(directory, source = "greet.c", name = "greet.node") {
  const output = join(directory, name);
  const result = Bun.spawnSync({
    cmd: [compiler, "-shared", "-fPIC", "-o", output, join(addonFixtures, source),
      ...process.platform === "darwin" ? ["-Wl,-undefined,dynamic_lookup"] : []],
    stdout: "pipe",
    stderr: "pipe",
  });
  if (result.exitCode !== 0) throw new Error(`compiling ${source} failed: ${result.stderr}`);
  return output;
}

// A 64-byte ELF64 header is all describeNativeBinary reads, so a host-independent
// one stands in for an addon built somewhere else.
export function elfHeader({ machine = 0xb7, type = 3 } = {}) {
  const header = Buffer.alloc(64);
  header.write("\x7fELF", 0, "binary");
  header[4] = 2;
  header[5] = 1;
  header[6] = 1;
  header.writeUInt16LE(type, 16);
  header.writeUInt16LE(machine, 18);
  return header;
}


// A minimal shared library for any of the six targets.
//
// `describeNativeBinary` reads only the container header, so a synthetic one is
// enough to stand in for an addon built somewhere this test cannot build for —
// which is what makes the cross-target export path (#72) testable at all
// without six toolchains. It is deliberately not loadable: nothing in these
// tests requires it, and `bun build --compile` never opens the addon it embeds.
const NATIVE_STUB_MACHINES = {
  linux: { x64: 0x3e, arm64: 0xb7 },
  darwin: { x64: 0x01000007, arm64: 0x0100000c },
  win32: { x64: 0x8664, arm64: 0xaa64 },
};

export function nativeStub(target = `${process.platform}-${process.arch}`) {
  const platform = target.slice(0, target.lastIndexOf("-"));
  const architecture = target.slice(target.lastIndexOf("-") + 1);
  const machine = NATIVE_STUB_MACHINES[platform]?.[architecture];
  if (machine === undefined) throw new Error(`no native stub for ${target}`);
  if (platform === "linux") return elfHeader({ machine });
  if (platform === "darwin") {
    const header = Buffer.alloc(64);
    header.writeUInt32LE(0xfeedfacf, 0);
    header.writeUInt32LE(machine, 4);
    header.writeUInt32LE(6, 12); // MH_DYLIB
    return header;
  }
  const header = Buffer.alloc(0x100);
  header.write("MZ", 0, "binary");
  header.writeUInt32LE(0x80, 0x3c);
  header.writeUInt32LE(0x00004550, 0x80); // "PE\0\0"
  header.writeUInt16LE(machine, 0x84);
  header.writeUInt16LE(0x2000, 0x80 + 22); // IMAGE_FILE_DLL
  return header;
}

// The Phase 2 counterpart: a minimal *executable* for any of the six targets.
//
// The export links the target's own runtime by appending to it, so the artifact
// a cross-target build produces is this file plus a payload — which is exactly
// why the header has to be the target's. `file` reads no further than these
// bytes to name the format, which is what the cross-target test asserts on.
export function executableStub(target = `${process.platform}-${process.arch}`) {
  const platform = target.slice(0, target.lastIndexOf("-"));
  const architecture = target.slice(target.lastIndexOf("-") + 1);
  const machine = NATIVE_STUB_MACHINES[platform]?.[architecture];
  if (machine === undefined) throw new Error(`no executable stub for ${target}`);
  if (platform === "linux") {
    const header = elfHeader({ machine, type: 2 }); // ET_EXEC
    header.writeUInt32LE(1, 20); // EV_CURRENT, so `file` reads it as version 1
    return header;
  }
  if (platform === "darwin") {
    const header = Buffer.alloc(64);
    header.writeUInt32LE(0xfeedfacf, 0);
    header.writeUInt32LE(machine, 4);
    header.writeUInt32LE(2, 12); // MH_EXECUTE
    return header;
  }
  const header = Buffer.alloc(0x100);
  header.write("MZ", 0, "binary");
  header.writeUInt32LE(0x80, 0x3c);
  header.writeUInt32LE(0x00004550, 0x80); // "PE\0\0"
  header.writeUInt16LE(machine, 0x84);
  header.writeUInt16LE(0xf0, 0x80 + 20); // size of the optional header
  header.writeUInt16LE(0x0002, 0x80 + 22); // IMAGE_FILE_EXECUTABLE_IMAGE
  header.writeUInt16LE(0x20b, 0x80 + 24); // PE32+
  return header;
}

/** What the Phase 2 executable is called inside `target`'s platform package. */
export const phase2Name = target => `blitsen-runtime${target.startsWith("win32-") ? ".exe" : ""}`;

// Neither host opens what it links: Bun.build --compile refuses to start
// without the addon file but never loads it, and the Phase 2 link is an append
// to the runtime executable. So a placeholder for each is enough to exercise
// the whole export pipeline. The addon does have to be a shared library for the
// host, because the exporter refuses to link a runtime built for another target
// (#72) — a check that only means anything if it is on for every build,
// including these. Stubbing the Phase 2 runtime rather than reaching for the
// one this checkout built also keeps these tests off a 37 MB copy per export.
export async function withStubbedExport(run) {
  const directory = await mkdtemp(join(tmpdir(), "blitsen-export-test-"));
  const nativePath = join(directory, "blitsen.node");
  const runtimePath = join(directory, phase2Name(`${process.platform}-${process.arch}`));
  await writeFile(nativePath, nativeStub());
  await writeFile(runtimePath, executableStub());
  const previous = process.env.BLITSEN_RUNTIME_PATH;
  process.env.BLITSEN_RUNTIME_PATH = runtimePath;
  try {
    return await run({ directory, nativePath, runtimePath, outfile: join(directory, "App") });
  } finally {
    if (previous === undefined) delete process.env.BLITSEN_RUNTIME_PATH;
    else process.env.BLITSEN_RUNTIME_PATH = previous;
    await rm(directory, { recursive: true, force: true });
  }
}

// Step ⑤ is file generation over an already-linked artifact, so the macOS and
// Windows layouts are exercised on any host by handing it a stand-in executable.
export async function withArtifact(run, name = "Pong") {
  const directory = await mkdtemp(join(tmpdir(), "blitsen-package-test-"));
  const executable = join(directory, name);
  await writeFile(executable, "#!/bin/sh\nexit 0\n", { mode: 0o755 });
  try {
    return await run({ directory, executable });
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}

export function capture() {
  const lines = [];
  return {
    lines,
    output: {
      log: (line) => lines.push(["out", line]),
      error: (line) => lines.push(["err", line]),
    },
  };
}
