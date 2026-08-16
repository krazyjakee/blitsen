// `blitsen build --android` (issue #148).
//
// Two things here are worth knowing before reading the assertions.
//
// **Nothing in this file builds an APK.** The packager is a subprocess and the
// NDK is a prerequisite, so every test that would need one injects the runner
// and asserts the *plan* — the argv, the environment, the generated project and
// the staged tree. That is deliberate rather than a shortcut: an argv is the
// whole of what this package decides, and a test that shelled out to cargo-apk
// would be measuring the NDK's presence on whichever machine ran it. What was
// actually built, when, and against what is recorded in the issue rather than
// claimed by a green test here.
//
// **The constants are checked against the Rust that reads them.** `apk.rs` is
// the reader for the index this writes, and there is no build step that could
// derive one side from the other, so the first suite parses the Rust and fails
// if the two have drifted. Three string literals and a schema in two languages
// is exactly the shape of thing that silently disagrees.
import { describe, expect, test } from "bun:test";
import { mkdir, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import {
  ANDROID_ABIS, ANDROID_NOTICES_FILE, androidNotices, androidProject, apkPlan, applicationId,
  buildAndroid, DEFAULT_ABIS, detectAndroidToolchain, resolveAbis, resolveEntryCrate,
  versionCode, workspacePatches,
} from "../src/android.mjs";
import {
  ASSET_INDEX, ASSET_ROOT, INDEX_VERSION, assetIndex, stageAndroidAssets,
} from "../src/android-assets.mjs";
import { MIN_SDK } from "../src/android-toolchain.mjs";
import { main, parseArgs } from "../src/cli.mjs";
import { changed, decodeFrame, describe as describeFrame } from "./run-android-smoke.mjs";
import { capture } from "./cli-support.mjs";

const apkSource = join(import.meta.dir, "../../../crates/blitsen-host/src/apk.rs");

const withWork = async run => {
  const directory = await mkdtemp(join(tmpdir(), "blitsen-android-"));
  try {
    return await run(directory);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
};

/** A small application on disk, with one reference for the rewriter to follow. */
async function application(directory) {
  const root = join(directory, "dist");
  await mkdir(join(root, "assets"), { recursive: true });
  await writeFile(join(root, "index.html"),
    "<html><link rel=stylesheet href=\"/assets/app.css\"><script type=module src=\"/app.js\">"
    + "</script></html>");
  await writeFile(join(root, "app.js"), "export const ready = true;\n");
  await writeFile(join(root, "assets/app.css"), "body { color: red }\n");
  await writeFile(join(root, "orphan.txt"), "not reachable\n");
  return root;
}

describe("the index format agrees with the host that reads it", () => {
  test("the three constants are the same on both sides", async () => {
    const rust = await readFile(apkSource, "utf8");
    const constant = name =>
      new RegExp(`pub const ${name}: &str = "([^"]+)"`).exec(rust)?.[1];
    expect(constant("DEFAULT_ASSET_ROOT")).toBe(ASSET_ROOT);
    expect(constant("ASSET_INDEX")).toBe(ASSET_INDEX);
    expect(Number(/pub const INDEX_VERSION: u32 = (\d+)/.exec(rust)?.[1])).toBe(INDEX_VERSION);
  });

  test("the index carries the fields the reader deserialises", async () => {
    const rust = await readFile(apkSource, "utf8");
    // `IndexEntry` and `Index` are private, so the schema is read off the struct
    // bodies. Anything the reader gains has to appear here before a writer can
    // be said to produce it.
    const entry = /struct IndexEntry \{([\s\S]*?)\n\}/.exec(rust)[1];
    const index = /struct Index \{([\s\S]*?)\n\}/.exec(rust)[1];
    const fields = body => [...body.matchAll(/^\s{4}(\w+):/gm)].map(match => match[1]);
    expect(fields(entry).sort()).toEqual(["bytes", "path"]);
    expect(fields(index).sort()).toEqual(["files", "version"]);
    const written = JSON.parse(assetIndex([{ path: "index.html", bytes: 12 }]));
    expect(Object.keys(written).sort()).toEqual(["files", "version"]);
    expect(Object.keys(written.files[0]).sort()).toEqual(["bytes", "path"]);
  });

  test("the index is sorted, deterministic, and does not list itself", () => {
    const files = [
      { path: "index.html", bytes: 10 },
      { path: ASSET_INDEX, bytes: 3 },
      { path: "app.js", bytes: 20 },
    ];
    const written = assetIndex(files);
    expect(written).toBe(assetIndex([...files].reverse()));
    expect(JSON.parse(written).files.map(file => file.path)).toEqual(["app.js", "index.html"]);
    expect(written.endsWith("\n")).toBe(true);
  });
});

describe("staging an application into an APK's assets/", () => {
  test("files land under assets/blitsen, rewritten, with an index beside them", async () => {
    await withWork(async directory => {
      const root = await application(directory);
      const assets = join(directory, "assets");
      const staged = await stageAndroidAssets({ root, directory: assets });
      const at = path => join(assets, ASSET_ROOT, ...path.split("/"));
      expect(await readFile(at("index.html"), "utf8")).toContain("./assets/app.css");
      expect(await readFile(at("app.js"), "utf8")).toContain("ready");
      expect((await stat(at("assets/app.css"))).size).toBeGreaterThan(0);
      // Unreachable from index.html, so it is dropped exactly as the desktop
      // export drops it — one ingest plan, one answer.
      expect(await stat(at("orphan.txt")).catch(() => null)).toBe(null);
      expect(staged.unreferenced).toEqual(["orphan.txt"]);
      const index = JSON.parse(await readFile(at(ASSET_INDEX), "utf8"));
      expect(index.version).toBe(INDEX_VERSION);
      expect(index.files.map(file => file.path))
        .toEqual(["app.js", "assets/app.css", "index.html"]);
      // The recorded length is the staged file's, not the source's: index.html
      // is rewritten on the way in and the two differ.
      const html = index.files.find(file => file.path === "index.html");
      expect(html.bytes).toBe((await stat(at("index.html"))).size);
    });
  });

  test("--include keeps a file the walk cannot reach, and indexes it", async () => {
    await withWork(async directory => {
      const root = await application(directory);
      const assets = join(directory, "assets");
      const staged = await stageAndroidAssets({ root, directory: assets, include: ["orphan.txt"] });
      expect(staged.files.map(file => file.path)).toContain("orphan.txt");
      expect(await readFile(join(assets, ASSET_ROOT, "orphan.txt"), "utf8"))
        .toBe("not reachable\n");
    });
  });

  test("what the build adds is staged and listed like anything else", async () => {
    await withWork(async directory => {
      const root = await application(directory);
      const assets = join(directory, "assets");
      const extra = new Map([["blitsen.notices.txt.gz", Buffer.from([1, 2, 3, 4])]]);
      const staged = await stageAndroidAssets({ root, directory: assets, extra });
      const listed = JSON.parse(await readFile(join(assets, ASSET_ROOT, ASSET_INDEX), "utf8"));
      expect(listed.files.find(file => file.path === "blitsen.notices.txt.gz"))
        .toEqual({ path: "blitsen.notices.txt.gz", bytes: 4 });
      expect(staged.files.length).toBe(4);
    });
  });
});

describe("the third-party notices an APK owes", () => {
  test("are staged uncompressed, under the name aapt leaves alone", async () => {
    await withWork(async directory => {
      const source = join(directory, "NOTICES.txt");
      await writeFile(source, "THIRD-PARTY NOTICES\n");
      const notices = await androidNotices({ BLITSEN_NOTICES_PATH: source });
      // Measured, not chosen: `aapt` strips `.gz` from an asset name and
      // inflates the contents, so a `.gz` here would arrive under a name
      // nothing looks for and every APK would report itself uncleared.
      expect(notices.file).toBe(ANDROID_NOTICES_FILE);
      expect(notices.file.endsWith(".gz")).toBe(false);
      expect(notices.contents.toString()).toBe("THIRD-PARTY NOTICES\n");
    });
  });

  test("the host reads exactly the name this writes", async () => {
    const rust = await readFile(join(import.meta.dir, "../../../crates/blitsen-host/src/app.rs"),
      "utf8");
    expect(/pub const NOTICES_UNCOMPRESSED: &str = "([^"]+)"/.exec(rust)?.[1])
      .toBe(ANDROID_NOTICES_FILE);
  });

  test("absent, there is nothing to stage and the caller has to say so", async () => {
    expect(await androidNotices({})).toBe(null);
    expect(await androidNotices({ BLITSEN_NOTICES_PATH: "/nowhere/NOTICES.txt" })).toBe(null);
  });
});

describe("the ABI set", () => {
  test("defaults to the shipping one and the emulator one", () => {
    expect(resolveAbis(undefined)).toEqual(["arm64-v8a", "x86_64"]);
    expect(DEFAULT_ABIS).toEqual(["arm64-v8a", "x86_64"]);
    expect(resolveAbis([])).toEqual(["arm64-v8a", "x86_64"]);
  });

  test("takes what was asked for, in order, once each", () => {
    expect(resolveAbis(["x86_64", "arm64-v8a", "x86_64"])).toEqual(["x86_64", "arm64-v8a"]);
    expect(resolveAbis(["armeabi-v7a"])).toEqual(["armeabi-v7a"]);
  });

  test("refuses an ABI with no Rust target behind it", () => {
    expect(() => resolveAbis(["x86"])).toThrow("unknown --android-abi x86");
    expect(Object.keys(ANDROID_ABIS)).not.toContain("x86");
  });
});

describe("the application ID", () => {
  test("is generated from the application name when none is given", () => {
    expect(applicationId(null, "Pong")).toBe("com.blitsen.pong");
    expect(applicationId(null, "My Great App!")).toBe("com.blitsen.mygreatapp");
    expect(applicationId(null, "2048")).toBe("com.blitsen.app");
  });

  test("is validated rather than sanitised when it is given", () => {
    expect(applicationId("com.example.pong", "Pong")).toBe("com.example.pong");
    expect(applicationId("com.example.pong_2", "Pong")).toBe("com.example.pong_2");
    expect(() => applicationId("pong", "Pong")).toThrow("at least two dot-separated segments");
    expect(() => applicationId("com.example.my-app", "Pong")).toThrow("\"my-app\"");
    expect(() => applicationId("com.example.9lives", "Pong")).toThrow("start with a letter");
    expect(() => applicationId("com.new.pong", "Pong")).toThrow("is a Java keyword");
  });
});

describe("the version code", () => {
  // This is the packager's scheme, not one Blitsen chose — `cargo apk` panics
  // if the manifest carries a version code at all. The assertions are here so
  // that what `blitsen build` prints is what the APK carries; the argument for
  // why Blitsen does not get to pick is in android.mjs.
  test("is the packed form the packager will write", () => {
    expect(versionCode("1.2.3")).toBe((1 << 24) | (1 << 16) | (2 << 8) | 3);
    expect(versionCode("1.2.3")).toBe(16843267);
    expect(versionCode("0.1.0")).toBe(16777472);
  });

  test("orders the way semver does, inside the range it can express", () => {
    expect(versionCode("1.0.0")).toBeGreaterThan(versionCode("0.255.255"));
    expect(versionCode("0.2.0")).toBeGreaterThan(versionCode("0.1.255"));
  });

  test("drops what Android has nowhere to put", () => {
    expect(versionCode("1.2.3-beta.1+build9")).toBe(versionCode("1.2.3"));
  });

  test("refuses a version the packager cannot pack, before the cross-compile", () => {
    expect(() => versionCode("1.0.256")).toThrow("patch must be below 256");
    expect(() => versionCode("300.400.0")).toThrow("major and minor must be below 256");
    expect(() => versionCode("1.2.3.4")).toThrow("major.minor.patch");
    expect(() => versionCode("v1.2.3")).toThrow("major.minor.patch");
    expect(() => versionCode("1.2")).toThrow("major.minor.patch");
    // And it is refused where the project is generated, not only where the code
    // is reported — that is the call the packager would have panicked on.
    expect(() => androidProject({ name: "P", applicationId: "com.a.b", version: "1.0.256" }))
      .toThrow("below 256");
  });
});

describe("the generated Cargo project", () => {
  const project = () => androidProject({
    name: "Pong Deluxe",
    applicationId: "com.example.pong",
    version: "1.2.3",
    abis: ["arm64-v8a", "x86_64"],
    entryCrate: "/checkout/crates/blitsen-android",
  });

  test("is a cdylib whose one statement is the entry point", () => {
    const generated = project();
    expect(generated.cargoToml).toContain('crate-type = ["cdylib"]');
    expect(generated.libRs.trim().split("\n").at(-1)).toBe("blitsen_android::android_main!();");
    expect(generated.cargoToml)
      .toContain('blitsen-android = { path = "/checkout/crates/blitsen-android" }');
  });

  test("carries the manifest fields Android is keyed by", () => {
    const generated = project();
    expect(generated.cargoToml).toContain('package = "com.example.pong"');
    expect(generated.cargoToml).toContain('label = "Pong Deluxe"');
    expect(generated.cargoToml)
      .toContain('build_targets = ["aarch64-linux-android", "x86_64-linux-android"]');
    // The version reaches the manifest through the crate's own version and
    // nowhere else: cargo-apk 0.10 panics if the metadata names either the
    // version code or the version name, which was measured rather than read.
    expect(generated.cargoToml).toContain('version = "1.2.3"');
    expect(generated.cargoToml).not.toContain("version_code");
    expect(generated.cargoToml).not.toContain("version_name");
  });

  test("names the artifact something a filesystem accepts", () => {
    expect(project().apkName).toBe("Pong-Deluxe");
    expect(project().library).toBe("com_example_pong");
    expect(androidProject({ name: "!!", applicationId: "com.a.b" }).apkName).toBe("app");
  });

  test("builds only what was asked for", () => {
    const one = androidProject({ name: "P", applicationId: "com.a.b", abis: ["armeabi-v7a"] });
    expect(one.cargoToml).toContain('build_targets = ["armv7-linux-androideabi"]');
    expect(one.cargoToml).not.toContain("aarch64");
  });
});

describe("the workspace's dependency pins travel with the generated project", () => {
  test("the [patch] tables are copied verbatim", async () => {
    const patches = await workspacePatches(join(import.meta.dir, "../../../crates/blitsen-host"));
    expect(patches).toContain("[patch.crates-io]");
    expect(patches).toContain('[patch."https://github.com/DioxusLabs/blitz"]');
    // Copied as text, so the reasoning above each pin survives the move.
    expect(patches).toContain("# A fork of Blitz at the pinned revision");
    expect(patches).not.toContain("[profile.release]");
  });

  test("a patch that names a local path is refused rather than relocated", async () => {
    await withWork(async directory => {
      await writeFile(join(directory, "Cargo.toml"),
        "[workspace]\nmembers = []\n\n[patch.crates-io]\nblitz = { path = \"../blitz\" }\n");
      await expect(workspacePatches(directory)).rejects.toThrow("local path");
    });
  });
});

/** A minimal SDK tree: enough for the detector, nothing that could build. */
async function fakeSdk(directory, { ndk = "27.2.12479018", tools = ["aapt", "zipalign", "apksigner"],
  buildTools = "34.0.0" } = {}) {
  const sdk = join(directory, "Sdk");
  if (ndk) {
    // The C toolchain the detector reads back out. An NDK ships exactly one
    // `toolchains/llvm/prebuilt/<host>`, which is what makes finding it by
    // reading the directory correct rather than a table of host names.
    await mkdir(join(sdk, "ndk", ndk, "toolchains", "llvm", "prebuilt", "linux-x86_64", "bin"),
      { recursive: true });
  }
  await mkdir(join(sdk, "build-tools", buildTools), { recursive: true });
  for (const tool of tools) await writeFile(join(sdk, "build-tools", buildTools, tool), "");
  await mkdir(join(sdk, "platforms", "android-33"), { recursive: true });
  await writeFile(join(sdk, "platforms", "android-33", "android.jar"), "");
  return sdk;
}

const detected = (sdk, overrides = {}) => detectAndroidToolchain({
  env: { ANDROID_HOME: sdk, ...overrides },
  which: () => "/somewhere/cargo-apk",
});

describe("the toolchain is detected, never installed", () => {
  test("finds the SDK, the newest NDK and the newest build-tools", async () => {
    await withWork(async directory => {
      const sdk = await fakeSdk(directory);
      await mkdir(join(sdk, "ndk", "9.0.0"), { recursive: true });
      await mkdir(join(sdk, "build-tools", "9.0.0"), { recursive: true });
      const toolchain = await detected(sdk);
      expect(toolchain.sdk).toBe(sdk);
      // Numeric, not lexicographic: "9.0.0" sorts after "27..." as a string.
      expect(toolchain.ndk).toBe(join(sdk, "ndk", "27.2.12479018"));
      expect(toolchain.buildToolsVersion).toBe("34.0.0");
    });
  });

  test("names what is missing and the command that installs it", async () => {
    await withWork(async directory => {
      await expect(detected(join(directory, "nowhere"))).rejects.toThrow("nothing is there");
      const noNdk = await fakeSdk(join(directory, "a"), { ndk: null });
      await expect(detected(noNdk)).rejects.toThrow("sdkmanager \"ndk;");
      // And it says why it will not fetch one itself.
      await expect(detected(noNdk)).rejects.toThrow("Blitsen does not download it");
      const noAapt = await fakeSdk(join(directory, "b"), { tools: ["zipalign", "apksigner"] });
      await expect(detected(noAapt)).rejects.toThrow("has no aapt");
      const sdk = await fakeSdk(join(directory, "c"));
      await expect(detectAndroidToolchain({ env: { ANDROID_HOME: sdk }, which: () => null }))
        .rejects.toThrow("cargo-apk is not on PATH");
    });
  });

  test("ANDROID_NDK_HOME outranks the SDK's own", async () => {
    await withWork(async directory => {
      const sdk = await fakeSdk(directory);
      const elsewhere = join(directory, "ndk-r99");
      await mkdir(join(elsewhere, "toolchains", "llvm", "prebuilt", "darwin-x86_64"),
        { recursive: true });
      const toolchain = await detected(sdk, { ANDROID_NDK_HOME: elsewhere });
      expect(toolchain.ndk).toBe(elsewhere);
      // And the C toolchain follows it, rather than staying with the SDK's.
      expect(toolchain.llvm)
        .toBe(join(elsewhere, "toolchains", "llvm", "prebuilt", "darwin-x86_64"));
      expect(toolchain.sysroot).toBe(join(toolchain.llvm, "sysroot"));
    });
  });

  test("an NDK with no C toolchain in it is refused, not used", async () => {
    await withWork(async directory => {
      const sdk = await fakeSdk(directory);
      const hollow = join(directory, "not-an-ndk");
      await mkdir(hollow, { recursive: true });
      await expect(detected(sdk, { ANDROID_NDK_HOME: hollow }))
        .rejects.toThrow("no C toolchain");
    });
  });
});

describe("the packager invocation", () => {
  const plan = (overrides = {}) => apkPlan({
    project: { apkName: "Pong", abis: ["arm64-v8a", "x86_64"] },
    directory: "/build/.Pong.apk.blitsen-android",
    toolchain: {
      sdk: "/sdk", ndk: "/sdk/ndk/27",
      llvm: "/sdk/ndk/27/toolchains/llvm/prebuilt/linux-x86_64",
      sysroot: "/sdk/ndk/27/toolchains/llvm/prebuilt/linux-x86_64/sysroot",
    },
    ...overrides,
  });

  test("is one command, with the SDK and NDK in the environment", () => {
    const release = plan();
    expect(release.command).toEqual(["cargo-apk", "apk", "build", "--release", "--lib"]);
    expect(release.environment.ANDROID_HOME).toBe("/sdk");
    expect(release.environment.ANDROID_NDK_HOME).toBe("/sdk/ndk/27");
    expect(release.artifact)
      .toBe(join("/build/.Pong.apk.blitsen-android", "target", "release", "apk", "Pong.apk"));
  });

  // Both of these are absences in `cargo apk` rather than preferences, and both
  // were found by building an APK against Blitsen's own graph for the first
  // time (#149). Asserted per ABI, because the variables are per target triple
  // and an ABI that is named but unconfigured fails an hour into the build.
  test("tells the cross-compile the two things cargo-apk does not", () => {
    const environment = plan().environment;
    const llvm = "/sdk/ndk/27/toolchains/llvm/prebuilt/linux-x86_64";
    for (const triple of ["aarch64_linux_android", "x86_64_linux_android"]) {
      // openssl-sys builds OpenSSL vendored on Android, and its makefile runs a
      // `ranlib` the NDK stopped shipping under that name in r23.
      expect(environment[`RANLIB_${triple}`]).toBe(`${llvm}/bin/llvm-ranlib`);
      // rquickjs-sys generates its own Android bindings and hands bindgen no
      // sysroot, so libclang reads Android's headers as the host's.
      expect(environment[`BINDGEN_EXTRA_CLANG_ARGS_${triple}`])
        .toContain(`--sysroot=${llvm}/sysroot`);
    }
    expect(environment.BINDGEN_EXTRA_CLANG_ARGS_aarch64_linux_android)
      .toContain(`-I${llvm}/sysroot/usr/include/aarch64-linux-android`);
    // Only the ABIs asked for: naming one configures one.
    expect(environment.RANLIB_armv7_linux_androideabi).toBeUndefined();
  });

  test("names the sysroot headers for the ABI, which 32-bit ARM spells differently", () => {
    const environment = plan({
      project: { apkName: "Pong", abis: ["armeabi-v7a"] },
    }).environment;
    // The Rust triple is `armv7-linux-androideabi`; the include directory is
    // `arm-linux-androideabi`, and using the first finds nothing.
    expect(environment.BINDGEN_EXTRA_CLANG_ARGS_armv7_linux_androideabi)
      .toContain("/sysroot/usr/include/arm-linux-androideabi");
    expect(environment.BINDGEN_EXTRA_CLANG_ARGS_armv7_linux_androideabi)
      .not.toContain("include/armv7-linux-androideabi");
  });

  test("signs a release with the debug key until a real one is named", () => {
    const release = plan();
    expect(release.debugSigned).toBe(true);
    expect(release.environment.CARGO_APK_RELEASE_KEYSTORE).toBe(release.keystore);
    expect(release.environment.CARGO_APK_RELEASE_KEYSTORE_PASSWORD).toBe("android");
    const signed = plan({ keystore: "/keys/release.jks", keystorePassword: "hunter2" });
    expect(signed.debugSigned).toBe(false);
    expect(signed.environment.CARGO_APK_RELEASE_KEYSTORE).toBe("/keys/release.jks");
    expect(signed.environment.CARGO_APK_RELEASE_KEYSTORE_PASSWORD).toBe("hunter2");
  });

  test("refuses a keystore whose password was not put in the environment", () => {
    expect(() => plan({ keystore: "/keys/release.jks" }))
      .toThrow("BLITSEN_ANDROID_KEYSTORE_PASSWORD");
  });

  test("a debug build is the debug profile and carries no release key", () => {
    const debug = plan({ release: false });
    expect(debug.command).toEqual(["cargo-apk", "apk", "build", "--lib"]);
    expect(debug.artifact).toContain(join("target", "debug", "apk"));
    expect(debug.environment.CARGO_APK_RELEASE_KEYSTORE).toBeUndefined();
  });
});

describe("the entry point crate", () => {
  test("is taken from the environment when it is named", async () => {
    await withWork(async directory => {
      await writeFile(join(directory, "Cargo.toml"), "[package]\nname = \"blitsen-android\"\n");
      expect(await resolveEntryCrate({ BLITSEN_ANDROID_CRATE: directory })).toBe(directory);
      await expect(resolveEntryCrate({ BLITSEN_ANDROID_CRATE: join(directory, "gone") }))
        .rejects.toThrow("has no Cargo.toml");
    });
  });

  test("says it is issue #142's rather than failing inside cargo", async () => {
    // Written against the tree as it stands: crates/blitsen-android does not
    // exist yet, and this is the message a user gets today. It becomes a test
    // that the checkout path resolves the moment #142 lands, which is the point
    // — either way the assertion is about something real.
    const inTree = join(import.meta.dir, "../../../crates/blitsen-android/Cargo.toml");
    const present = await Bun.file(inTree).exists();
    if (present) expect(await resolveEntryCrate({})).toContain("blitsen-android");
    else await expect(resolveEntryCrate({})).rejects.toThrow("issue #142's");
  });
});

describe("an Android build, with the packager stubbed", () => {
  test("stages, generates and invokes, then reports the APK", async () => {
    await withWork(async directory => {
      const root = await application(directory);
      const crate = join(directory, "blitsen-android");
      await mkdir(crate, { recursive: true });
      await writeFile(join(crate, "Cargo.toml"), "[package]\nname = \"blitsen-android\"\n");
      const commands = [];
      const run = async (command, options = {}) => {
        commands.push(command);
        if (command[0] === "rustup") {
          return { code: 0, stdout: "aarch64-linux-android\nx86_64-linux-android\n", stderr: "" };
        }
        // Stand in for cargo-apk: leave an artifact exactly where the plan says.
        const artifact = join(options.cwd, "target", "release", "apk", "Pong.apk");
        await mkdir(dirname(artifact), { recursive: true });
        await writeFile(artifact, "PKstub");
        return { code: 0, stdout: "", stderr: "" };
      };
      const steps = [];
      const result = await buildAndroid({
        root,
        name: "Pong",
        outfile: join(directory, "Pong.apk"),
        appVersion: "1.2.3",
        env: { BLITSEN_ANDROID_CRATE: crate },
        run,
        detect: async () => ({ sdk: "/sdk", ndk: "/sdk/ndk/27", buildTools: "/sdk/bt",
          llvm: "/sdk/ndk/27/llvm", sysroot: "/sdk/ndk/27/llvm/sysroot",
          buildToolsVersion: "34.0.0", platform: "/sdk/p", packager: "cargo-apk" }),
        progress: event => steps.push(event),
      });
      expect(commands.at(-1)).toEqual(["cargo-apk", "apk", "build", "--release", "--lib"]);
      expect(result.applicationId).toBe("com.blitsen.pong");
      expect(result.versionCode).toBe(versionCode("1.2.3"));
      expect(result.abis).toEqual(["arm64-v8a", "x86_64"]);
      expect(result.assets).toBe(3);
      expect(result.debugSigned).toBe(true);
      expect((await stat(result.outfile)).size).toBeGreaterThan(0);
      const staging = join(directory, ".Pong.apk.blitsen-android");
      expect(await readFile(join(staging, "Cargo.toml"), "utf8"))
        .toContain('package = "com.blitsen.pong"');
      expect(await readFile(join(staging, "assets", ASSET_ROOT, "index.html"), "utf8"))
        .toContain("./assets/app.css");
      // The three notes a reader has to see: what was signed, that it is not an
      // AAB, and that release assets are deflated (#144's noCompress).
      const notes = steps.flatMap(step => step.notes ?? []).join("\n");
      expect(notes).toContain("debug key");
      expect(notes).toContain("App Bundle");
      expect(notes).toContain("noCompress");
    });
  });

  test("refuses to cross-compile for a Rust target that is not installed", async () => {
    await withWork(async directory => {
      const root = await application(directory);
      const crate = join(directory, "blitsen-android");
      await mkdir(crate, { recursive: true });
      await writeFile(join(crate, "Cargo.toml"), "[package]\nname = \"blitsen-android\"\n");
      await expect(buildAndroid({
        root,
        name: "Pong",
        outfile: join(directory, "Pong.apk"),
        env: { BLITSEN_ANDROID_CRATE: crate },
        run: async command => (command[0] === "rustup"
          ? { code: 0, stdout: "x86_64-unknown-linux-gnu\n", stderr: "" }
          : { code: 0, stdout: "", stderr: "" }),
        detect: async () => ({ sdk: "/sdk", ndk: "/sdk/ndk/27", llvm: "/sdk/ndk/27/llvm",
          sysroot: "/sdk/ndk/27/llvm/sysroot", buildToolsVersion: "34.0.0" }),
      })).rejects.toThrow("rustup target add aarch64-linux-android x86_64-linux-android");
    });
  });
});

describe("the command line", () => {
  test("--android is a flag on build, not a --target value", () => {
    expect(parseArgs(["build", "dist", "--android"]))
      .toEqual({ command: "build", directory: "dist", width: 800, height: 600,
        title: "Blitsen", android: true });
    expect(parseArgs(["build", "dist", "--android", "--android-abi", "x86_64",
      "--android-abi", "armeabi-v7a"]).androidAbis).toEqual(["x86_64", "armeabi-v7a"]);
    expect(() => parseArgs(["dist", "--android"])).toThrow("only valid with build");
    expect(() => parseArgs(["build", "dist", "--android", "--android-abi", "mips"]))
      .toThrow("unknown --android-abi mips");
  });

  test("an Android option without --android is refused rather than ignored", () => {
    expect(() => parseArgs(["build", "dist", "--android-abi", "x86_64"]))
      .toThrow("--android-abi needs --android");
    expect(() => parseArgs(["build", "dist", "--android-debug"]))
      .toThrow("--android-debug needs --android");
    expect(() => parseArgs(["build", "dist", "--android-keystore", "k.jks"]))
      .toThrow("--android-keystore needs --android");
  });

  test("--target and --android are different artifacts", () => {
    expect(() => parseArgs(["build", "dist", "--android", "--target", "linux-x64"]))
      .toThrow("different artifacts");
    // doctor still grades for Android, and build still refuses to (#147).
    expect(parseArgs(["doctor", "dist", "--target", "android-arm64"]).target)
      .toBe("android-arm64");
    expect(() => parseArgs(["build", "dist", "--target", "android-arm64"]))
      .toThrow("unknown --target android-arm64");
  });

  test("desktop options that describe something an APK has not are refused", () => {
    expect(() => parseArgs(["build", "dist", "--android", "--assets", "side-loaded"]))
      .toThrow("nothing beside it to side-load");
    expect(() => parseArgs(["build", "dist", "--android", "--addon", "p.node"]))
      .toThrow("--addon is not valid with --android");
    expect(() => parseArgs(["build", "dist", "--android", "--icon", "app.png"]))
      .toThrow("--icon is not valid with --android");
  });

  test("--bundle-id supplies the application ID when --android-package does not", () => {
    expect(parseArgs(["build", "dist", "--android", "--bundle-id", "com.example.pong"])
      .androidPackage).toBe("com.example.pong");
    expect(parseArgs(["build", "dist", "--android", "--bundle-id", "com.example.pong",
      "--android-package", "com.example.pong.android"]).androidPackage)
      .toBe("com.example.pong.android");
  });

  test("help lists the Android flags under their own heading", async () => {
    const { lines } = capture();
    const help = [];
    await main(["--help"], { log: line => help.push(line), error: line => help.push(line) });
    expect(lines).toEqual([]);
    expect(help.join("\n")).toContain("--android-abi <abi>");
    expect(help.join("\n")).toContain("BLITSEN_ANDROID_KEYSTORE_PASSWORD");
  });

  test("--android grades the application against Android's native: table", async () => {
    // #147 landed the table and taught `doctor` to read it; the point here is
    // that `build --android` reaches the same answer without being told a
    // target, because there is no target to tell it.
    await withWork(async directory => {
      const root = await application(directory);
      await writeFile(join(root, "app.js"),
        "import { writeText } from \"blitsen/clipboard\";\n"
        + "import { platform } from \"blitsen/os\";\n"
        + "export const ready = [writeText, platform];\n");
      const { lines, output } = capture();
      const previous = process.env.ANDROID_HOME;
      process.env.ANDROID_HOME = join(directory, "no-sdk-here");
      try {
        await main(["build", root, "--android", "--out", join(directory, "P.apk")], output);
      } finally {
        if (previous === undefined) delete process.env.ANDROID_HOME;
        else process.env.ANDROID_HOME = previous;
      }
      const said = lines.map(([, line]) => line).join("\n");
      expect(said).toContain("blitsen/clipboard does not exist on android");
      // The negative column, and it is the one that makes the assertion mean
      // something: `os` survives on Android, so silence about it is a result.
      expect(said).not.toContain("blitsen/os does not exist");
    });
  });

  test("a build with no SDK fails saying so, and resolves no desktop runtime", async () => {
    await withWork(async directory => {
      const root = await application(directory);
      const { lines, output } = capture();
      const previous = process.env.ANDROID_HOME;
      process.env.ANDROID_HOME = join(directory, "no-sdk-here");
      try {
        expect(await main(["build", root, "--android", "--out", join(directory, "P.apk")],
          output)).toBe(1);
      } finally {
        if (previous === undefined) delete process.env.ANDROID_HOME;
        else process.env.ANDROID_HOME = previous;
      }
      const said = lines.map(([, line]) => line).join("\n");
      expect(said).toContain("nothing is there");
      // The desktop path would have failed here first, on a missing addon; this
      // one never asks for one.
      expect(said).not.toContain("native build runtime");
    });
  });
});

// The one piece of the emulator smoke test (#149) that can be measured without
// an emulator, and the one most worth measuring. `screencap`'s raw header grew
// a field in Android 9, so the reader tries both sizes; if it ever picked the
// wrong one the pixels would be shifted by four bytes and every frame would
// decode as noise — which has thousands of distinct colours and would sail
// through the blankness check. A green Android job that measured nothing is
// precisely the outcome #149 exists to avoid, so the decoder is held to frames
// whose contents are known.
describe("the frame the Android smoke test reads back", () => {
  /// A raw `screencap` buffer: header, then width x height little-endian pixels.
  const frame = (width, height, pixel, header = 16) => {
    const bytes = Buffer.alloc(header + width * height * 4);
    bytes.writeUInt32LE(width, 0);
    bytes.writeUInt32LE(height, 4);
    bytes.writeUInt32LE(1, 8);
    for (let at = 0; at < width * height; at += 1) {
      bytes.writeUInt32LE(pixel(at % width, Math.floor(at / width)), header + at * 4);
    }
    return bytes;
  };

  test("decodes both header sizes, and reads the pixels in the right places", () => {
    for (const header of [12, 16]) {
      const decoded = decodeFrame(frame(4, 3, (x, y) => 0x1000 * y + x, header));
      expect(decoded.header).toBe(header);
      expect([decoded.width, decoded.height]).toEqual([4, 3]);
      // The corner pixels, which is where an off-by-four in the header shows.
      expect(decoded.pixels[0]).toBe(0);
      expect(decoded.pixels[3]).toBe(3);
      expect(decoded.pixels[11]).toBe(0x2003);
    }
  });

  test("refuses a buffer whose arithmetic does not work out", () => {
    const truncated = frame(4, 3, () => 0).subarray(0, 40);
    expect(() => decodeFrame(truncated)).toThrow("not a frame this understands");
    expect(() => decodeFrame(Buffer.alloc(0))).toThrow("not a frame");
  });

  test("a flat frame is blank and a varied one is not", () => {
    expect(describeFrame(decodeFrame(frame(64, 64, () => 0xff000000))).blank).toBe(true);
    const varied = describeFrame(decodeFrame(frame(64, 64, (x, y) => (x << 8) | y)));
    expect(varied.blank).toBe(false);
    expect(varied.colours).toBe(64 * 64);
    // A frame that is one colour with a handful of pixels of another is still
    // blank: the launcher's clock must not count as an application painting.
    const nearlyFlat = describeFrame(decodeFrame(frame(64, 64, (x, y) => (x < 1 && y < 4 ? y : 0))));
    expect(nearlyFlat.blank).toBe(true);
  });

  test("change is measured against the control, not against nothing", () => {
    const before = decodeFrame(frame(10, 10, () => 7));
    expect(changed(before, decodeFrame(frame(10, 10, () => 7)))).toBe(0);
    expect(changed(before, decodeFrame(frame(10, 10, () => 9)))).toBe(1);
    // Half the rows repainted.
    expect(changed(before, decodeFrame(frame(10, 10, (x, y) => (y < 5 ? 9 : 7))))).toBe(0.5);
    // A different size is a different screen, and comparing them pixelwise
    // would be meaningless rather than zero.
    expect(changed(before, decodeFrame(frame(10, 12, () => 7)))).toBe(1);
  });
});

// The CI job cross-compiles at an API level spelled in YAML, and `MIN_SDK` is
// spelled in JavaScript, and nothing links the two. That is the shape of thing
// this file already guards for `apk.rs`: two languages holding the same number
// with no build step between them. It matters here because the number is not
// cosmetic — `cpal` links `libaaudio`, which the NDK first ships at 26, so a
// job that built below it would fail at the linker, and a job that built above
// it would be testing an artifact the packager does not produce.
describe("the Android CI job and the packager agree on the API floor", () => {
  test("ci.yml cross-compiles at MIN_SDK", async () => {
    const workflow = await readFile(join(import.meta.dir, "../../../.github/workflows/ci.yml"),
      "utf8");
    const platform = /cargo ndk[^\n]*-P (\d+)/.exec(workflow);
    expect(platform).not.toBe(null);
    expect(Number(platform[1])).toBe(MIN_SDK);
    // And it builds the ABIs an APK would carry, not a subset of them.
    for (const abi of DEFAULT_ABIS) expect(workflow).toContain(`-t ${abi}`);
  });
});
