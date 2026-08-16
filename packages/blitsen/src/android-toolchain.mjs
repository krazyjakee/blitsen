// Finding the Android toolchain, and the crate the artifact is built from
// (issue #148).
//
// Split from `android.mjs` because it answers a different question. That file
// decides what an Android artifact *is* — its ABIs, its identity, its signing,
// how it is packaged. This one is entirely about the machine the build is
// running on: whether it has an SDK, an NDK, a build-tools that ships `aapt2`,
// a cross-compiler driver, a libclang, the Rust targets, and the entry crate
// that has not been published yet. None of it is a decision about the product;
// all of it is a question with a yes-or-no answer and an installation command
// attached.
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
import { access, readdir } from "node:fs/promises";
import { constants } from "node:fs";
import { homedir } from "node:os";
import { join, resolve } from "node:path";

/// The API level an artifact targets, and the oldest it installs on.
///
/// Here rather than with the packaging decisions because the target level is
/// also a *prerequisite*: `android-<TARGET_SDK>/android.jar` has to be installed
/// for the packager to link against, and this is the file that checks for it.
///
/// 24 was the first answer — the floor `android-activity` and the NDK's own C++
/// runtime settle on in practice — and **it does not link**. The runtime's audio
/// backend reaches `libaaudio`, which the NDK ships from API 26 and no earlier,
/// so `cargo ndk -P 24` fails at the link step with `unable to find library
/// -laaudio`. Raising the compile level while leaving the manifest at 24 would
/// have produced a shared object binding symbols that are absent on the devices
/// the manifest invites, and the failure there is a `dlopen` on a cold start
/// with nothing on screen. 26 is Android 8.0.
///
/// This is measured rather than read: the sysroot has `libaaudio.so` under
/// `usr/lib/<triple>/26` and under no lower level, and #148's original 24 was
/// never built against the real entry crate, which is why nothing caught it.
///
/// 33 is what #139 and #143 measured against, so it is what is claimed.
export const MIN_SDK = 26;
export const TARGET_SDK = 33;

/// Where the entry point comes from, and what linking it produces.
///
/// #142 owns the crate and it is a `cdylib` exporting `android_main` directly —
/// there is no rlib, no macro, and nothing for a build to generate. This was an
/// open question until #143 settled it by building one and running it: `cargo
/// ndk ... -p blitsen-android` links `libblitsen_android.so`, and
/// `android.app.lib_name = blitsen_android` is what `NativeActivity` `dlopen`s.
/// So the library name is the crate's `[lib] name` and not something derived
/// from the application, and `cli-android.test.mjs` reads all three of these
/// back out of `crates/blitsen-android/Cargo.toml` and fails if they drift.
export const ENTRY_CRATE = "blitsen-android";
export const ENTRY_LIBRARY = "blitsen_android";
export const ENTRY_SO = `lib${ENTRY_LIBRARY}.so`;

/// Where the environment names the SDK and the NDK, in the order Google's own
/// tools read them. `ANDROID_SDK_ROOT` is deprecated and still what many CI
/// images set, so it is read second rather than dropped.
const SDK_VARIABLES = ["ANDROID_HOME", "ANDROID_SDK_ROOT"];
const NDK_VARIABLES = ["ANDROID_NDK_HOME", "ANDROID_NDK_ROOT", "NDK_HOME"];

const readable = path => access(path, constants.R_OK).then(() => true, () => false);

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
export async function detectAndroidToolchain({ env = process.env, which = command => Bun?.which?.(command) ?? null } = {}) {
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
  const version = await newestVersioned(join(sdk, "build-tools"));
  if (version === null) {
    throw missing(`no Android build-tools are installed under ${sdk}.`,
      "Install some — `sdkmanager \"build-tools;34.0.0\"`.");
  }
  const buildTools = join(sdk, "build-tools", version);
  // Named one at a time rather than discovered, because each is a separate step
  // of the packaging in `android.mjs` and a build-tools missing any of them
  // fails halfway through with a partial archive on disk. `aapt2` and not
  // `aapt`: v1 is what Google has been removing, and nothing here needs it now
  // that the archive is written rather than handed to a packager.
  const tools = {};
  for (const tool of ["aapt2", "zipalign", "apksigner"]) {
    tools[tool] = join(buildTools, tool);
    if (!await readable(tools[tool])) {
      throw missing(`build-tools ${version} has no ${tool}, which packaging an APK runs.`,
        `Reinstall build-tools ${version} — \`sdkmanager "build-tools;${version}"\`.`);
    }
  }
  const platform = join(sdk, "platforms", `android-${TARGET_SDK}`, "android.jar");
  if (!await readable(platform)) {
    throw missing(`the API ${TARGET_SDK} platform is not installed under ${sdk}.`,
      `Install it — \`sdkmanager "platforms;android-${TARGET_SDK}"\`.`);
  }
  const packager = which("cargo-ndk");
  if (!packager) {
    throw missing("cargo-ndk is not on PATH, and it is what points the Rust cross-compile at "
      + "the NDK.",
      "Install it — `cargo install cargo-ndk`. See the reasoning at the top of "
      + "packages/blitsen/src/android.mjs.");
  }
  const libclang = await findLibclang(env);
  if (!libclang) {
    throw missing("no libclang was found, and this is the one target that needs one: rquickjs "
      + "ships no pre-generated Android bindings and runs bindgen instead.",
      "Install one — `apt install libclang-dev`, `dnf install clang-devel`, or "
      + "`brew install llvm` — or set LIBCLANG_PATH to the directory that holds it.");
  }
  return { sdk, ndk, buildTools, buildToolsVersion: version, platform, packager, libclang, tools };
}

/// Where a shared library called `libclang` may be, in the order it is worth
/// looking. Newest LLVM first, then the multiarch and system directories, then
/// the two places macOS keeps one.
async function libclangCandidates() {
  const versioned = (await readdir("/usr/lib").catch(() => []))
    .filter(name => /^llvm-\d/.test(name))
    .sort((left, right) => Number(right.slice(5)) - Number(left.slice(5)))
    .map(name => `/usr/lib/${name}/lib`);
  return [
    ...versioned,
    "/usr/lib/x86_64-linux-gnu", "/usr/lib/aarch64-linux-gnu", "/usr/lib64", "/usr/lib",
    "/usr/local/lib", "/opt/homebrew/opt/llvm/lib", "/usr/local/opt/llvm/lib",
    "/Library/Developer/CommandLineTools/usr/lib",
    "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib",
  ];
}

/**
 * Where `libclang` lives, which is a build-time requirement of the Android
 * target and of no other: `rquickjs-sys` ships no pre-generated bindings for
 * Android and runs bindgen instead. #143's spike had to set `LIBCLANG_PATH` by
 * hand to build at all, which is exactly the kind of prerequisite this file
 * exists to name before an hour of cross-compiling finds it.
 *
 * `LIBCLANG_PATH` is taken on trust when it is set, because bindgen reads the
 * same variable and reporting a disagreement with it here would be this file
 * second-guessing the thing it is configuring. A directory is returned rather
 * than a file, because that is what bindgen wants.
 */
export async function findLibclang(env, candidates = null) {
  if (env.LIBCLANG_PATH) return env.LIBCLANG_PATH;
  for (const directory of candidates ?? await libclangCandidates()) {
    const entries = await readdir(directory).catch(() => []);
    // `libclang.so`, `libclang.so.1`, `libclang-18.so.18.1` and
    // `libclang.dylib` are all the same library under four packagings.
    if (entries.some(name => /^libclang.*\.(so|dylib)(\.|$)/.test(name))) return directory;
  }
  return null;
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
 * Where cargo will leave the cross-compiled shared objects.
 *
 * Asked of cargo rather than assumed to be `<workspace>/target`, because it is
 * not: `CARGO_TARGET_DIR`, `build.target-dir` in any `.cargo/config.toml` on
 * the way up, and a shared target directory across a set of checkouts are all
 * ordinary, and every one of them moves the `.so` this build has to pick up. A
 * wrong guess here does not fail loudly — it finds a *stale* library from an
 * earlier build and packages that — so it is resolved rather than guessed.
 *
 * `--no-deps` because nothing about the dependency graph is wanted, only the
 * one path, and resolving the graph for an Android target is the slow part.
 */
export async function cargoTargetDirectory(entryCrate, run = defaultRun) {
  const result = await run(["cargo", "metadata", "--no-deps", "--format-version", "1",
    "--manifest-path", join(entryCrate, "Cargo.toml")], { capture: true });
  if (result.code !== 0) {
    throw new Error(`cargo metadata exited ${result.code} for ${entryCrate}, so this build cannot `
      + `tell where the cross-compiled ${ENTRY_SO} will be left.\n${result.stderr.trim()}`);
  }
  const directory = JSON.parse(result.stdout).target_directory;
  if (typeof directory !== "string" || directory === "") {
    throw new Error("cargo metadata named no target_directory, so this build cannot tell where "
      + `the cross-compiled ${ENTRY_SO} will be left.`);
  }
  return directory;
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
