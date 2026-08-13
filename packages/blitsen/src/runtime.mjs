import { access, copyFile, mkdir, mkdtemp, readdir, readFile, rm, stat, writeFile }
  from "node:fs/promises";
import { gunzipSync } from "node:zlib";
import { createRequire } from "node:module";
import { homedir, tmpdir } from "node:os";
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

// Issue #72: a target's runtime is fetched when it is asked for, not installed
// six times over on every machine that only ever builds for one of them.
//
// The download is `npm pack`, not `npm install`: a platform package declares its
// `os` and `cpu`, so installing a foreign one is refused by npm as EBADPLATFORM
// — which is right for a dependency and wrong for a build input. `npm pack` has
// no such opinion, and still goes through the user's registry, credentials and
// integrity checking rather than around them.

/**
 * Where fetched runtimes are kept, honouring the platform's cache convention.
 *
 * A cache, not state: everything here can be re-downloaded, so it belongs
 * somewhere the system is allowed to clear.
 */
export function runtimeCacheDir(env = process.env, platform = process.platform) {
  if (env.BLITSEN_CACHE_DIR) return env.BLITSEN_CACHE_DIR;
  if (platform === "win32") {
    return join(env.LOCALAPPDATA || join(homedir(), "AppData", "Local"), "blitsen", "Cache");
  }
  if (platform === "darwin") return join(homedir(), "Library", "Caches", "blitsen");
  return join(env.XDG_CACHE_HOME || join(homedir(), ".cache"), "blitsen");
}

/**
 * Reads one file out of a gzipped tar, by the name npm gives it.
 *
 * Implemented here rather than shelled out to `tar` because a build tool that
 * cross-compiles ought not to depend on which tar the host happens to have —
 * and because one regular file out of a flat npm tarball is a small enough
 * problem to do exactly. Only what that needs is handled: the ustar name and
 * size fields, the 512-byte record padding, and the long-name extension GNU tar
 * writes for paths past 100 bytes. Anything else is skipped rather than guessed.
 */
export function extractFromTarball(archive, wanted) {
  const bytes = new Uint8Array(gunzipSync(archive));
  const text = (offset, length) => {
    const raw = new TextDecoder().decode(bytes.subarray(offset, offset + length));
    const end = raw.indexOf("\0");
    return end < 0 ? raw : raw.slice(0, end);
  };
  let longName = null;
  for (let offset = 0; offset + 512 <= bytes.length; ) {
    const name = longName ?? text(offset, 100);
    const sizeField = text(offset + 124, 12).trim();
    // A zero-filled record is the end-of-archive marker, not a member.
    if (name === "" && sizeField === "") break;
    const size = Number.parseInt(sizeField, 8) || 0;
    const type = String.fromCharCode(bytes[offset + 156]);
    const body = offset + 512;
    const next = body + Math.ceil(size / 512) * 512;
    if (type === "L") {
      // GNU long name: the next header's real name is this record's body.
      longName = new TextDecoder().decode(bytes.subarray(body, body + size)).replace(/\0+$/, "");
      offset = next;
      continue;
    }
    longName = null;
    if (name === wanted && (type === "0" || type === "\0")) return bytes.slice(body, body + size);
    offset = next;
  }
  return null;
}

/** Runs `npm pack` for one package version and returns the tarball's bytes. */
async function downloadRuntimePackage(name, version, run) {
  const scratch = await mkdtemp(join(tmpdir(), "blitsen-runtime-"));
  try {
    const packed = await run(
      ["npm", "pack", `${name}@${version}`, "--pack-destination", scratch, "--silent"],
      scratch);
    if (packed.code !== 0) {
      const reported = (packed.stderr || packed.stdout).trim().split("\n")
        .filter(line => line.trim()).at(-1) ?? `npm pack exited ${packed.code}`;
      // The target being unpublished is the ordinary failure here and reads
      // nothing like a network error, so it is answered separately: the reader
      // needs to know it is not their machine, and what they can do instead.
      const missing = /E404|not found|is not in this registry/i.test(reported);
      throw new Error(missing
        ? `${name}@${version} is not published, so ${name.slice("@blitsen/".length)} `
          + "cannot be built for from here.\n"
          + `  Build it on a ${name.slice("@blitsen/".length)} host with 'blitsen build',\n`
          + "  or point this build at an addon you already have with "
          + "BLITSEN_NATIVE_PATH=/path/to/blitsen.node.\n"
          + `  npm said: ${reported}`
        : `could not download ${name}@${version}: ${reported}`);
    }
    // npm prints the file it wrote, but --silent suppresses it on some versions,
    // so the directory is the answer: it was empty a moment ago.
    const files = (await readdir(scratch)).filter(file => file.endsWith(".tgz"));
    if (files.length !== 1) {
      throw new Error(`npm pack produced ${files.length} tarballs for ${name}@${version}`);
    }
    return await readFile(join(scratch, files[0]));
  } finally {
    await rm(scratch, { recursive: true, force: true });
  }
}

const npmRun = async (cmd, cwd) => {
  const spawned = Bun.spawnSync({ cmd, cwd, stdout: "pipe", stderr: "pipe" });
  return {
    code: spawned.exitCode,
    stdout: spawned.stdout.toString(),
    stderr: spawned.stderr.toString(),
  };
};

/**
 * Fetches one of a target's binaries into the cache, or returns the cached one.
 *
 * Cached by package version as well as by target, because the runtime and the
 * CLI are one ABI (#73): two versions must not share a slot. `binary` names the
 * file inside the platform package — the `.node` addon `blitsen run` loads, or
 * the Phase 2 executable an export links into — and the two share a cache
 * directory because they share a package, a version and a target.
 */
export async function fetchRuntime({
  target,
  version,
  binary = RUNTIME_BINARY,
  env = process.env,
  run = npmRun,
  cacheDir = runtimeCacheDir(env),
} = {}) {
  if (!TARGETS.includes(target)) throw missingRuntime(target);
  const name = runtimePackage(target);
  const directory = join(cacheDir, "runtimes", version, target);
  const cached = join(directory, binary);
  if (await readable(cached)) {
    return { path: cached, target, version, package: name, source: "cache" };
  }
  const tarball = await downloadRuntimePackage(name, version, run);
  const addon = extractFromTarball(tarball, `package/${binary}`);
  if (addon === null) {
    throw new Error(`${name}@${version} carries no ${binary}; `
      + "it cannot be used to build for " + target);
  }
  await mkdir(directory, { recursive: true });
  await writeFile(cached, addon);
  return { path: cached, target, version, package: name, source: "fetched" };
}

const modified = async path => (await stat(path).catch(() => null))?.mtimeMs ?? -Infinity;

/**
 * The addon a checkout built for itself, made loadable.
 *
 * `require` decides a native addon by extension and cargo names its output per
 * platform, so what cargo left behind is copied to `blitsen.node` rather than
 * loaded where it sits — returning the `.so` directly hands `require` a file it
 * tries to parse as JavaScript. The copy is refreshed whenever cargo's is newer,
 * so a rebuild is picked up instead of being shadowed by the last run's copy.
 */
async function repositoryRuntime(target) {
  if (target !== hostTarget()) return null;
  const directory = fileURLToPath(new URL("../../../target/release/", import.meta.url));
  const addon = join(directory, RUNTIME_BINARY);
  const name = CARGO_LIBRARIES[process.platform];
  const library = name ? join(directory, name) : null;
  if (library !== null && await readable(library)) {
    if (await modified(library) > await modified(addon)) await copyFile(library, addon);
    return addon;
  }
  return await readable(addon) ? addon : null;
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
  // Issue #72: only a cross-target build reaches for the network, and only
  // after every local answer has been tried. A host build must never start
  // downloading because a checkout happened to be missing its own addon.
  fetch = false,
  run,
  cacheDir,
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
  if (fetch) return fetchRuntime({ target, version: version ?? await packageVersion(), env, run, cacheDir });
  throw missingRuntime(target);
}

// Phase 2 (issue #88): the export links into Blitsen's own executable rather
// than into Bun's, so the platform package carries one more file. Which host an
// export uses is deliberately *not* a CLI flag or a config key — structural
// constraint 7 says the migration is a smaller binary and nothing else — so it
// is not selected by the user at all. The platform packages carry the Phase 2
// runtime, so that is what an export links into, unless the application needs
// something only Phase 1 can give it — see `buildStandalone`, which decides
// from what the export collected. `BLITSEN_HOST` forces either host, for
// measuring one against the other and for getting out of a regression.
export const PHASE2_BINARY = "blitsen-runtime";

/** What the Phase 2 executable is called inside a target's platform package. */
export const phase2Binary = (target = hostTarget()) =>
  `${PHASE2_BINARY}${target.startsWith("win32-") ? ".exe" : ""}`;

/**
 * The host the environment asked for, or `null` for "whichever fits".
 *
 * Left unset — which is the ordinary case — the exporter chooses, because the
 * choice is not free: the Phase 1 pair is the only one that can load a `.node`
 * addon, and it costs a copy of Bun to say so.
 */
export function requestedHost(env = process.env) {
  const requested = env.BLITSEN_HOST;
  if (requested === undefined || requested === "") return null;
  if (requested !== "bun" && requested !== "blitsen") {
    throw new Error(`BLITSEN_HOST must be bun or blitsen, got ${JSON.stringify(requested)}`);
  }
  return requested;
}

/** Which host an export links into: `"blitsen"` (the default) or `"bun"` (Phase 1). */
export function exportHost(env = process.env) {
  return requestedHost(env) ?? "blitsen";
}

/**
 * Finds the Phase 2 runtime executable for `target`.
 *
 * Same order as [`resolveRuntime`], and the same reasons: an explicit path, the
 * installed platform package, then this checkout's release build, then — for a
 * cross-target build only — the registry.
 */
export async function resolvePhase2Runtime({
  target = hostTarget(),
  version,
  env = process.env,
  require: resolver = createRequire(import.meta.url),
  fetch = false,
  run,
  cacheDir,
} = {}) {
  const configured = env.BLITSEN_RUNTIME_PATH;
  if (configured) {
    const path = configured.startsWith("file:") ? fileURLToPath(configured) : configured;
    return { path, target, version: null, package: null, source: "environment" };
  }
  const name = runtimePackage(target);
  const binary = phase2Binary(target);
  try {
    const manifestPath = resolver.resolve(`${name}/package.json`);
    const path = join(dirname(manifestPath), binary);
    if (await readable(path)) {
      const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
      return { path, target, version: manifest.version, package: name, source: "package" };
    }
  } catch {} // Not installed is the ordinary case for every non-host target.
  if (target === hostTarget()) {
    const root = new URL("../../../", import.meta.url);
    const path = fileURLToPath(new URL(`target/release/${binary}`, root));
    if (await readable(path)) {
      return { path, target, version: null, package: null, source: "repository" };
    }
  }
  if (fetch) {
    return fetchRuntime({
      target, version: version ?? await packageVersion(), binary, env, run, cacheDir,
    });
  }
  throw new Error(`no Phase 2 Blitsen runtime for ${target}: ${name} is not installed `
    + `and this checkout has no target/release/${binary}. `
    + "From a checkout, build one with `cargo build --release -p blitsen-runtime`, "
    + "or set BLITSEN_RUNTIME_PATH.");
}

/** Loads a resolved addon and adapts the engine to the surface the CLI drives. */
export function openRuntime(resolved) {
  const native = createRequire(import.meta.url)(resolved.path);
  const engine = new native.Engine();
  return {
    resolved,
    openDirectory(options) {
      return engine.openDirectory(options);
    },
    reloadCSS: engine.reloadCSS ? file => engine.reloadCSS(file) : null,
    reloadDirectory: engine.reloadDirectory ? () => engine.reloadDirectory() : null,
    pumpWindow: engine.pumpWindow ? () => engine.pumpWindow() : null,
    waitForNextFrame: globalThis.Bun ? delay => Bun.sleep(delay) : null,
  };
}
