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
//     which every SDK install has and which [`ensureDebugKeystore`] creates
//     through `keytool` if it does not. That is what makes
//     `blitsen build --android` produce something installable on the first run
//     with nothing configured, which is the whole reason it is the default. It
//     is also not distributable, and the build says so on every run rather than
//     in the documentation.
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
// # 4. The toolchain: the SDK's own tools, behind a seam
//
// This decision was made twice and the second answer is the one in the code,
// because the first one was tried and measured.
//
// The first was **`cargo apk`**: one binary, no Gradle, no JDK, no checked-in
// Android project, and what Blitz drives against this exact stack. #139
// measured it producing a signed, installable APK from this dependency graph,
// and #148 landed against it. Two things then killed it, in order of when they
// were found:
//
//   - **`noCompress` is inexpressible**, and it is issue #144's one packaging
//     request. `cargo apk` sets `disable_aapt_compression` from
//     `is_debug_profile` and offers no override, so a debug APK stores every
//     asset and a release APK deflates every asset. `blitsen_host::apk` reads
//     an asset as a pointer into the mapped archive; a deflated entry is
//     inflated into a heap buffer on every open. The design was therefore
//     undone on exactly the profile anyone ships.
//   - **It cannot build the entry crate.** `cargo apk` takes the application's
//     identity, label, assets directory and ABI list out of
//     `[package.metadata.android]` in a `Cargo.toml`, so the crate it packages
//     has to be generated per build — which means the entry point has to be
//     reachable from a *generated* cdylib, which means an rlib and a macro.
//     #142 landed the opposite: `crates/blitsen-android` is itself the cdylib,
//     with `#[unsafe(no_mangle)] pub fn android_main` in it and no rlib to
//     re-export from. #143 then built and ran the artifact, and #142's
//     arrangement is the one that links.
//
// So: **`cargo ndk` compiles and the SDK's own build-tools package.** That is
// #143's spike, `spikes/s9/run.sh`, which produced the first APK that has ever
// painted a Blitsen document:
//
//     cargo ndk -t <abi> -P <min sdk> build --release -p blitsen-android
//     aapt2 link --output-to-dir ...        the binary manifest and resources.arsc
//     <write the archive, every entry stored>
//     zipalign -f -p 4                      4 bytes, and a page for the .so
//     apksigner sign                        v1, v2 and v3
//
// What that buys over the packager, all of it measurable in the archive:
//
//   - **Every entry is stored, on every profile.** #144's requirement, and the
//     precondition for `android:extractNativeLibs="false"`, which is what stops
//     a 35 MB shared object being copied out of the APK at install time.
//   - **`aapt2`**, which is the tool Google is keeping, rather than `aapt` v1,
//     which it is removing.
//   - **`zipalign -p`**, so the shared object is page-aligned rather than
//     aligned to four bytes. Android 15's 16 KB page devices want this.
//   - **The version code is ours** (see [`versionCode`]). `cargo apk` panicked
//     if the manifest carried one and imposed a scheme with a byte per
//     component; writing the manifest means writing the number in it.
//   - **No `cargo apk`, and no `xbuild` either** — #139 found that deprecation
//     chain broken at both ends. This depends on nothing whose maintenance is
//     in question: `cargo ndk` for the cross-compile, and three tools out of
//     the SDK that every Android build in the world runs.
//
// What it still gives up:
//
//   - **No AAB.** Google Play has required an Android App Bundle for new
//     applications since August 2021. This produces a sideloadable and
//     emulator-installable artifact — a developer's inner loop and #149's CI —
//     and *not* a Play upload. That is now the whole of what the Gradle path
//     would buy, and it is not delivered here.
//   - **No resources**, so no launcher icon: `--icon` is refused rather than
//     ignored. An Android icon is a multi-density drawable set plus an
//     adaptive-icon XML, which is `aapt2 compile` and a `res/` tree.
//   - **A JRE is required**, because `apksigner` is a shell script around a jar
//     — and `keytool`, from a JDK, if the debug keystore has to be created.
//
// The seam that makes a Gradle backend affordable later is that nothing below
// runs a command directly. [`androidProject`] describes the project — its
// manifest, its ABIs, where its assets are — and [`apkPlan`] turns that
// description into the steps that realise it. A Gradle backend is a second
// function of the same shape reading the same description, not a rewrite of the
// build. The description is deliberately expressed in Android's vocabulary
// (application ID, version code, ABI names) rather than in any tool's.
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

import { access, mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { constants } from "node:fs";
import { homedir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { androidManifest, apkEntries, storedZip } from "./android-apk.mjs";
import { ASSET_ROOT, stageAndroidAssets } from "./android-assets.mjs";
import {
  cargoTargetDirectory, defaultRun, detectAndroidToolchain, ENTRY_CRATE, ENTRY_LIBRARY, ENTRY_SO,
  MIN_SDK, missing, missingRustTargets, resolveEntryCrate, TARGET_SDK,
} from "./android-toolchain.mjs";

// Re-exported so that `--android` has one module on its surface: everything a
// caller or a test asks for about an Android build is asked of this file, and
// the split behind it is an implementation detail of where the reasoning lives.
export {
  cargoTargetDirectory, detectAndroidToolchain, ENTRY_CRATE, ENTRY_LIBRARY, ENTRY_SO, findLibclang,
  MIN_SDK, missingRustTargets, resolveEntryCrate, TARGET_SDK,
} from "./android-toolchain.mjs";
export { androidManifest, apkEntries, CONFIG_CHANGES, storedZip } from "./android-apk.mjs";

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
export const DEBUG_KEYSTORE_ALIAS = "androiddebugkey";

/// What the signing passwords are called in the environment `apksigner` is
/// handed. Private to this process — they are not the variables a user sets,
/// which are `BLITSEN_ANDROID_KEYSTORE_PASSWORD` and
/// `BLITSEN_ANDROID_KEY_PASSWORD`, which `cli.mjs` reads — and they exist
/// because `apksigner`'s
/// `env:` scheme is the only one of its four that keeps a password out of both
/// the argv and the filesystem.
const KEYSTORE_PASSWORD_VARIABLE = "BLITSEN_APKSIGNER_KEYSTORE_PASSWORD";
const KEY_PASSWORD_VARIABLE = "BLITSEN_APKSIGNER_KEY_PASSWORD";

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

/// The largest version code Google Play accepts. Not a limit of the field,
/// which is a signed 32-bit integer, but a limit of the one place the number
/// has consequences.
const VERSION_CODE_CEILING = 2_100_000_000;

/// What each semver component is multiplied by. `major` is bounded by the
/// ceiling above rather than stated here.
const VERSION_CODE_PLACES = { minor: 1_000, patch: 1 };

/**
 * The integer Android orders installs by, in decimal places that read back.
 *
 * `major * 1_000_000 + minor * 1_000 + patch`: monotonic under semver,
 * legible at a glance in Play's console and in `dumpsys package`, and inside
 * the ceiling above for every major version below 2100.
 *
 * This is a decision that was made twice and reversed. The first answer was
 * this scheme, and it was abandoned because `cargo apk` **panics** if the
 * manifest carries a version code at all — it derives one as
 * `apk_id << 24 | major << 16 | minor << 8 | patch`, a byte per component, and
 * treats a value already there as a bug. So what Blitsen reported had to be the
 * packager's number rather than a better one, since reporting a number the
 * artifact does not carry is wrong in the one place anyone checks it.
 *
 * Decision 4 then dropped `cargo apk`, and this build writes the manifest
 * itself. The constraint is gone with the packager, and with it the ceiling of
 * 255 per component that made `1.0.256` unshippable. What remains is refused
 * up front rather than at upload, because a version code cannot be walked back:
 * Play refuses an upload numbered at or below one already published, forever.
 *
 * Pre-release and build metadata are dropped. Android has nowhere to put them,
 * and `versionName` carries the whole string including them.
 */
export function versionCode(version) {
  const core = String(version ?? "0.0.0").split(/[-+]/)[0];
  const parts = core.split(".");
  if (parts.length !== 3 || parts.some(part => !/^\d+$/.test(part))) {
    throw new Error(`--app-version ${JSON.stringify(version)} is not a version Android can order: `
      + "expected major.minor.patch in digits");
  }
  const [major, minor, patch] = parts.map(Number);
  const over = ["minor", "patch"].filter((name, index) =>
    [minor, patch][index] >= VERSION_CODE_PLACES.minor);
  if (over.length > 0) {
    throw new Error(`--app-version ${JSON.stringify(version)} cannot be given an Android version `
      + `code: each component below the major occupies three decimal places, so ${over.join(" and ")} `
      + "must be below 1000. See the note on versionCode in packages/blitsen/src/android.mjs.");
  }
  const code = major * 1_000_000 + minor * VERSION_CODE_PLACES.minor + patch;
  if (code > VERSION_CODE_CEILING) {
    throw new Error(`--app-version ${JSON.stringify(version)} gives an Android version code of `
      + `${code}, and Google Play refuses anything above ${VERSION_CODE_CEILING}. `
      + "See the note on versionCode in packages/blitsen/src/android.mjs.");
  }
  return code;
}

/**
 * The project, described in Android's own vocabulary.
 *
 * This is the seam. Everything a backend needs to know about the artifact is
 * decided here and nothing about *how* it is produced is: the manifest is XML
 * that `aapt2` and Gradle would both accept, the ABI list is Android's names
 * rather than Rust triples, and the version code is an integer rather than a
 * scheme some packager imposes.
 *
 * There is no generated crate. That was the shape of this function until #143
 * built an APK against the real entry point and found it did not fit: #142's
 * `crates/blitsen-android` is a `cdylib` exporting `android_main` itself, so
 * there is no rlib for a generated crate to depend on and nothing for it to
 * re-export. The library is [`ENTRY_LIBRARY`] on every build, which is what
 * `android.app.lib_name` names and what `NativeActivity` calls `dlopen` with,
 * and it is a constant because the crate's `[lib] name` is.
 */
export function androidProject({
  name,
  applicationId: id,
  version = "0.1.0",
  abis = DEFAULT_ABIS,
  debuggable = false,
}) {
  const code = versionCode(version);
  return {
    applicationId: id,
    label: name,
    library: ENTRY_LIBRARY,
    soName: ENTRY_SO,
    version,
    versionCode: code,
    abis,
    manifest: androidManifest({
      applicationId: id,
      label: name,
      versionCode: code,
      versionName: version,
      library: ENTRY_LIBRARY,
      minSdk: MIN_SDK,
      targetSdk: TARGET_SDK,
      debuggable,
    }),
  };
}

/**
 * The steps that turn that description into a signed APK, as values.
 *
 * Split out from running them so the whole decision is something a test can
 * read rather than something only a machine with an NDK can observe. Each step
 * is one argv plus the environment it needs; the archive between `link` and
 * `align` is written in process, because writing a stored-only zip is the one
 * thing no tool in the SDK will do (see `android-apk.mjs`).
 *
 * Passwords are in the environment and never in the argv, for the reason
 * decision 3 gives — an argument is visible in `ps` and lands in shell history
 * and CI logs — and `apksigner` supports exactly that with `env:`.
 */
export function apkPlan({
  project,
  directory,
  toolchain,
  entryCrate,
  targetDirectory,
  release = true,
  keystore = null,
  keystorePassword = null,
  keyAlias = null,
  keyPassword = null,
}) {
  const profile = release ? "release" : "debug";
  const environment = {
    ANDROID_HOME: toolchain.sdk,
    ANDROID_SDK_ROOT: toolchain.sdk,
    ANDROID_NDK_HOME: toolchain.ndk,
    ANDROID_NDK_ROOT: toolchain.ndk,
    LIBCLANG_PATH: toolchain.libclang,
  };
  // The two variables this graph needs beyond a compiler and an archiver.
  // Neither is visible from reading; both were found by building an APK against
  // Blitsen's own dependencies for the first time (#149), when the compile was
  // still driven by a packager that set `CC_`, `CFLAGS_`, `AR_` and a linker
  // per target and stopped there — enough for a crate whose C is one
  // `cc::Build`, and not enough for this one.
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
  //
  // Decision 4 then replaced that packager with `cargo ndk`, which sets both
  // itself, to these same two paths, and wins where the two disagree — so on
  // the path the CLI takes today these are belt and braces rather than the
  // difference between building and not. They stay because this plan is the
  // record of what the compile needs, and a record that leaves out the two
  // things a person would spend a day rediscovering is not one.
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
  if (password === null) {
    throw new Error(`--android-keystore ${keystore} needs its password in `
      + "BLITSEN_ANDROID_KEYSTORE_PASSWORD. It is read from the environment rather than "
      + "taken as a flag because an argument is visible in `ps` and lands in shell history.");
  }
  // A keystore with one key needs no alias, which is every keystore anyone
  // makes for one application, so the alias is omitted rather than guessed. The
  // debug keystore is the exception because its alias is a fixed convention.
  const alias = keystore === null ? DEBUG_KEYSTORE_ALIAS : keyAlias;
  const paths = {
    manifest: join(directory, "AndroidManifest.xml"),
    linked: join(directory, "linked"),
    unaligned: join(directory, "unaligned.apk"),
    apk: join(directory, "aligned.apk"),
  };
  return {
    paths,
    // `-P` is the API level the NDK toolchain compiles against, and it is
    // MIN_SDK rather than TARGET_SDK on purpose: it decides which libc symbols
    // the shared object may bind, and a library built against 33 while the
    // manifest claims 26 fails to `dlopen` on every device in between, with no
    // diagnostic beyond a linker line in logcat.
    compile: {
      command: ["cargo", "ndk", ...project.abis.flatMap(abi => ["-t", abi]),
        "-P", String(MIN_SDK), "build", ...release ? ["--release"] : [],
        "--manifest-path", join(entryCrate, "Cargo.toml"), "-p", ENTRY_CRATE],
      environment,
    },
    libraries: project.abis.map(abi => ({
      abi,
      triple: ANDROID_ABIS[abi],
      source: join(targetDirectory, ANDROID_ABIS[abi], profile, ENTRY_SO),
      entry: `lib/${abi}/${ENTRY_SO}`,
    })),
    // No `res/` and no `-A`: there are no resources to compile, and the assets
    // go into the archive uncompressed, which is the one thing aapt2 would take
    // away again.
    link: {
      command: [toolchain.tools.aapt2, "link", "-o", paths.linked, "--output-to-dir",
        "-I", toolchain.platform, "--manifest", paths.manifest,
        "--min-sdk-version", String(MIN_SDK), "--target-sdk-version", String(TARGET_SDK)],
    },
    // `-p` aligns an uncompressed `.so` to a page rather than to four bytes,
    // which is what `android:extractNativeLibs="false"` needs and what Android
    // 15's 16 KB page devices want.
    align: {
      command: [toolchain.tools.zipalign, "-f", "-p", "4", paths.unaligned, paths.apk],
    },
    sign: {
      command: [toolchain.tools.apksigner, "sign", "--ks", key,
        "--ks-pass", `env:${KEYSTORE_PASSWORD_VARIABLE}`,
        ...alias === null ? [] : ["--ks-key-alias", alias],
        "--key-pass", `env:${KEY_PASSWORD_VARIABLE}`,
        "--min-sdk-version", String(MIN_SDK), paths.apk],
      environment: {
        [KEYSTORE_PASSWORD_VARIABLE]: password,
        // Defaulted to the keystore's, which is what `keytool` writes when it
        // is not asked for two, rather than left for apksigner to prompt for on
        // a terminal a build may not have.
        [KEY_PASSWORD_VARIABLE]: keystore === null ? DEBUG_KEYSTORE_PASSWORD
          : (keyPassword ?? password),
      },
    },
    keystore: key,
    keystorePassword: password,
    debugSigned: keystore === null,
    artifact: paths.apk,
  };
}

/**
 * Creates the Android debug keystore if there is not one, exactly as every
 * other Android toolchain does.
 *
 * This is the one thing in the Android path that writes outside the build
 * directory, and it is deliberate: `~/.android/debug.keystore` with the
 * well-known password is a shared convention, not Blitsen's file, and creating
 * it is what `cargo apk`, Gradle and Android Studio all do on a first build.
 * The alternative is that a machine which has never opened Android Studio gets
 * an unsigned APK and an error, which is decision 3's default not working.
 */
export async function ensureDebugKeystore(path, run = defaultRun) {
  if (await access(path, constants.R_OK).then(() => true, () => false)) return false;
  await mkdir(dirname(path), { recursive: true });
  const result = await run(["keytool", "-genkeypair", "-keystore", path,
    "-storepass", DEBUG_KEYSTORE_PASSWORD, "-keypass", DEBUG_KEYSTORE_PASSWORD,
    "-alias", DEBUG_KEYSTORE_ALIAS, "-keyalg", "RSA", "-keysize", "2048",
    "-validity", "10000", "-dname", "CN=Android Debug, O=Android, C=US"], { capture: true });
  if (result.code !== 0) {
    throw missing(`there is no Android debug keystore at ${path} and keytool could not create `
      + `one (exit ${result.code}), so this APK could not be signed.`,
      "Install a JDK so that `keytool` is on PATH, or pass --android-keystore with a key of "
      + "your own. The password in the argv above is the well-known Android debug one, which is "
      + "why it is the only password this build ever writes there.");
  }
  return true;
}

/// What the notices are called inside an APK, and why it is not
/// [`blitsen.notices.txt.gz`], which is what a desktop export carries.
///
/// The name was forced before it was chosen. `aapt` v1 treats an asset whose
/// name ends in `.gz` as pre-compressed, **strips the suffix and stores the
/// inflated bytes** under the shortened name — measured on a real APK — so
/// every Android artifact built through `cargo apk` would have reported itself
/// uncleared for redistribution while carrying exactly the notices it owes,
/// because the reader was looking for a name that no longer existed.
///
/// Decision 4 dropped `aapt` v1 with the packager, and the name would now
/// survive. It stays anyway, and now for a reason rather than a workaround:
/// every entry in the archive is stored, so a gzip inside it compresses nothing
/// the APK was going to compress and costs an inflate on the one read that
/// matters. `blitsen_host::app::notices` reads both names, so an APK built by
/// either path is understood.
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
 * Staged uncompressed, for the reason above. The gzip the desktop bundle needs
 * is a property of *that* container — a byte range inside an executable, with
 * no compressor of its own — and an APK entry has nowhere to put it that would
 * not have to be undone on the one read that asks for it.
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
  keyAlias = null,
  keyPassword = null,
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
  // The Activity can report its installed ID, but the engine-neutral runtime
  // config is also what desktop reads. Keep one identity record across every
  // artifact shape rather than teaching storage about Android packaging.
  extra = new Map(extra);
  extra.set("blitsen.runtime.json", Buffer.from(
    `${JSON.stringify({ storageIdentity: id })}\n`));
  const code = versionCode(appVersion);
  const toolchain = await detect({ env });
  const entryCrate = await resolveEntryCrate(env);
  const destination = resolve(outfile ?? join(process.cwd(), `${name}.apk`));
  if (!force && await stat(destination).catch(() => null)) {
    throw new Error(`output already exists: ${destination} (pass --force to replace it)`);
  }
  const directory = join(dirname(destination), `.${basename(destination)}.blitsen-android`);
  await rm(directory, { recursive: true, force: true });
  await mkdir(join(directory, "linked"), { recursive: true });
  const staged = await stageAndroidAssets({
    root,
    directory: join(directory, "assets"),
    include,
    extra,
  });
  const project = androidProject({
    name, applicationId: id, version: appVersion, abis, debuggable: !release,
  });
  await writeFile(join(directory, "AndroidManifest.xml"), project.manifest);
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
  const unproven = abis.filter(abi => UNPROVEN_ABIS.includes(abi));
  const stale = await missingRustTargets(abis.map(abi => ANDROID_ABIS[abi]), run);
  if (stale !== null && stale.length > 0) {
    throw missing(`the Rust ${stale.length === 1 ? "target" : "targets"} `
      + `${stale.join(", ")} ${stale.length === 1 ? "is" : "are"} not installed, `
      + "so this APK cannot be cross-compiled.",
      `Install ${stale.length === 1 ? "it" : "them"} — \`rustup target add ${stale.join(" ")}\`.`);
  }
  const targetDirectory = await cargoTargetDirectory(entryCrate, run);
  const plan = apkPlan({
    project, directory, toolchain, entryCrate, targetDirectory, release, keystore, keystorePassword,
    keyAlias, keyPassword,
  });
  progress({
    step: "link",
    detail: `cargo ndk ${release ? "release" : "debug"}: ${abis.join(", ")}`,
    notes: [
      `${ENTRY_CRATE} at ${entryCrate}, NDK ${basename(toolchain.ndk)} at API ${MIN_SDK}, `
        + `build-tools ${toolchain.buildToolsVersion}`,
      ...unproven.length === 0 ? [] : [
        `${unproven.join(", ")} is built on request and has never been run by this project: `
        + "32-bit ARM is where #139 records Vello as unproven, and nothing here has measured it",
      ],
      ...release ? [] : ["--android-debug: the Rust is unoptimised and the manifest is debuggable"],
    ],
  });
  const compiled = await run(plan.compile.command,
    { environment: plan.compile.environment, output });
  if (compiled.code !== 0) {
    throw new Error(`cargo ndk exited ${compiled.code}; ${ENTRY_SO} was not built`);
  }
  const linked = await run(plan.link.command, { environment: plan.compile.environment, output });
  if (linked.code !== 0) {
    throw new Error(`aapt2 link exited ${linked.code}; the APK manifest was not compiled`);
  }
  // The archive, written here rather than by a tool, because "every entry
  // stored" is the one instruction none of them takes. See android-apk.mjs.
  const entries = await apkEntries({
    linked: plan.paths.linked,
    libraries: plan.libraries,
    assets: join(directory, "assets"),
  });
  await writeFile(plan.paths.unaligned, storedZip(entries));
  const aligned = await run(plan.align.command, { environment: plan.compile.environment, output });
  if (aligned.code !== 0) {
    throw new Error(`zipalign exited ${aligned.code}; the APK was not aligned`);
  }
  const created = plan.debugSigned ? await ensureDebugKeystore(plan.keystore, run) : false;
  const signature = await run(plan.sign.command,
    { environment: { ...plan.compile.environment, ...plan.sign.environment }, output });
  if (signature.code !== 0) {
    throw new Error(`apksigner exited ${signature.code}; the APK was not signed and cannot be `
      + "installed");
  }
  await rm(destination, { force: true });
  await writeFile(destination, await readFile(plan.artifact));
  const bytes = (await stat(destination)).size;
  const library = entries.find(entry => entry.name.startsWith("lib/"));
  progress({
    step: "package",
    detail: `${destination} (${abis.join(", ")})`,
    notes: [
      plan.debugSigned
        ? "signed with the Android debug key: installable and not distributable. "
          + `${created ? `Created ${plan.keystore}, because there was none. ` : ""}`
          + "Pass --android-keystore, with its password in BLITSEN_ANDROID_KEYSTORE_PASSWORD, "
          + "to sign with a real one — an Android install is keyed by its signing certificate "
          + "and refuses an update signed by another"
        : `signed with ${plan.keystore}`,
      `every entry is stored rather than deflated (issue #144's noCompress) and the `
        + `${abis.length === 1 ? "shared object is" : "shared objects are"} page-aligned, so the `
        + "assets are read in place and the libraries are not extracted at install"
        + (library ? ` — ${library.name} is ${library.data.length} bytes` : ""),
      "this is an APK and not an AAB: it sideloads and installs on an emulator, and "
      + "Google Play has required an App Bundle for new applications since August 2021. "
      + "Nothing on this path emits an AAB (see android.mjs)",
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
    entries: entries.map(entry => ({ name: entry.name, bytes: entry.data.length })),
    keystore: plan.keystore,
    debugSigned: plan.debugSigned,
    release,
    toolchain,
    project,
  };
}
