import { access, readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

// TECH.md §11: one binary package per target, installed by optionalDependencies and
// found through ordinary package resolution — never by walking node_modules.
export const TARGETS = ["darwin-arm64", "darwin-x64", "linux-arm64", "linux-x64",
  "win32-arm64", "win32-x64"];
// The targets whose runtime is actually built. The rest have a manifest and no
// binary until CI can build them; see issue #70.
export const BUILT_TARGETS = ["linux-x64"];
export const RUNTIME_BINARY = "blitsen.node";
// What `cargo build -p blitsen-node` leaves behind, for a checkout that built its own.
const CARGO_LIBRARIES = {
  linux: "libblitsen_node.so", darwin: "libblitsen_node.dylib", win32: "blitsen_node.dll",
};

export const hostTarget = () => `${process.platform}-${process.arch}`;
export const runtimePackage = target => `@blitsen/${target}`;

// Single source of truth: the published package manifest, not a literal.
export async function packageVersion() {
  const manifest = new URL("../package.json", import.meta.url);
  return JSON.parse(await readFile(manifest, "utf8")).version;
}

export function describeRuntime(runtime) {
  if (!runtime) return "none";
  if (runtime.package) return `${runtime.package}@${runtime.version}`;
  return `${runtime.target} (unversioned, from ${runtime.source})`;
}

const readable = path => access(path).then(() => true, () => false);

// TECH.md §11: the runtime package is pinned to this package's version exactly, so a
// pair that was never built together is a hard stop rather than a warning — the two
// halves are one ABI, and a mismatch changes what the application renders.
export function assertRuntimeVersion(target, expected, found) {
  if (expected === found) return;
  throw new Error(`runtime version mismatch: blitsen ${expected} requires `
    + `${runtimePackage(target)} ${expected}, but ${found} is installed. `
    + "The runtime is pinned exactly, so take both from one release "
    + `(npm install --save-dev --save-exact blitsen@${expected}), `
    + `or pin blitsen back to ${found}.`);
}

function missingRuntime(target) {
  if (!TARGETS.includes(target)) {
    return new Error(`Blitsen has no runtime for ${target}: `
      + `supported targets are ${TARGETS.join(", ")}`);
  }
  return new Error(`no Blitsen runtime for ${target}: ${runtimePackage(target)} is not installed. `
    + "It installs as an optional dependency of blitsen for a matching host, but no platform "
    + `runtime package is published yet and only ${BUILT_TARGETS.join(", ")} is built today `
    + "(see issue #70). From a checkout, build one with "
    + "`cargo build --release -p blitsen-node`, or set BLITSEN_NATIVE_PATH to an addon.");
}

async function repositoryRuntime(target) {
  if (target !== hostTarget()) return null;
  const root = new URL("../../../", import.meta.url);
  for (const name of [CARGO_LIBRARIES[process.platform], RUNTIME_BINARY]) {
    if (!name) continue;
    const path = fileURLToPath(new URL(`target/release/${name}`, root));
    if (await readable(path)) return path;
  }
  return null;
}

/**
 * Finds the native runtime for `target`, in the order a user can reason about:
 * an explicit path, then the platform package npm installed, then an addon this
 * checkout built. Throws rather than returning null — every caller needs the
 * reason, and "which platform, and why not" is the whole message.
 */
export async function resolveRuntime({
  target = hostTarget(),
  version,
  env = process.env,
  require: resolver = createRequire(import.meta.url),
} = {}) {
  const configured = env.BLITSEN_NATIVE_PATH;
  if (configured) {
    const path = configured.startsWith("file:") ? fileURLToPath(configured) : configured;
    return { path, target, version: null, package: null, source: "environment" };
  }
  const name = runtimePackage(target);
  let manifestPath = null;
  try {
    manifestPath = resolver.resolve(`${name}/package.json`);
  } catch {} // Not installed is the ordinary case for every non-host target.
  if (manifestPath !== null) {
    const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
    // Version before contents: a mismatched pair is the cause, an odd-looking package
    // the symptom, and reporting the symptom sends the reader to the wrong place.
    assertRuntimeVersion(target, version ?? await packageVersion(), manifest.version);
    const path = join(dirname(manifestPath), RUNTIME_BINARY);
    if (!await readable(path)) {
      throw new Error(`${name}@${manifest.version} is installed but carries no ${RUNTIME_BINARY}: `
        + `expected ${path}. Reinstall it, or set BLITSEN_NATIVE_PATH to an addon.`);
    }
    return { path, target, version: manifest.version, package: name, source: "package" };
  }
  const built = await repositoryRuntime(target);
  if (built !== null) return { path: built, target, version: null, package: null, source: "repository" };
  throw missingRuntime(target);
}

/** Loads a resolved addon and adapts the engine to the surface the CLI drives. */
export function openRuntime(resolved) {
  const native = createRequire(import.meta.url)(resolved.path);
  const engine = new native.Engine();
  return {
    resolved,
    openDirectory(options) {
      return engine.openDirectory?.(options) ?? engine.loadHTML(options.entrypoint);
    },
    reloadCSS: engine.reloadCSS ? file => engine.reloadCSS(file) : null,
    reloadDirectory: engine.reloadDirectory ? () => engine.reloadDirectory() : null,
    pumpWindow: engine.pumpWindow ? () => engine.pumpWindow() : null,
    waitForNextFrame: globalThis.Bun ? delay => Bun.sleep(delay) : null,
  };
}
