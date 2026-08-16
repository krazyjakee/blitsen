// `blitsen build --android` — the packaging step for the one target that is not
// a runtime an install resolves (issue #148).
//
// Every other target `blitsen build` knows is a row in TECH.md §11: one npm
// platform package per triple, fetched on demand, appended to. Android is none
// of that. There is no `@blitsen/android-arm64` to install, `hostTarget()` can
// never say `android`, and the artifact is not an executable — it is a signed
// zip the platform mounts, holding one shared library per ABI and the
// application's files under `assets/`. This module is where that difference is
// decided, and the decisions are written here rather than in an issue comment
// because this is the file that has to keep them.
//
// # 1. The command shape: a flag, not a triple
//
// **`blitsen build --android`, and `--target` keeps refusing Android.**
//
// `--target <triple>` selects *which prebuilt runtime gets linked into the same
// artifact*. Every value it takes produces a single-file executable by the same
// pipeline; the flag chooses a binary to append to. Android changes the noun.
// The output is an APK, the input is a cross-compile rather than a download,
// the signing is a keystore rather than codesign or Authenticode, and one
// artifact carries several architectures at once — which is the detail that
// settles it. `--target android-arm64` would have to mean "an APK for one ABI",
// and an APK for one ABI is the wrong default: `arm64-v8a` alone cannot be
// installed on the emulator, and the whole point of the format is that it does
// not need a flag per architecture.
//
// So the two flags mean different things and are refused together. What Android
// keeps of `--target` is `doctor`'s reading of it, which #147 already landed:
// `--target android-arm64` grades an application against the `native:` modules
// Android has. Grading for a platform and building for it are different claims,
// and the vocabulary stays split along that line.
//
// # 2. ABIs: `arm64-v8a` and `x86_64` by default, `armeabi-v7a` on request
//
// `arm64-v8a` is every Android device that matters — Play has required a 64-bit
// build since August 2019 and 64-bit-only devices are now ordinary — so it is
// not optional and cannot be turned off by leaving it out of `--android-abi`
// unless something else was named.
//
// `x86_64` is in the default set because it is the *emulator*, and an Android
// build nobody can run is not a build. An APK without it installs on no
// standard AVD image, which would make `blitsen build --android` produce
// something whose only proof of life is a physical phone — and #149's CI has an
// emulator, not a phone. The cost is one more cross-compile of the same graph.
//
// `armeabi-v7a` is accepted when asked for by name and is not in the default
// set. It is 32-bit ARM: `usize` is 32 bits, so the address space is the
// constraint an engine that keeps a scene, a DOM and a JavaScript heap resident
// feels first; the devices that are 32-bit-only stopped shipping around 2017
// and top out at Android 8; and they are precisely the Adreno and Mali
// generation where #139 records Vello as unproven. Building it is one flag away
// for anyone who needs it. Claiming it works is not something this file does,
// and the build prints that rather than staying quiet.
//
// 32-bit x86 is refused outright rather than left out: it exists only as an
// emulator image nobody uses now that `x86_64` images are universal, so
// accepting it would add an untested ABI to artifacts for no reader.
//
// # 3. Signing: a keystore, and the debug one by default
//
// APK signing is not codesign and not Authenticode. There is no notary, no
// certificate authority, and no revocation: the signing key *is* the
// application's identity, and Android refuses an update signed by a different
// key for the lifetime of the install. That makes the release key the one piece
// of state a project cannot regenerate, and the one thing this build must never
// invent.
//
// So the split is:
//
//   - **No `--android-keystore`** — the APK is signed with the Android debug
//     key, `~/.android/debug.keystore` with the well-known password `android`,
//     which every SDK install has and `cargo apk` creates through `keytool` if
//     it does not. That is what makes `blitsen build --android` produce
//     something installable on the first run with nothing configured, which is
//     the whole reason it is the default. It is also not distributable, and the
//     build says so on every run rather than in the documentation.
//   - **`--android-keystore <path>`** — a real key, with the password taken
//     from `BLITSEN_ANDROID_KEYSTORE_PASSWORD` rather than from a flag, because
//     an argument is visible in `ps` and lands in shell history and CI logs.
//     The alias and its password follow the same rule.
//
// `--sign <command>` is unchanged and still runs last over the finished
// artifact, which on this path is the APK. An organisation that signs from an
// HSM or a signing service uses that and does not hand this process a key at
// all.
//
// # 4. The toolchain: `cargo apk` now, behind a seam
//
// #139 surveyed five tools and the survey's own conclusion is that the
// deprecation chain is broken: `cargo apk`'s notice points at `xbuild`, and
// `xbuild` is unmaintained by its own repository. A deprecation with no
// successor is not a migration, so it is treated as what it is — a warning that
// nobody is promising future work — rather than as an instruction.
//
// **`cargo apk` is what this invokes.** It is one binary; it needs no Gradle,
// no JDK, no Android Studio project checked into the repository, and no
// generated Java. It is what Blitz drives against this exact stack — winit,
// wgpu, `android-activity`, no application logic in Java or Kotlin — and #139
// measured it producing a signed, installable APK from this dependency graph.
//
// What it gives up is real, and three of the four were measured rather than
// read, by building a probe APK with build-tools 34.0.0 and NDK r27c and
// reading the archive back:
//
//   - **No AAB.** Google Play has required an Android App Bundle for new
//     applications since August 2021. `cargo apk` emits an APK and nothing
//     else, so this path produces a sideloadable and emulator-installable
//     artifact — a developer's inner loop and #149's CI — and *not* a Play
//     upload. That is the single largest thing the Gradle path would buy, and
//     it is not delivered here.
//   - **`noCompress` cannot be asked for.** This is issue #144's one packaging
//     request, and the answer is measured: `cargo apk` sets its
//     `disable_aapt_compression` flag from `is_debug_profile` and exposes no
//     way to override it. A debug APK stores every asset uncompressed; a
//     release APK deflates all of them. So the property `apk.rs` wanted — read
//     in place, no inflation per read — holds on `--android-debug` and does not
//     hold on the default build. Nothing here pretends otherwise. It is also
//     the clearest single argument for the Gradle path, where it is one line.
//   - **`aapt` v1, not `aapt2`.** `cargo apk` 0.10 shells out to the original
//     `aapt`, which Google has been removing from build-tools. It is present in
//     34.0.0, which is what this was measured against; a newer build-tools that
//     has dropped it breaks this path, so the toolchain check below looks for
//     `aapt` by name and says so rather than letting the failure surface as a
//     missing file inside cargo-apk.
//   - **`zipalign 4`, not `zipalign -p`.** Shared libraries are aligned to 4
//     bytes rather than to a page, and in a release build they are deflated,
//     which means the system extracts them at install. Android 15's 16 KB page
//     devices want the other arrangement. Not fatal, and not fixable from here.
//
// The seam that makes the other choice affordable later is that nothing below
// runs a command directly. [`androidProject`] describes the project — its
// manifest fields, its ABIs, where its assets are — and [`apkPlan`] turns that
// description into one argv. A `cargo-ndk` plus Gradle backend is a second
// function of the same shape reading the same description, not a rewrite of the
// build. The description is deliberately expressed in Android's vocabulary
// (application ID, version code, ABI names) rather than in `cargo apk`'s.
//
// # 5. The NDK is a prerequisite, and the CLI does not provision it
//
// Product requirement P9 says an install fetches one runtime and needs no Rust
// toolchain. Android is where that stops being achievable, and it is worth
// being exact about why: an Android build *is* a cross-compile. It needs cargo,
// rustc, two installed Rust targets and a C toolchain no matter who provides
// them. Downloading the NDK would take that list from five prerequisites to
// four while adding two and a half gigabytes, a licence that has to be
// accepted, and a versioned cache to a CLI whose whole distribution story is
// that it has none of those.
//
// So: **detect precisely, install nothing.** [`detectAndroidToolchain`] names
// the one thing that is missing and the one command that fixes it, and it
// checks the pieces in the order a person can act on — SDK, then NDK, then
// build-tools, then the packager, then the Rust targets — rather than reporting
// the first failure a subprocess happens to hit.

import { mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { homedir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { ASSET_ROOT, stageAndroidAssets } from "./android-assets.mjs";
import {
  defaultRun, detectAndroidToolchain, ENTRY_CRATE, ENTRY_MACRO, MIN_SDK, missing,
  missingRustTargets, resolveEntryCrate, TARGET_SDK, workspacePatches,
} from "./android-toolchain.mjs";

// Re-exported so that `--android` has one module on its surface: everything a
// caller or a test asks for about an Android build is asked of this file, and
// the split behind it is an implementation detail of where the reasoning lives.
export {
  detectAndroidToolchain, ENTRY_CRATE, ENTRY_MACRO, MIN_SDK, missingRustTargets,
  resolveEntryCrate, TARGET_SDK, workspacePatches,
} from "./android-toolchain.mjs";

/// Every ABI this build knows, and the Rust target triple each is built from.
export const ANDROID_ABIS = {
  "arm64-v8a": "aarch64-linux-android",
  "x86_64": "x86_64-linux-android",
  "armeabi-v7a": "armv7-linux-androideabi",
};

/// What `--android` builds when `--android-abi` is not given. See decision 2.
export const DEFAULT_ABIS = ["arm64-v8a", "x86_64"];

/// ABIs that build if asked for and that nothing here claims to have run.
export const UNPROVEN_ABIS = ["armeabi-v7a"];

/// The Android debug key, as every SDK install has it.
export const DEBUG_KEYSTORE = () => join(homedir(), ".android", "debug.keystore");
export const DEBUG_KEYSTORE_PASSWORD = "android";

/// The ABIs a build was asked for, checked and ordered.
export function resolveAbis(requested) {
  if (requested === undefined || requested.length === 0) return [...DEFAULT_ABIS];
  const known = Object.keys(ANDROID_ABIS);
  const chosen = [];
  for (const abi of requested) {
    if (!known.includes(abi)) {
      throw new Error(`unknown --android-abi ${abi} (expected one of: ${known.join(", ")})`);
    }
    if (!chosen.includes(abi)) chosen.push(abi);
  }
  return chosen;
}

const JAVA_SEGMENT = /^[a-zA-Z][a-zA-Z0-9_]*$/;
// Reserved words a package segment may not be, because the manifest's package
// name becomes a Java package and `aapt` rejects the ones it can see.
const JAVA_KEYWORDS = new Set(["abstract", "assert", "boolean", "break", "byte", "case", "catch",
  "char", "class", "const", "continue", "default", "do", "double", "else", "enum", "extends",
  "final", "finally", "float", "for", "goto", "if", "implements", "import", "instanceof", "int",
  "interface", "long", "native", "new", "package", "private", "protected", "public", "return",
  "short", "static", "strictfp", "super", "switch", "synchronized", "this", "throw", "throws",
  "transient", "try", "void", "volatile", "while", "true", "false", "null"]);

const slug = text => text.toLowerCase().replace(/[^a-z0-9]+/g, "").replace(/^[^a-z]+/, "") || "app";

/**
 * The application ID, which is the one string an Android install is keyed by.
 *
 * Validated rather than sanitised. An application ID cannot be changed after
 * the first release — Play treats a different ID as a different application and
 * a device treats it as a second install — so quietly rewriting a malformed one
 * into something that happens to parse is the worst available outcome. A
 * generated default is fine because nobody has shipped it yet; a *given* one
 * that is wrong is refused with the rule it broke.
 */
export function applicationId(given, name = "app") {
  const id = given ?? `com.blitsen.${slug(name)}`;
  const segments = id.split(".");
  const complain = why => new Error(`${given ? "--android-package" : "the generated application ID"}`
    + ` ${JSON.stringify(id)} is not a valid Android application ID: ${why}.`
    + (given ? "" : " Pass --android-package to choose one."));
  if (segments.length < 2) throw complain("it needs at least two dot-separated segments");
  for (const segment of segments) {
    if (!JAVA_SEGMENT.test(segment)) {
      throw complain(`the segment ${JSON.stringify(segment)} must start with a letter and `
        + "contain only letters, digits and underscores");
    }
    if (JAVA_KEYWORDS.has(segment)) {
      throw complain(`${JSON.stringify(segment)} is a Java keyword`);
    }
  }
  return id;
}

/// What `cargo apk` reserves the top byte of a version code for. Fixed at 1 by
/// the packager and not configurable.
const APK_ID = 1;

/**
 * The integer Android orders installs by, computed the way the artifact will
 * actually carry it.
 *
 * This is a decision that was made twice, and the second answer is the one in
 * the code. The first was to set the code explicitly:
 * `major * 1_000_000 + minor * 1_000 + patch` is monotonic under semver, reads
 * back at a glance, and stays inside Play's ceiling of 2,100,000,000 — plainly
 * better than what the packager does. Then it was tried, and `cargo apk` 0.10
 * **panics** on `version_code` or `version_name` appearing in the manifest at
 * all: it overwrites both from the crate's own version and treats a value
 * already there as a bug. There is no flag and no metadata key that changes it.
 *
 * So the scheme is the packager's, whether or not it is the better one:
 * `apk_id << 24 | major << 16 | minor << 8 | patch`, each component a `u8`. What
 * that costs is worth naming, because a version code cannot be walked back —
 * Play refuses an upload numbered at or below one already published, forever:
 *
 *   - Any component past 255 is unrepresentable, and `cargo apk` fails rather
 *     than truncating. `1.0.256` cannot ship.
 *   - The top byte is spent on a constant, so a quarter of the range is gone.
 *
 * This function exists so that Blitsen *reports the code the APK will carry*
 * and refuses a version the packager cannot express, before an hour of
 * cross-compiling discovers it. Reporting the code from the better scheme while
 * shipping the packager's would be a number that is wrong in the one place
 * anyone would check it. It is also the fourth entry on the toolchain's bill,
 * and the one most likely to force the move to Gradle first.
 *
 * Pre-release and build metadata are dropped, matching the packager: Android
 * has nowhere to put them, and `versionName` carries the whole string.
 */
export function versionCode(version) {
  const core = String(version ?? "0.0.0").split(/[-+]/)[0];
  const parts = core.split(".");
  if (parts.length !== 3 || parts.some(part => !/^\d+$/.test(part))) {
    throw new Error(`--app-version ${JSON.stringify(version)} is not a version Android can order: `
      + "expected major.minor.patch in digits");
  }
  const [major, minor, patch] = parts.map(Number);
  const over = ["major", "minor", "patch"].filter((_, index) => [major, minor, patch][index] > 255);
  if (over.length > 0) {
    throw new Error(`--app-version ${JSON.stringify(version)} cannot be given an Android version `
      + `code: the APK packager packs each component into a byte, so ${over.join(" and ")} `
      + "must be below 256. See the note on versionCode in packages/blitsen/src/android.mjs.");
  }
  return (APK_ID << 24) | (major << 16) | (minor << 8) | patch;
}

/**
 * The Cargo project the packager is pointed at.
 *
 * Generated per build rather than checked in, because everything in it is the
 * user's: the application ID, the label, the version, and the path to the
 * staged assets. `cargo apk` reads all four out of `[package.metadata.android]`
 * in a `Cargo.toml` and offers no flag for any of them, so a checked-in crate
 * could only serve one application.
 *
 * The generated crate is the `cdylib`, and it is one line long. Everything real
 * lives in [`ENTRY_CRATE`], including the `android-activity` dependency and its
 * `native-activity` feature — the selection #139 lists as obstacle (3). Keeping
 * that inside the entry crate rather than repeating it here means the generated
 * project has no opinion about which `android-activity` is in the graph, which
 * is what stops a per-build file from pinning a version it cannot know.
 *
 * The crate's own name is load-bearing twice over: it names the shared library
 * (`lib<name>.so`) and it is what the generated manifest writes into
 * `android.app.lib_name`, which is the string `NativeActivity` calls `dlopen`
 * with. It is derived from the application ID rather than from the display name
 * so that it is a valid Rust identifier without any sanitising step.
 */
export function androidProject({
  name,
  applicationId: id,
  version = "0.1.0",
  abis = DEFAULT_ABIS,
  entryCrate = null,
  patches = "",
  assets = "assets",
}) {
  const library = `${id.split(".").join("_")}`.replace(/[^A-Za-z0-9_]/g, "_");
  // The `.apk` cargo-apk writes is named by this, so it is held to what a file
  // name can be rather than to what a window title can be.
  const apkName = name.replace(/[^A-Za-z0-9._-]+/g, "-").replace(/^[-.]+|[-.]+$/g, "") || "app";
  // Checked here rather than only where it is reported: the packager derives
  // the version code and the version name from this field and panics on a value
  // it cannot pack, so a version it will refuse is refused now.
  versionCode(version);
  const dependency = entryCrate === null
    ? `${ENTRY_CRATE} = { version = "*" }`
    : `${ENTRY_CRATE} = { path = ${JSON.stringify(entryCrate)} }`;
  const cargoToml = `# Generated by \`blitsen build --android\`. Edits are discarded on the next build.
[package]
name = ${JSON.stringify(library)}
version = ${JSON.stringify(version)}
edition = "2021"
publish = false

[lib]
crate-type = ["cdylib"]

[workspace]

[dependencies]
${dependency}

[package.metadata.android]
package = ${JSON.stringify(id)}
apk_name = ${JSON.stringify(apkName)}
assets = ${JSON.stringify(assets)}
build_targets = [${abis.map(abi => JSON.stringify(ANDROID_ABIS[abi])).join(", ")}]

[package.metadata.android.application]
label = ${JSON.stringify(name)}

[package.metadata.android.sdk]
min_sdk_version = ${MIN_SDK}
target_sdk_version = ${TARGET_SDK}
${patches}`;
  // One statement, and it is a macro rather than a function call so that the
  // `#[no_mangle] extern "C" fn android_main` it expands to is defined in the
  // cdylib itself. A `pub extern "C" fn` re-exported from an rlib is not
  // guaranteed to survive into the dynamic symbol table of the library that
  // links it, and a missing `android_main` is a crash at `dlopen` with nothing
  // to read.
  const libRs = `// Generated by \`blitsen build --android\`. Edits are discarded on the next build.
${ENTRY_MACRO}!();
`;
  return { library, apkName, cargoToml, libRs, applicationId: id, version, abis, assets };
}

/**
 * The one command that turns a generated project into a signed APK.
 *
 * Split out from running it so the whole decision is a value a test can read.
 * The keystore travels in the environment rather than in the argv for the
 * reason decision 3 gives, and `cargo apk` happens to want it there too:
 * `CARGO_APK_<PROFILE>_KEYSTORE` is its only interface for a release key.
 */
export function apkPlan({
  project,
  directory,
  toolchain,
  release = true,
  keystore = null,
  keystorePassword = null,
}) {
  const profile = release ? "RELEASE" : "DEV";
  const environment = {
    ANDROID_HOME: toolchain.sdk,
    ANDROID_SDK_ROOT: toolchain.sdk,
    ANDROID_NDK_HOME: toolchain.ndk,
    ANDROID_NDK_ROOT: toolchain.ndk,
  };
  // The two things `cargo apk` does not tell the cross-compile, and Blitsen's
  // graph needs both. `cargo apk` sets `CC_`, `CFLAGS_`, `AR_` and the cargo
  // linker for each target and stops there, which is enough for a crate whose C
  // is one `cc::Build`; it is not enough for this dependency graph, and neither
  // gap is visible until an APK is built against the real one (#149). Both are
  // what `cargo-ndk` sets for the same triples, and are copied from it rather
  // than invented.
  //
  //   * `RANLIB_<triple>` — `openssl-sys` builds OpenSSL vendored on Android
  //     (blitz-net asks reqwest for `native-tls-vendored` there), and OpenSSL's
  //     makefile runs `$(CROSS_COMPILE)ranlib`. The `cc` crate answers with
  //     `aarch64-linux-android-ranlib`, which NDK r23 removed along with the
  //     rest of the GNU binutils wrappers; what exists is `llvm-ranlib`. Without
  //     this the build dies in `make install_dev` with `ranlib: not found`.
  //   * `BINDGEN_EXTRA_CLANG_ARGS_<triple>` — `rquickjs-sys` ships no Android
  //     bindings, so `crates/blitsen-quickjs/Cargo.toml` turns on `bindgen`
  //     there, and that crate's build script hands bindgen no target and no
  //     sysroot. libclang then reads Android's headers as the host's and emits
  //     a `JSValue` that does not match the one the crate's own inline
  //     functions expect, so `rquickjs-sys` fails to compile against bindings
  //     it generated itself.
  //
  // Underscored triples, because that is the spelling both readers agree on:
  // the `cc` crate takes either and bindgen takes only this one.
  for (const abi of project.abis) {
    const triple = ANDROID_ABIS[abi];
    const key = triple.replace(/-/g, "_");
    environment[`RANLIB_${key}`] = join(toolchain.llvm, "bin", "llvm-ranlib");
    // The sysroot's per-target include directory is named for the ABI rather
    // than the Rust triple, and 32-bit ARM is the one place the two differ.
    const headers = triple === "armv7-linux-androideabi" ? "arm-linux-androideabi" : triple;
    environment[`BINDGEN_EXTRA_CLANG_ARGS_${key}`] =
      `--sysroot=${toolchain.sysroot} -I${join(toolchain.sysroot, "usr", "include", headers)}`;
  }
  // A release build is unsigned unless a key is named, and an unsigned APK
  // installs nowhere. The debug key is the default so that the first run
  // produces something runnable; the build prints what it signed with.
  const key = keystore ?? DEBUG_KEYSTORE();
  const password = keystore === null ? DEBUG_KEYSTORE_PASSWORD : keystorePassword;
  if (release) {
    environment[`CARGO_APK_${profile}_KEYSTORE`] = key;
    if (password === null) {
      throw new Error(`--android-keystore ${keystore} needs its password in `
        + "BLITSEN_ANDROID_KEYSTORE_PASSWORD. It is read from the environment rather than "
        + "taken as a flag because an argument is visible in `ps` and lands in shell history.");
    }
    environment[`CARGO_APK_${profile}_KEYSTORE_PASSWORD`] = password;
  }
  return {
    command: ["cargo-apk", "apk", "build", ...release ? ["--release"] : [], "--lib"],
    cwd: directory,
    environment,
    keystore: release ? key : DEBUG_KEYSTORE(),
    debugSigned: keystore === null,
    // Where cargo-apk leaves it: `target/<profile>/apk/<apk_name>.apk`.
    artifact: join(directory, "target", release ? "release" : "debug", "apk",
      `${project.apkName}.apk`),
  };
}

/// What the notices are called inside an APK, and why it is not
/// [`blitsen.notices.txt.gz`], which is what a desktop export carries.
///
/// Measured: `aapt` treats an asset whose name ends in `.gz` as pre-compressed,
/// **strips the suffix and stores the inflated bytes** under the shortened name.
/// An APK built with `blitsen.notices.txt.gz` staged into it contains
/// `blitsen.notices.txt` holding plain text, so a reader looking for the gzipped
/// name finds nothing and every Android artifact would report itself uncleared
/// for redistribution while carrying exactly the notices it owes. This is the
/// name that survives, and `blitsen_host::app::notices` reads both.
export const ANDROID_NOTICES_FILE = "blitsen.notices.txt";

/**
 * The third-party notices this artifact owes (#121), as an asset.
 *
 * On the six desktop targets these are generated where the runtime was built
 * and travel in its platform package, so the export copies rather than computes
 * them. Android has no platform package — that is the whole of decision 1 — so
 * there is nowhere for this path to find them, and the only source is
 * `BLITSEN_NOTICES_PATH`, which is what the release job that builds the entry
 * crate would set. Absent, the build says the artifact is not cleared for
 * redistribution, exactly as a desktop export without them does. Saying nothing
 * would be the one outcome docs/LICENSING.md does not allow.
 *
 * Staged uncompressed, for the `aapt` reason above. Nothing is lost: the
 * archive deflates its own entries, so the gzip the desktop bundle needs — that
 * one is a byte range inside an executable and has no compressor of its own —
 * would only have been undone and re-done here.
 */
export async function androidNotices(env = process.env) {
  const path = env.BLITSEN_NOTICES_PATH;
  if (!path) return null;
  const text = await readFile(path).catch(() => null);
  if (text === null) return null;
  return { path, bytes: text.length, file: ANDROID_NOTICES_FILE, contents: text };
}

/**
 * Step ④–⑤ for Android: stage the application, generate the project, run the
 * packager, and report the APK.
 *
 * Kept to the same `progress` protocol as the desktop export so `blitsen build`
 * prints one shape of output whichever artifact it is producing.
 */
export async function buildAndroid({
  root,
  name = "app",
  outfile = null,
  abis: requestedAbis,
  applicationId: requestedId = null,
  appVersion = "0.1.0",
  keystore = null,
  keystorePassword = null,
  release = true,
  include = [],
  force = false,
  extra = new Map(),
  progress = () => {},
  env = process.env,
  run = defaultRun,
  detect = detectAndroidToolchain,
  output = null,
}) {
  const abis = resolveAbis(requestedAbis);
  const id = applicationId(requestedId, name);
  const code = versionCode(appVersion);
  const toolchain = await detect({ env });
  const entryCrate = await resolveEntryCrate(env);
  const destination = resolve(outfile ?? join(process.cwd(), `${name}.apk`));
  if (!force && await stat(destination).catch(() => null)) {
    throw new Error(`output already exists: ${destination} (pass --force to replace it)`);
  }
  const directory = join(dirname(destination), `.${basename(destination)}.blitsen-android`);
  await rm(directory, { recursive: true, force: true });
  await mkdir(join(directory, "src"), { recursive: true });
  const staged = await stageAndroidAssets({
    root,
    directory: join(directory, "assets"),
    include,
    extra,
  });
  const project = androidProject({
    name, applicationId: id, version: appVersion, abis, entryCrate,
    patches: await workspacePatches(entryCrate),
  });
  await writeFile(join(directory, "Cargo.toml"), project.cargoToml);
  await writeFile(join(directory, "src", "lib.rs"), project.libRs);
  progress({
    step: "collect",
    detail: `${staged.files.length} assets under assets/${ASSET_ROOT}/`,
    notes: [
      `application ID ${id}, version ${appVersion} (versionCode ${code}), `
        + `minSdk ${MIN_SDK}, targetSdk ${TARGET_SDK}`,
      ...staged.unreferenced.length === 0 ? [] : [
        `dropped ${staged.unreferenced.length} files unreachable from index.html `
        + "(--include <glob> keeps them)",
      ],
    ],
  });
  const plan = apkPlan({ project, directory, toolchain, release, keystore, keystorePassword });
  const unproven = abis.filter(abi => UNPROVEN_ABIS.includes(abi));
  const stale = await missingRustTargets(abis.map(abi => ANDROID_ABIS[abi]), run);
  if (stale !== null && stale.length > 0) {
    throw missing(`the Rust ${stale.length === 1 ? "target" : "targets"} `
      + `${stale.join(", ")} ${stale.length === 1 ? "is" : "are"} not installed, `
      + "so this APK cannot be cross-compiled.",
      `Install ${stale.length === 1 ? "it" : "them"} — \`rustup target add ${stale.join(" ")}\`.`);
  }
  progress({
    step: "link",
    detail: `cargo-apk ${release ? "release" : "debug"}: ${abis.join(", ")}`,
    notes: [
      `NDK ${basename(toolchain.ndk)}, build-tools ${toolchain.buildToolsVersion}`,
      ...unproven.length === 0 ? [] : [
        `${unproven.join(", ")} is built on request and has never been run by this project: `
        + "32-bit ARM is where #139 records Vello as unproven, and nothing here has measured it",
      ],
      ...release ? [] : [
        "--android-debug: assets are stored uncompressed, which is what apk.rs's "
        + "read-in-place design wants, and the Rust is unoptimised",
      ],
    ],
  });
  const result = await run(plan.command, { cwd: plan.cwd, environment: plan.environment, output });
  if (result.code !== 0) {
    throw new Error(`cargo-apk exited ${result.code}; the APK was not built`);
  }
  await rm(destination, { force: true });
  await writeFile(destination, await readFile(plan.artifact));
  const bytes = (await stat(destination)).size;
  progress({
    step: "package",
    detail: `${destination} (${abis.join(", ")})`,
    notes: [
      plan.debugSigned
        ? "signed with the Android debug key: installable and not distributable. "
          + "Pass --android-keystore, with its password in BLITSEN_ANDROID_KEYSTORE_PASSWORD, "
          + "to sign with a real one — an Android install is keyed by its signing certificate "
          + "and refuses an update signed by another"
        : `signed with ${plan.keystore}`,
      ...release ? [
        "this is an APK and not an AAB: it sideloads and installs on an emulator, and "
        + "Google Play has required an App Bundle for new applications since August 2021. "
        + "The packager Blitsen drives emits no AAB (see android.mjs)",
        "release assets are deflated inside the APK, so each read inflates: cargo-apk stores "
        + "assets uncompressed only in a debug build and offers no way to ask for it here "
        + "(issue #144's noCompress)",
      ] : [],
    ],
  });
  return {
    outfile: destination,
    abis,
    applicationId: id,
    versionCode: code,
    assets: staged.files.length,
    unreferenced: staged.unreferenced,
    bytes,
    keystore: plan.keystore,
    debugSigned: plan.debugSigned,
    release,
    toolchain,
    project,
  };
}
