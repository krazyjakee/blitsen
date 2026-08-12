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

// Bun.build --compile refuses to start without the addon file, but never loads
// it, so a placeholder is enough to exercise the whole export pipeline.
export async function withStubbedExport(run) {
  const directory = await mkdtemp(join(tmpdir(), "blitsen-export-test-"));
  const nativePath = join(directory, "blitsen.node");
  await writeFile(nativePath, "// placeholder addon\n");
  try {
    return await run({ directory, nativePath, outfile: join(directory, "App") });
  } finally {
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
