// Android builds produce a signed, multi-ABI APK instead of linking a prebuilt
// runtime into an executable, so `--android` remains distinct from `--target`.
// ARM64 and the emulator's x86_64 are the defaults; 32-bit ARM is opt-in and
// unproven.
//
// The debug keystore makes local builds installable, but is not distributable.
// Release passwords come from the environment because command-line values leak
// through process listings, shell history, and CI logs.
//
// Cargo NDK cross-compiles the entry library; SDK tools create and sign the APK.
// Every archive entry must remain stored, and native libraries page-aligned, so
// assets can be memory-mapped and `extractNativeLibs=false` remains valid. This
// path produces sideloadable APKs, not AABs, and intentionally installs no SDK,
// NDK, JDK, Rust target, or other toolchain component.

import { access, mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { constants } from "node:fs";
import { homedir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
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
const UNPROVEN_ABIS = ["armeabi-v7a"];

/// The Android debug key, as every SDK install has it.
const DEBUG_KEYSTORE = () => join(homedir(), ".android", "debug.keystore");
const DEBUG_KEYSTORE_PASSWORD = "android";
const DEBUG_KEYSTORE_ALIAS = "androiddebugkey";

/** The complete Java surface packaged for notification activation (#252). */
export const NOTIFICATION_BRIDGE_SOURCE = fileURLToPath(
  new URL("./android/NotificationBridge.java", import.meta.url));

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
 * The Java step is deliberately not Gradle: one checked-in source file is
 * compiled against `android.jar` and D8 turns its two class files into the
 * one `classes.dex` the archive carries.
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
    classes: join(directory, "classes"),
    dex: join(directory, "dex"),
    classesDex: join(directory, "dex", "classes.dex"),
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
    javaCompile: {
      command: [toolchain.tools.javac, "-source", "8", "-target", "8", "-bootclasspath",
        toolchain.platform, "-d", paths.classes, NOTIFICATION_BRIDGE_SOURCE],
      environment,
    },
    dex: {
      command: [toolchain.tools.d8, "--min-api", String(MIN_SDK), "--output", paths.dex,
        ...["NotificationBridge.class", "NotificationBridge$ActivationReceiver.class"]
          .map(name => join(paths.classes, "com", "blitsen", "runtime", name))],
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
  await mkdir(join(directory, "classes"), { recursive: true });
  await mkdir(join(directory, "dex"), { recursive: true });
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
  const javaCompiled = await run(plan.javaCompile.command,
    { environment: plan.javaCompile.environment, output });
  if (javaCompiled.code !== 0) {
    throw new Error(`javac exited ${javaCompiled.code}; the notification activation bridge was `
      + "not compiled");
  }
  const dexed = await run(plan.dex.command, { environment: plan.dex.environment, output });
  if (dexed.code !== 0) {
    throw new Error(`d8 exited ${dexed.code}; classes.dex was not produced`);
  }
  const linked = await run(plan.link.command, { environment: plan.compile.environment, output });
  if (linked.code !== 0) {
    throw new Error(`aapt2 link exited ${linked.code}; the APK manifest was not compiled`);
  }
  // The archive, written here rather than by a tool, because "every entry
  // stored" is the one instruction none of them takes. See android-apk.mjs.
  const entries = await apkEntries({
    linked: plan.paths.linked,
    dex: plan.paths.classesDex,
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
