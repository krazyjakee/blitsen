// Finding the Android toolchain, and the two things a generated project needs
// from the checkout it was built in (issue #148).
//
// Split from `android.mjs` because it answers a different question. That file
// decides what an Android artifact *is* — its ABIs, its identity, its signing,
// how it is packaged. This one is entirely about the machine the build is
// running on: whether it has an SDK, an NDK, a build-tools that still ships
// `aapt`, a packager, the Rust targets, and the entry crate that has not been
// published yet. None of it is a decision about the product; all of it is a
// question with a yes-or-no answer and an installation command attached.
//
// The rule it implements is decision 5 in `android.mjs`, and it is worth
// restating in one line where the code lives: **detect precisely, install
// nothing.** An Android build is a cross-compile, so cargo, rustc, two target
// standard libraries and a C toolchain have to be there whoever provides them;
// downloading two and a half licence-gated gigabytes would shorten that list by
// one and give this package a versioned cache it has no business owning. So
// every check below names the one thing that is missing and the one command
// that installs it, in the order a person can act on rather than the order a
// subprocess happens to trip over.

import { spawn } from "node:child_process";
import { access, readdir, readFile } from "node:fs/promises";
import { constants, existsSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";

/// The API level an artifact targets, and the oldest it installs on.
///
/// Here rather than with the packaging decisions because the target level is
/// also a *prerequisite*: `android-<TARGET_SDK>/android.jar` has to be installed
/// for the packager to link against, and this is the file that checks for it.
///
/// 26, and the number is a link error rather than a preference. `cpal`'s Android
/// backend is AAudio, so `blitsen-android` links `-laaudio`, and the NDK ships
/// `libaaudio.so` from API 26 and no earlier — `sysroot/usr/lib/<triple>/24/`
/// and `/25/` simply do not contain it, so a min_sdk below 26 fails at `ld.lld`
/// with `unable to find library -laaudio`. This was 24 until an APK was built
/// against Blitsen's own graph for the first time and found it (#149).
///
/// What it costs is Android 7.x, which no device with a Vulkan driver worth
/// rendering to is still on: 26 is Oreo, August 2017, and it is also the floor
/// #148 already names for the 32-bit ABI it declines to default. 33 is what
/// #139 measured against, so it is what is claimed.
export const MIN_SDK = 26;
export const TARGET_SDK = 33;

/// Where the entry point comes from. Issue #142 owns the crate; these name the
/// interface it has to present, and they are the only lines that have to change
/// if #142 lands under another name.
export const ENTRY_CRATE = "blitsen-android";
export const ENTRY_MACRO = "blitsen_android::android_main";

/// Where the environment names the SDK and the NDK, in the order Google's own
/// tools read them. `ANDROID_SDK_ROOT` is deprecated and still what many CI
/// images set, so it is read second rather than dropped.
const SDK_VARIABLES = ["ANDROID_HOME", "ANDROID_SDK_ROOT"];
const NDK_VARIABLES = ["ANDROID_NDK_HOME", "ANDROID_NDK_ROOT", "NDK_HOME"];

const readable = path => access(path, constants.R_OK).then(() => true, () => false);

/// `which`, without Bun.
///
/// The obvious spelling of this is `Bun.which(command)`, and it was — which
/// made `npx blitsen build --android` die at step ② with `Bun is not defined`
/// on every machine that installed the package instead of cloning it. That is
/// #131's bug exactly, on the one path #131 did not cover, and it is spelled
/// out here because `Bun?.which` *looks* like it guards the case and does not:
/// optional chaining short-circuits a null value, not an undeclared binding, so
/// the reference throws before the `?.` is ever consulted.
///
/// Synchronous and eager because the callers are, and because a PATH scan is a
/// handful of `stat`s. `PATHEXT` is honoured so this answers the same question
/// on Windows, where the packager is `cargo-apk.exe`.
function onPath(command) {
  const separator = process.platform === "win32" ? ";" : ":";
  const extensions = process.platform === "win32"
    ? (process.env.PATHEXT ?? ".EXE;.CMD;.BAT").split(";") : [""];
  for (const directory of (process.env.PATH ?? "").split(separator)) {
    if (directory === "") continue;
    for (const extension of extensions) {
      const candidate = join(directory, command + extension);
      if (existsSync(candidate)) return candidate;
    }
  }
  return null;
}

/// Highest-numbered entry in a versioned SDK directory, by numeric segments —
/// so `34.0.0` beats `9.0.0`, which a string sort gets wrong.
async function newestVersioned(directory) {
  const entries = await readdir(directory).catch(() => []);
  const parsed = entries
    .map(name => ({ name, parts: name.split(".").map(Number) }))
    .filter(entry => entry.parts.every(Number.isFinite) && entry.parts.length > 0);
  parsed.sort((left, right) => {
    for (let index = 0; index < Math.max(left.parts.length, right.parts.length); index += 1) {
      const difference = (right.parts[index] ?? 0) - (left.parts[index] ?? 0);
      if (difference !== 0) return difference;
    }
    return 0;
  });
  return parsed[0]?.name ?? null;
}

/// Every failure in this file has the same two parts — what is missing, and the
/// one command that supplies it — so it is one constructor rather than a
/// convention each call site is trusted to follow.
export const missing = (what, fix) => new Error(`${what}\n  ${fix}`);

/**
 * Finds the SDK, the NDK, the build tools and the packager, or says which one
 * is missing and what installs it.
 *
 * Ordered so the answer is the first thing the reader has to do rather than the
 * first thing a subprocess tripped over. Everything is looked up rather than
 * assumed, because a machine with `ANDROID_HOME` set to a directory that no
 * longer exists is the ordinary failure, not an exotic one.
 */
export async function detectAndroidToolchain({ env = process.env, which = onPath } = {}) {
  const named = SDK_VARIABLES.map(name => [name, env[name]]).find(([, value]) => value);
  const guess = join(homedir(), "Android", "Sdk");
  const sdk = named?.[1] ?? (await readable(guess) ? guess : null);
  if (!sdk) {
    throw missing("the Android SDK was not found, so blitsen cannot build an APK.",
      "Install it (Android Studio, or the command-line tools) and set ANDROID_HOME to it.");
  }
  if (!await readable(sdk)) {
    throw missing(`${named ? `${named[0]} names ${sdk}` : sdk} but nothing is there.`,
      "Point ANDROID_HOME at an Android SDK that exists.");
  }
  const ndkNamed = NDK_VARIABLES.map(name => [name, env[name]]).find(([, value]) => value);
  let ndk = ndkNamed?.[1] ?? null;
  if (ndk === null) {
    const installed = await newestVersioned(join(sdk, "ndk"));
    ndk = installed && join(sdk, "ndk", installed);
  }
  if (!ndk || !await readable(ndk)) {
    throw missing(`the Android NDK was not found under ${sdk}.`,
      "Install one — `sdkmanager \"ndk;27.2.12479018\"` — or set ANDROID_NDK_HOME. "
      + "Blitsen does not download it: an Android build is a cross-compile and needs a "
      + "C toolchain, a Rust toolchain and two installed Rust targets either way, and the "
      + "NDK is two and a half gigabytes behind a licence.");
  }
  // The one C toolchain inside the NDK, found rather than named. An NDK ships
  // exactly one `toolchains/llvm/prebuilt/<host>` — `linux-x86_64`,
  // `darwin-x86_64` (on Apple Silicon too) or `windows-x86_64` — and reading
  // the directory is both shorter than a table of those and correct when
  // Google adds a fourth. Wanted here because two of the environment variables
  // an Android cross-compile needs are paths inside it, and computing them at
  // the call site would mean the same guess in two places.
  const prebuilt = join(ndk, "toolchains", "llvm", "prebuilt");
  const [host] = (await readdir(prebuilt).catch(() => [])).sort();
  if (!host) {
    throw missing(`${ndk} has no C toolchain under toolchains/llvm/prebuilt.`,
      "That directory is the NDK's compiler, linker and sysroot, so this is not an NDK. "
      + "Reinstall it — `sdkmanager \"ndk;27.2.12479018\"`.");
  }
  const llvm = join(prebuilt, host);
  const version = await newestVersioned(join(sdk, "build-tools"));
  if (version === null) {
    throw missing(`no Android build-tools are installed under ${sdk}.`,
      "Install some — `sdkmanager \"build-tools;34.0.0\"`.");
  }
  const buildTools = join(sdk, "build-tools", version);
  // Named rather than inferred: cargo-apk 0.10 drives `aapt` v1, which Google
  // is in the middle of removing, and a build-tools without it fails deep
  // inside the packager with a path nobody can act on.
  for (const tool of ["aapt", "zipalign", "apksigner"]) {
    if (!await readable(join(buildTools, tool))) {
      throw missing(`build-tools ${version} has no ${tool}, which the APK packager runs.`,
        tool === "aapt"
          ? "aapt v1 was removed from recent build-tools; install an older set — "
            + "`sdkmanager \"build-tools;34.0.0\"` — which is what this path is verified against."
          : `Reinstall build-tools ${version}.`);
    }
  }
  const platform = join(sdk, "platforms", `android-${TARGET_SDK}`, "android.jar");
  if (!await readable(platform)) {
    throw missing(`the API ${TARGET_SDK} platform is not installed under ${sdk}.`,
      `Install it — \`sdkmanager "platforms;android-${TARGET_SDK}"\`.`);
  }
  const packager = which("cargo-apk");
  if (!packager) {
    throw missing("cargo-apk is not on PATH, and it is what assembles the APK.",
      "Install it — `cargo install cargo-apk`. It is the packager Blitsen drives; see the "
      + "reasoning at the top of packages/blitsen/src/android.mjs.");
  }
  return { sdk, ndk, llvm, sysroot: join(llvm, "sysroot"), buildTools,
    buildToolsVersion: version, platform, packager };
}

/**
 * Refuses a Rust target that is not installed, before an hour of cross-compile
 * discovers it. Takes triples rather than ABI names so that this module owes
 * nothing to the one that decides which ABIs exist.
 *
 * Reported as notes rather than thrown when rustup is not the toolchain
 * manager: a rustup-less install may still have the targets, and refusing a
 * build on the absence of a tool that was never required is worse than letting
 * cargo answer.
 */
export async function missingRustTargets(triples, run = defaultRun) {
  const listed = await run(["rustup", "target", "list", "--installed"], { capture: true })
    .catch(() => null);
  if (listed === null || listed.code !== 0) return null;
  const installed = new Set(listed.stdout.split("\n").map(line => line.trim()).filter(Boolean));
  return triples.filter(triple => !installed.has(triple));
}

/**
 * The `[patch]` tables of the workspace the entry crate came from, verbatim.
 *
 * The generated project is its own workspace — it has to be, because it is
 * written next to the user's build output rather than inside a checkout — and a
 * `[patch.crates-io]` is a property of the workspace that does the resolving,
 * not of the crate that needs it. So a checkout whose engine is pinned to a
 * fork resolves the fork only if the generated project carries the same tables.
 * Without this the entry crate's `blitz` dependencies resolve to crates.io and
 * either fail or, worse, succeed against a different engine.
 *
 * Copied as text rather than parsed, because the sections carry the reasoning
 * for each pin and a regenerated TOML would drop it. Only git and registry
 * sources survive the move: a `path =` patch is relative to the manifest it is
 * written in, so one is refused rather than copied to somewhere it means
 * something else.
 */
export async function workspacePatches(entryCrate) {
  for (let directory = entryCrate, previous = null; directory !== previous;
    previous = directory, directory = dirname(directory)) {
    const manifest = await readFile(join(directory, "Cargo.toml"), "utf8").catch(() => null);
    if (manifest === null || !/^\[workspace\]/m.test(manifest)) continue;
    const sections = [];
    let capturing = false;
    for (const line of manifest.split("\n")) {
      const header = /^\s*\[([^\]]+)\]/.exec(line);
      // A comment block immediately above a `[patch]` header belongs to it, so
      // capture is decided at the header and the preceding comments are pulled
      // in with it.
      if (header) capturing = header[1] === "patch" || header[1].startsWith("patch.");
      if (capturing) sections.push(line);
    }
    const text = sections.join("\n").trim();
    if (text === "") return "";
    if (/^\s*[\w"-]+\s*=\s*\{[^}]*\bpath\s*=/m.test(text)) {
      throw new Error(`${join(directory, "Cargo.toml")} patches a dependency with a local path, `
        + "which cannot be copied into a generated project: a relative path would resolve "
        + "somewhere else. Pin it to a git revision, or build the APK from that workspace.");
    }
    return `\n${text}\n`;
  }
  return "";
}

/** Runs one command, streaming its output, and resolves with its exit code. */
export function defaultRun(command, { cwd, environment, capture = false, output = null } = {}) {
  return new Promise((settle, fail) => {
    const child = spawn(command[0], command.slice(1), {
      cwd,
      env: { ...process.env, ...environment },
      stdio: capture ? ["ignore", "pipe", "pipe"] : ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", chunk => {
      stdout += chunk;
      if (!capture && output) for (const line of String(chunk).split("\n")) if (line) output.log(line);
    });
    child.stderr.on("data", chunk => {
      stderr += chunk;
      if (!capture && output) for (const line of String(chunk).split("\n")) if (line) output.log(line);
    });
    child.on("error", fail);
    child.on("close", code => settle({ code: code ?? 1, stdout, stderr }));
  });
}

/// Where the entry crate (#142) is found: named explicitly, or beside this
/// package in a checkout. There is no published crate to fall back to, and
/// saying so is more useful than a resolver error out of cargo.
export async function resolveEntryCrate(env = process.env) {
  const named = env.BLITSEN_ANDROID_CRATE;
  if (named) {
    if (!await readable(join(named, "Cargo.toml"))) {
      throw missing(`BLITSEN_ANDROID_CRATE names ${named}, which has no Cargo.toml.`,
        "Point it at the directory of the blitsen-android crate.");
    }
    return resolve(named);
  }
  const inTree = join(import.meta.dirname, "../../../crates", ENTRY_CRATE);
  if (await readable(join(inTree, "Cargo.toml"))) return resolve(inTree);
  throw missing(`the Android entry point crate ${ENTRY_CRATE} was not found.`,
    "It is the cdylib that exports android_main and is issue #142's; it is not published yet. "
    + "From a checkout that has it, this resolves on its own; otherwise set "
    + "BLITSEN_ANDROID_CRATE to its directory.");
}
