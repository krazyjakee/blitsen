import { strict as assert } from "node:assert";
import { spawnSync } from "node:child_process";
import { mkdir, mkdtemp, readdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const packageDirectory = join(import.meta.dirname, "..");
const scratch = await mkdtemp(join(tmpdir(), "blitsen-node-smoke-"));
const project = join(scratch, "project");
const cache = join(scratch, "npm-cache");
const npm = process.platform === "win32" ? "npm.cmd" : "npm";

function run(command, args, cwd = packageDirectory) {
  const result = spawnSync(command, args, { cwd, encoding: "utf8" });
  if (result.status !== 0) {
    throw new Error(`${command} ${args.join(" ")} exited ${result.status ?? "without a status"}\n`
      + `${result.stdout}${result.stderr}`);
  }
  return result.stdout.trim();
}

try {
  await mkdir(project);
  await mkdir(cache);
  await writeFile(join(project, "package.json"), '{"private":true,"type":"module"}\n');

  run(npm, ["pack", packageDirectory, "--pack-destination", scratch, "--silent"]);
  const tarballs = (await readdir(scratch)).filter(file => file.endsWith(".tgz"));
  assert.equal(tarballs.length, 1, `npm pack produced ${tarballs.length} tarballs`);
  const tarball = join(scratch, tarballs[0]);

  // Offline plus a new cache proves this install uses only the tarball. Optional
  // platform packages are deliberately omitted; the resolver gets a local
  // stand-in below, so this smoke never depends on a published release.
  run(npm, [
    "install", tarball, "--ignore-scripts", "--omit=optional", "--no-audit", "--no-fund",
    "--no-package-lock", "--offline", "--cache", cache,
  ], project);

  const installed = join(project, "node_modules", "blitsen");
  const manifest = JSON.parse(await readFile(join(installed, "package.json"), "utf8"));
  const version = run(process.execPath, [join(installed, "bin", "blitsen.mjs"), "--version"], project);
  assert.equal(version, manifest.version, "the packed CLI reports its manifest version");

  const runtime = await import(pathToFileURL(join(installed, "src", "runtime.mjs")));
  const target = runtime.hostTarget();
  const platformPackage = join(project, "node_modules", "@blitsen", target);
  await mkdir(platformPackage, { recursive: true });
  await writeFile(join(platformPackage, "package.json"), JSON.stringify({
    name: `@blitsen/${target}`,
    version: manifest.version,
  }));
  await writeFile(join(platformPackage, runtime.RUNTIME_BINARY), "runtime-resolution-smoke");

  const resolved = await runtime.resolveRuntime({
    version: manifest.version,
    env: {},
    repository: async () => null,
  });
  assert.deepEqual(resolved, {
    path: join(platformPackage, runtime.RUNTIME_BINARY),
    target,
    version: manifest.version,
    package: `@blitsen/${target}`,
    source: "package",
  });
  console.log(`packed blitsen@${manifest.version} passed on Node ${process.version} (${target})`);
} finally {
  await rm(scratch, { recursive: true, force: true });
}
