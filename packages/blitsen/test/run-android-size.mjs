// Issue #150: what a release APK weighs, and where the bytes are.
//
// The Android counterpart to `run-phase2-size.mjs`. Same bare application
// (`bare-app.mjs`), same question — where does the shipped artifact's size
// actually go — against an artifact with a different shape: an APK carries one
// shared object per ABI rather than one executable, its assets are packaged
// rather than appended, and the number a user downloads depends on whether the
// ABIs ship together or apart.
//
//     bun run --cwd packages/blitsen size:android [options]
//       --abis arm64-v8a,x86_64   which ABIs to build and package
//       --attribute               also build the symbol-bearing profile and
//                                 attribute bytes to OpenSSL and QuickJS-ng
//       --bundletool <jar>        also build an AAB and ask bundletool what
//                                 Play would deliver per ABI
//       --out measurements.json   write the record
//
// # Why this does not go through `blitsen build --android`
//
// It measures the same pipeline the CLI drives — `cargo ndk` for the library,
// then a packaging step — but assembles the APK with `aapt2` + `zip -0` +
// `zipalign` + `apksigner` rather than `cargo apk`. Two reasons, both about the
// measurement rather than about the CLI. `cargo apk` deflates every asset in a
// release build and stores every asset in a debug one, with no override
// (#148), so its release APK is not the artifact #144's design describes.
// And `spikes/s9` proved this exact path produces an APK that runs, so what is
// weighed here is a thing that works rather than a plausible archive.
import { execFile } from "node:child_process";
import { cp, mkdir, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { gzipSync } from "node:zlib";
import { tmpdir } from "node:os";
import { homedir } from "node:os";
import { basename, join } from "node:path";
import { promisify } from "node:util";

import { ANDROID_ABIS, DEFAULT_ABIS, resolveAbis } from "../src/android.mjs";
import { MIN_SDK, TARGET_SDK, detectAndroidToolchain, missing } from "../src/android-toolchain.mjs";
import { BARE_APP } from "./bare-app.mjs";
import { stageAndroidAssets } from "../src/android-assets.mjs";
import { repository } from "./build-addon.mjs";

const run = promisify(execFile);
const argv = process.argv.slice(2);
const flag = name => {
  const index = argv.indexOf(name);
  return index === -1 ? null : argv[index + 1];
};
const abis = resolveAbis(flag("--abis")?.split(",").map(value => value.trim()) ?? DEFAULT_ABIS);
const attribute = argv.includes("--attribute");
const bundletool = flag("--bundletool");
const outFile = flag("--out");

const bytes = async path => (await stat(path)).size;
const gzipped = async path => gzipSync(await readFile(path), { level: 9 }).length;
const mb = value => `${(value / 1e6).toFixed(1)} MB`;
const pad = (value, width) => String(value).padStart(width);

const PACKAGE = "dev.blitsen.size";
// The manifest a release artifact carries. `debuggable` is absent — spikes/s9's
// manifest sets it, and an APK measured with it on is not the one that ships.
// `extractNativeLibs="false"` is what makes the stored, page-aligned `.so` in
// the archive the only copy on the device, and is why the library is not
// deflated below.
const MANIFEST = `<?xml version="1.0" encoding="utf-8"?>
<manifest xmlns:android="http://schemas.android.com/apk/res/android" package="${PACKAGE}"
    android:versionCode="1" android:versionName="1.0">
    <uses-sdk android:minSdkVersion="${MIN_SDK}" android:targetSdkVersion="${TARGET_SDK}" />
    <application android:label="Bare" android:hasCode="false" android:extractNativeLibs="false">
        <activity android:name="android.app.NativeActivity" android:exported="true">
            <meta-data android:name="android.app.lib_name" android:value="blitsen_android" />
            <intent-filter>
                <action android:name="android.intent.action.MAIN" />
                <category android:name="android.intent.category.LAUNCHER" />
            </intent-filter>
        </activity>
    </application>
</manifest>
`;

async function toolchain() {
  const found = await detectAndroidToolchain();
  const aapt2 = join(found.buildTools, "aapt2");
  await stat(aapt2).catch(() => {
    throw missing(`build-tools ${found.buildToolsVersion} has no aapt2, which links the APK.`,
      "Install a set that has one — `sdkmanager \"build-tools;34.0.0\"`.");
  });
  const keystore = process.env.BLITSEN_ANDROID_KEYSTORE
    ?? join(homedir(), ".android", "debug.keystore");
  await stat(keystore).catch(() => {
    throw missing(`no keystore at ${keystore}, and an unsigned APK is not the artifact.`,
      "Create the debug one — `keytool -genkeypair -keystore ~/.android/debug.keystore "
      + "-alias androiddebugkey -storepass android -keypass android -dname CN=Android,O=Android "
      + "-keyalg RSA -validity 10000` — or set BLITSEN_ANDROID_KEYSTORE.");
  });
  // The NDK's own binutils, which is where `llvm-strip` and `llvm-size` come
  // from — the host's `strip` does not know these targets, and the prebuilt
  // directory is named after the machine running the build rather than the one
  // being built for.
  const host = process.platform === "darwin" ? "darwin-x86_64" : `${process.platform}-x86_64`;
  const llvm = join(found.ndk, "toolchains/llvm/prebuilt", host, "bin");
  return { ...found, aapt2, keystore, llvm };
}

/// One ABI's shared object, built the way #143 built the one that ran.
///
/// `[profile.release]` sets `strip = "symbols"`, so what cargo writes is
/// already the shipped artifact; the extra `llvm-strip` is measured rather than
/// assumed to be a no-op, because "the release profile already strips" is
/// exactly the kind of claim this repository has been wrong about before.
async function library(tools, abi, profile = "release") {
  const triple = ANDROID_ABIS[abi];
  await run("cargo", ["ndk", "-t", abi, "-P", String(TARGET_SDK),
    "build", "--profile", profile, "-p", "blitsen-android"], {
    cwd: repository,
    env: { ...process.env, ANDROID_NDK_HOME: tools.ndk, ANDROID_HOME: tools.sdk },
    maxBuffer: 64 * 1024 * 1024,
  });
  const path = join(repository, "target", triple, profile, "libblitsen_android.so");
  const linked = await bytes(path);
  const copy = `${path}.size-probe`;
  await cp(path, copy);
  await run(join(tools.llvm, "llvm-strip"), [copy]);
  const stripped = await bytes(copy);
  await rm(copy, { force: true });
  const { stdout } = await run(join(tools.llvm, "llvm-size"), ["-A", path]);
  const sections = Object.fromEntries(stdout.split("\n")
    .map(line => line.trim().split(/\s+/))
    .filter(parts => parts.length >= 2 && parts[0].startsWith("."))
    .map(parts => [parts[0], Number(parts[1])])
    .filter(([, size]) => Number.isFinite(size)));
  return { abi, triple, profile, path, bytes: linked, stripped, sections };
}

/// The bare application staged as an APK's `assets/`, through the real writer.
async function assets(directory) {
  const source = join(directory, "app");
  await mkdir(source, { recursive: true });
  await writeFile(join(source, "index.html"), BARE_APP);
  const root = join(directory, "assets");
  const staged = await stageAndroidAssets({ root: source, directory: root });
  const total = staged.files.reduce((sum, file) => sum + file.bytes, 0) + staged.index.length;
  return { root, files: staged.files, bytes: total };
}

/**
 * Links, aligns and signs one APK holding the named ABIs.
 *
 * `compress` picks the zip level for `lib/` and `assets/`, which is the whole
 * of the per-ABI-versus-universal argument in miniature: at `-0` the `.so` is
 * mapped out of the archive and never extracted, and the APK is as large as
 * what it holds; at `-9` the download is smaller and the platform writes a
 * second copy of every library into `/data` at install.
 */
async function apk(tools, directory, built, staged, { compress, name }) {
  const stage = join(directory, `stage-${name}`);
  for (const one of built) {
    await mkdir(join(stage, "lib", one.abi), { recursive: true });
    await run(join(tools.llvm, "llvm-strip"),
      ["-o", join(stage, "lib", one.abi, "libblitsen_android.so"), one.path]);
  }
  await cp(staged.root, join(stage, "assets"), { recursive: true });
  const manifest = join(directory, "AndroidManifest.xml");
  await writeFile(manifest, MANIFEST);
  const unaligned = join(directory, `${name}-unaligned.apk`);
  await run(tools.aapt2, ["link", "-o", unaligned, "-I", tools.platform,
    "--manifest", manifest, "--min-sdk-version", String(MIN_SDK),
    "--target-sdk-version", String(TARGET_SDK)]);
  await run("zip", [`-${compress}`, "-q", "-X", "-r", unaligned, "lib", "assets"], { cwd: stage });
  const aligned = join(directory, `${name}.apk`);
  await run(join(tools.buildTools, "zipalign"), ["-f", "-p", "4", unaligned, aligned]);
  await run(join(tools.buildTools, "apksigner"), ["sign",
    "--ks", tools.keystore, "--ks-pass", "pass:android",
    "--ks-key-alias", "androiddebugkey", "--key-pass", "pass:android",
    "--min-sdk-version", String(MIN_SDK), aligned]);
  return {
    name,
    abis: built.map(one => one.abi),
    compress,
    // `bytes` repeats to the byte across runs; `gzip` moves by a few, because
    // apksigner writes a signing time into the block it appends and the
    // compressor sees different input. It is a transfer proxy either way — the
    // figure with authority over a download is bundletool's, below.
    bytes: await bytes(aligned),
    gzip: await gzipped(aligned),
  };
}

/**
 * An AAB, and the download size Play would report for each ABI.
 *
 * This is the one number in the file that is not this machine's arithmetic:
 * `bundletool get-size total` is Google's own calculation of the compressed
 * download, which is what every Play size limit is measured against, so it is
 * asked rather than approximated. It is measured here — under #150, a budget
 * question — and not adopted: nothing ships an AAB (P5c), and what this prices
 * is the option.
 *
 * That it works at all is the finding. #148 concluded an AAB needed a Gradle
 * backend because `cargo apk` emits none; it does not. `aapt2 link
 * --proto-format` writes the protobuf-encoded module a bundle is made of, and
 * `bundletool` assembles it — two of Google's own tools, on a JDK `apksigner`
 * already requires. What is unproven is the other half: no split produced here
 * has been installed on a device or run.
 */
async function bundle(tools, directory, built, staged, jar) {
  const stage = join(directory, "bundle");
  const module = join(stage, "module");
  await mkdir(module, { recursive: true });
  const manifest = join(stage, "AndroidManifest.xml");
  await writeFile(manifest, MANIFEST);
  const proto = join(stage, "proto.apk");
  await run(tools.aapt2, ["link", "--proto-format", "-o", proto, "-I", tools.platform,
    "--manifest", manifest, "--min-sdk-version", String(MIN_SDK),
    "--target-sdk-version", String(TARGET_SDK)]);
  await run("unzip", ["-o", "-q", proto], { cwd: module });
  await mkdir(join(module, "manifest"), { recursive: true });
  await run("mv", ["-f", join(module, "AndroidManifest.xml"), join(module, "manifest")]);
  for (const one of built) {
    await mkdir(join(module, "lib", one.abi), { recursive: true });
    await run(join(tools.llvm, "llvm-strip"),
      ["-o", join(module, "lib", one.abi, "libblitsen_android.so"), one.path]);
  }
  await cp(join(staged.root, "blitsen"), join(module, "assets", "blitsen"), { recursive: true });
  // `uncompressNativeLibraries` is the bundle's spelling of the APK's
  // `extractNativeLibs="false"`, and the ABI split dimension is what makes the
  // per-ABI download exist at all. Both are stated rather than defaulted,
  // because a bundle that silently deflated its library would be measuring a
  // different artifact from the APKs above.
  const config = join(stage, "BundleConfig.json");
  await writeFile(config, `${JSON.stringify({
    optimizations: {
      uncompressNativeLibraries: { enabled: true },
      splitsConfig: { splitDimension: [{ value: "ABI", negate: false }] },
    },
  }, null, 2)}\n`);
  const modules = join(stage, "base.zip");
  await run("zip", ["-q", "-r", "-X", modules, "manifest", "lib", "assets", "resources.pb"],
    { cwd: module });
  const aab = join(stage, "app.aab");
  await run("java", ["-jar", jar, "build-bundle",
    `--modules=${modules}`, `--config=${config}`, `--output=${aab}`, "--overwrite"]);
  const apks = join(stage, "app.apks");
  await run("java", ["-jar", jar, "build-apks",
    `--bundle=${aab}`, `--output=${apks}`, "--overwrite",
    `--ks=${tools.keystore}`, "--ks-pass=pass:android",
    "--ks-key-alias=androiddebugkey", "--key-pass=pass:android"], { maxBuffer: 64 * 1024 * 1024 });
  const { stdout } = await run("java", ["-jar", jar, "get-size", "total",
    `--apks=${apks}`, "--dimensions=ABI"]);
  const download = stdout.split("\n").slice(1)
    .map(line => line.trim().split(","))
    .filter(parts => parts.length >= 2 && ANDROID_ABIS[parts[0]])
    .map(([abi, min]) => ({ abi, download: Number(min) }));
  return { bytes: await bytes(aab), download };
}

/// Every defined, sized symbol in an object file or archive, by name.
async function symbols(tools, path) {
  const { stdout } = await run(join(tools.llvm, "llvm-nm"),
    ["--defined-only", "--print-size", "--no-sort", path],
    { maxBuffer: 256 * 1024 * 1024 });
  const table = new Map();
  for (const line of stdout.split("\n")) {
    // `<address> <size> <type> <name>`; symbols with no size are omitted by the
    // width of the match rather than filtered, because a sizeless symbol
    // contributes nothing to attribute.
    const match = /^[0-9a-f]+ ([0-9a-f]+) \S (.+)$/.exec(line.trim());
    if (match) table.set(match[2], parseInt(match[1], 16));
  }
  return table;
}

/**
 * How many of a linked library's bytes came from a given static archive.
 *
 * The symbol *names* come from the archive, so nothing here guesses from a
 * prefix; the *sizes* come from the linked `.so`, so what is counted is what
 * survived the link rather than what was compiled. What it cannot see is stated
 * with the number: only symbols the linker kept and sized are counted, so
 * unnamed `.rodata`, string literals, `.eh_frame`, `.gcc_except_table` and the
 * relocation tables are all outside it. The figure is a floor.
 */
async function attributeTo(tools, linked, archives) {
  const wanted = new Map();
  for (const archive of archives) {
    for (const name of (await symbols(tools, archive)).keys()) wanted.set(name, archive);
  }
  let total = 0;
  let matched = 0;
  for (const [name, size] of linked) {
    if (!wanted.has(name)) continue;
    total += size;
    matched += 1;
  }
  return {
    bytes: total,
    symbols: matched,
    declared: wanted.size,
    archives: archives.map(path => basename(path)),
  };
}

/// The vendored OpenSSL and the QuickJS-ng archives this target built, found in
/// cargo's output rather than named by a path that would rot on the next bump.
async function archives(triple, profile) {
  const build = join(repository, "target", triple, profile, "build");
  const { stdout } = await run("find", [build, "-name", "*.a", "-type", "f"],
    { maxBuffer: 16 * 1024 * 1024 });
  const found = stdout.split("\n").filter(Boolean);
  const pick = pattern => found.filter(path => pattern.test(path));
  return {
    openssl: pick(/openssl-sys-[^/]+\/out\/.*\/lib(crypto|ssl)\.a$/),
    quickjs: pick(/rquickjs-sys-[^/]+\/out\/libquickjs\.a$/),
  };
}

const directory = await mkdtemp(join(tmpdir(), "blitsen-android-size-"));
try {
  const tools = await toolchain();
  const staged = await assets(directory);
  const built = [];
  for (const abi of abis) built.push(await library(tools, abi));

  // Two axes, both of which move the number a user downloads: which ABIs are in
  // the archive, and whether its library is stored or deflated. Stored is what
  // ships — `extractNativeLibs="false"` requires it — and deflated is measured
  // beside it because it is the only answer this machine can give to "what
  // would Play deliver", an AAB's per-ABI split being a compressed transfer.
  const sets = built.length < 2 ? [] : [{ name: "universal", of: built }];
  for (const one of built) sets.unshift({ name: one.abi, of: [one] });
  const packaged = [];
  for (const set of sets) {
    for (const compress of [0, 9]) {
      packaged.push(await apk(tools, directory, set.of, staged,
        { compress, name: `${set.name}-${compress}` }));
    }
  }
  const stored = packaged.filter(one => one.compress === 0);
  const deflated = packaged.filter(one => one.compress === 9);
  const aab = bundletool ? await bundle(tools, directory, built, staged, bundletool) : null;

  // Attribution needs a symbol table, and `[profile.release]` strips one out.
  // `release-dbg` is that profile with its symbols back and nothing else
  // changed, so the section sizes are checked against the release build's
  // rather than trusted: if the codegen differed, the attribution would be of
  // some other binary.
  let attribution = null;
  if (attribute) {
    const target = built[0];
    const probe = await library(tools, target.abi, "release-dbg");
    const drift = probe.sections[".text"] - target.sections[".text"];
    const found = await archives(target.triple, "release-dbg");
    const linked = await symbols(tools, probe.path);
    let sized = 0;
    for (const size of linked.values()) sized += size;
    attribution = {
      abi: target.abi,
      profile: probe.profile,
      textDrift: drift,
      symbolBytes: sized,
      openssl: await attributeTo(tools, linked, found.openssl),
      quickjs: await attributeTo(tools, linked, found.quickjs),
    };
  }

  const version = async command => await run(...command)
    .then(({ stdout }) => stdout.trim().split("\n")[0]).catch(() => null);
  const measurements = {
    target: "android",
    application: "bare",
    profile: "release",
    recordedAt: new Date().toISOString(),
    commit: await version(["git", ["rev-parse", "--short", "HEAD"], { cwd: repository }]),
    rustc: await version(["rustc", ["--version"]]),
    ndk: basename(tools.ndk),
    buildTools: tools.buildToolsVersion,
    minSdk: MIN_SDK,
    targetSdk: TARGET_SDK,
    abis,
    libraries: built.map(({ abi, triple, bytes: linked, stripped, sections }) =>
      ({ abi, triple, bytes: linked, stripped, sections })),
    application_bytes: staged.bytes,
    apks: packaged,
    aab,
    attribution,
  };

  console.log(`Android bare app, release profile, minSdk ${MIN_SDK} / targetSdk ${TARGET_SDK}`);
  console.log("  shared object, per ABI");
  for (const one of built) {
    console.log(`    ${one.abi.padEnd(12)} ${pad(one.bytes, 12)} B  ${mb(one.bytes)}`
      + `   (llvm-strip takes a further ${one.bytes - one.stripped} B)`);
  }
  const label = one => (one.abis.length > 1 ? "universal" : one.abis[0]).padEnd(12);
  console.log("  signed APK, library and assets stored — what ships");
  for (const one of stored) {
    console.log(`    ${label(one)} ${pad(one.bytes, 12)} B  ${mb(one.bytes)}`
      + `   (gzip -9 ${mb(one.gzip)})`);
  }
  console.log("  the same APKs at zip -9, which forfeits extractNativeLibs=false");
  for (const one of deflated) {
    console.log(`    ${label(one)} ${pad(one.bytes, 12)} B  ${mb(one.bytes)}`);
  }
  console.log(`  the application itself ${pad(staged.bytes, 10)} B  `
    + `${staged.files.length} file(s) plus the index`);
  if (aab) {
    console.log(`  AAB ${pad(aab.bytes, 25)} B  ${mb(aab.bytes)}   `
      + "not what ships; what Play would take");
    for (const { abi, download } of aab.download) {
      console.log(`    delivers to ${abi.padEnd(12)} ${pad(download, 10)} B  ${mb(download)}  `
        + "bundletool get-size total, Play's own compressed-download figure");
    }
  }
  if (attribution) {
    const { openssl, quickjs, symbolBytes, textDrift } = attribution;
    console.log(`  attribution on ${attribution.abi}, from ${attribution.profile} `
      + `(.text differs from release by ${textDrift} B)`);
    console.log(`    all sized symbols   ${pad(symbolBytes, 12)} B  `
      + "what the method can see at all");
    console.log(`    vendored OpenSSL    ${pad(openssl.bytes, 12)} B  ${mb(openssl.bytes)}  `
      + `${openssl.symbols} of ${openssl.declared} symbols from ${openssl.archives.join(" + ")}`);
    console.log(`    QuickJS-ng          ${pad(quickjs.bytes, 12)} B  ${mb(quickjs.bytes)}  `
      + `${quickjs.symbols} of ${quickjs.declared} symbols from ${quickjs.archives.join(" + ")}`);
    console.log("    a floor, not a share: unnamed .rodata, .eh_frame, .gcc_except_table and the");
    console.log("    relocation tables carry bytes no symbol is named for.");
  }

  if (outFile) {
    await writeFile(outFile, `${JSON.stringify(measurements, null, 2)}\n`);
    console.log(`  written to ${outFile}`);
  }
} finally {
  await rm(directory, { recursive: true, force: true });
}
