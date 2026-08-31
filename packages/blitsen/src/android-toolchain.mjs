// Detect Android build prerequisites precisely, but never install them. Checks
// are ordered so each failure identifies the next prerequisite a user can fix.

import { spawn } from "node:child_process";
import { access, readdir } from "node:fs/promises";
import { constants, existsSync } from "node:fs";
import { homedir } from "node:os";
import { join, resolve } from "node:path";

/// API 26 is the minimum because the audio backend links `libaaudio.so`, which
/// the NDK does not provide for earlier levels. Compile and manifest floors must
/// match or the application can bind unavailable symbols and fail at startup.
export const MIN_SDK = 26;
export const TARGET_SDK = 33;

/// The entry crate is itself the `cdylib` exporting `android_main`; NativeActivity
/// loads its fixed library name rather than a name derived from the application.
export const ENTRY_CRATE = "blitsen-android";
export const ENTRY_LIBRARY = "blitsen_android";
export const ENTRY_SO = `lib${ENTRY_LIBRARY}.so`;

/// Where the environment names the SDK and the NDK, in the order Google's own
/// tools read them. `ANDROID_SDK_ROOT` is deprecated and still what many CI
/// images set, so it is read second rather than dropped.
const SDK_VARIABLES = ["ANDROID_HOME", "ANDROID_SDK_ROOT"];
const NDK_VARIABLES = ["ANDROID_NDK_HOME", "ANDROID_NDK_ROOT", "NDK_HOME"];

const readable = path => access(path, constants.R_OK).then(() => true, () => false);

/// `which`, without requiring Bun. `PATHEXT` preserves Windows executable lookup.
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
export async function detectAndroidToolchain({
  env = process.env, which = onPath, hostPlatform = process.platform,
} = {}) {
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
  // Named one at a time rather than discovered, because each is a separate step
  // of the packaging in `android.mjs` and a build-tools missing any of them
  // fails halfway through with a partial archive on disk. `aapt2` and not
  // `aapt`: v1 is what Google has been removing, and nothing here needs it now
  // that the archive is written rather than handed to a packager.
  const tools = {};
  const buildToolFile = tool => hostPlatform === "win32"
    ? ({ aapt2: "aapt2.exe", d8: "d8.bat", zipalign: "zipalign.exe",
      apksigner: "apksigner.bat" })[tool]
    : tool;
  for (const tool of ["aapt2", "d8", "zipalign", "apksigner"]) {
    tools[tool] = join(buildTools, buildToolFile(tool));
    if (!await readable(tools[tool])) {
      throw missing(`build-tools ${version} has no ${tool}, which packaging an APK runs.`,
        `Reinstall build-tools ${version} — \`sdkmanager "build-tools;${version}"\`.`);
    }
  }
  const javac = which("javac");
  if (!javac) {
    throw missing("javac is not on PATH, and the notification activation bridge is Java code.",
      "Install a JDK. Android packaging already needs its Java runtime for apksigner and its "
      + "keytool for the default debug signing key.");
  }
  tools.javac = javac;
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
  return { sdk, ndk, llvm, sysroot: join(llvm, "sysroot"), buildTools,
    buildToolsVersion: version, platform, packager, libclang, tools };
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

/**
 * The executable and argv Node should spawn for one planned command.
 *
 * Android's Windows build-tools deliberately ship `d8.bat` and
 * `apksigner.bat`. Windows does not make batch files executable through
 * CreateProcess, so spawning those paths directly fails before the tool runs.
 * Do not use `shell: true`: it expands metacharacters in every command on
 * every platform. Only the two batch suffixes go through cmd.exe, with
 * AutoRun and delayed expansion disabled and every token quoted.
 *
 * Percent and quote cannot be represented without invoking cmd.exe expansion
 * rules. Windows filenames cannot contain quote, and rejecting percent in the
 * uncommon path/argument that has one is safer than turning it into an
 * environment-variable expansion. Ampersand, pipe and the other ordinary cmd
 * metacharacters remain inert inside the quotes.
 */
export function subprocessInvocation(command, {
  platform = process.platform,
  environment = process.env,
} = {}) {
  if (!Array.isArray(command) || command.length === 0) {
    throw new Error("a subprocess command needs an executable");
  }
  const [executable, ...arguments_] = command.map(String);
  if (platform !== "win32" || !/\.(?:bat|cmd)$/i.test(executable)) {
    return { executable, arguments: arguments_ };
  }
  const unsafe = command.find(value => /[\0\r\n"%]/.test(String(value)));
  if (unsafe !== undefined) {
    throw new Error("a Windows .bat/.cmd tool argument contains NUL, a newline, quote or %, "
      + "which cannot be passed through cmd.exe without expansion");
  }
  const line = `"${command.map(value => `"${String(value)}"`).join(" ")}"`;
  return {
    executable: environment.ComSpec ?? environment.COMSPEC ?? "cmd.exe",
    arguments: ["/d", "/v:off", "/s", "/c", line],
  };
}

/** Runs one command, streaming its output, and resolves with its exit code. */
export function defaultRun(command, { cwd, environment, capture = false, output = null } = {}) {
  return new Promise((settle, fail) => {
    const invocation = subprocessInvocation(command, {
      environment: { ...process.env, ...environment },
    });
    const child = spawn(invocation.executable, invocation.arguments, {
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
