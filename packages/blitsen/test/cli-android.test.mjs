// `blitsen build --android` (issue #148).
//
// Two things here are worth knowing before reading the assertions.
//
// **Nothing in this file builds an APK.** The cross-compile is a subprocess and
// the NDK is a prerequisite, so every test that would need one injects the
// runner and asserts the *plan* — the argv, the environment, the manifest and
// the staged tree — plus the archive, which is written in process and so can be
// read straight back. That is deliberate rather than a shortcut: a test that
// shelled out to `cargo ndk` would be measuring the NDK's presence on whichever
// machine ran it. What was actually built, when, and against what is recorded
// in the issue rather than claimed by a green test here.
//
// **The constants are checked against the code on the other side of them**, in
// two directions and in two languages. `apk.rs` is the reader for the index
// this writes; `crates/blitsen-android/Cargo.toml` is the crate this build
// links, and the library name it produces is what the generated manifest tells
// `NativeActivity` to `dlopen`. There is no build step that could derive either
// side from the other, so both are parsed here. That second check exists
// because its absence was a real bug: #148 first landed naming an entry macro
// that #142 never wrote, and the whole suite passed while no APK the CLI built
// could ever have contained the engine.
import { describe, expect, test } from "bun:test";
import { mkdir, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  ANDROID_ABIS, ANDROID_NOTICES_FILE, androidManifest, androidNotices, androidProject, apkEntries,
  apkPlan, applicationId, buildAndroid, cargoTargetDirectory, CONFIG_CHANGES, DEFAULT_ABIS,
  detectAndroidToolchain, ENTRY_CRATE, ENTRY_LIBRARY, ENTRY_SO, ensureDebugKeystore, findLibclang,
  MIN_SDK, resolveAbis, resolveEntryCrate, storedZip, TARGET_SDK, versionCode,
} from "../src/android.mjs";
import {
  ASSET_INDEX, ASSET_ROOT, INDEX_VERSION, assetIndex, stageAndroidAssets,
} from "../src/android-assets.mjs";
import { main, parseArgs } from "../src/cli.mjs";
import { capture } from "./cli-support.mjs";

const apkSource = join(import.meta.dir, "../../../crates/blitsen-host/src/apk.rs");
const entrySource = join(import.meta.dir, "../../../crates", ENTRY_CRATE);

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
  // This scheme is Blitsen's again. It was abandoned once because `cargo apk`
  // panics if the manifest carries a version code at all and imposed a byte per
  // component; this build writes the manifest, so the number in it is the one
  // chosen here and `1.0.256` ships.
  test("is the semver, in decimal places that read back", () => {
    expect(versionCode("1.2.3")).toBe(1_002_003);
    expect(versionCode("0.1.0")).toBe(1_000);
    expect(versionCode("12.34.56")).toBe(12_034_056);
  });

  test("orders the way semver does", () => {
    expect(versionCode("1.0.0")).toBeGreaterThan(versionCode("0.999.999"));
    expect(versionCode("0.2.0")).toBeGreaterThan(versionCode("0.1.999"));
    // The ceiling the packager imposed is gone, and this is the version that
    // could not previously be expressed at all.
    expect(versionCode("1.0.256")).toBe(1_000_256);
  });

  test("drops what Android has nowhere to put", () => {
    expect(versionCode("1.2.3-beta.1+build9")).toBe(versionCode("1.2.3"));
  });

  test("refuses a version the places cannot hold, before the cross-compile", () => {
    expect(() => versionCode("1.0.1000")).toThrow("patch must be below 1000");
    expect(() => versionCode("1.1000.1000")).toThrow("minor and patch must be below 1000");
    expect(() => versionCode("1.2.3.4")).toThrow("major.minor.patch");
    expect(() => versionCode("v1.2.3")).toThrow("major.minor.patch");
    expect(() => versionCode("1.2")).toThrow("major.minor.patch");
    // And it is refused where the project is described, not only where the code
    // is reported.
    expect(() => androidProject({ name: "P", applicationId: "com.a.b", version: "1.0.1000" }))
      .toThrow("below 1000");
  });

  test("refuses a code above what Google Play will accept", () => {
    // The field is a signed 32-bit integer, so this is not the format's limit;
    // it is the limit of the one place the number has consequences, and a
    // version code cannot be walked back once published.
    expect(versionCode("2100.0.0")).toBe(2_100_000_000);
    expect(() => versionCode("2101.0.0")).toThrow("2100000000");
    expect(() => versionCode("2100.0.1")).toThrow("Google Play refuses");
  });
});

describe("the entry point this build links, as the crate actually declares it", () => {
  // The check that was missing. #148 landed assuming the entry point was a
  // macro invoked from a generated cdylib; #142 had landed a cdylib exporting
  // `android_main` itself. Nothing here compared the two, so the suite was
  // green while `blitsen build --android` named a macro that did not exist.
  test("is a cdylib named blitsen_android, exporting android_main", async () => {
    const manifest = await readFile(join(entrySource, "Cargo.toml"), "utf8");
    const section = header => new RegExp(`\\[${header}\\]([\\s\\S]*?)(?=\\n\\[|$)`)
      .exec(manifest)?.[1] ?? "";
    expect(/^name = "([^"]+)"/m.exec(section("package"))?.[1]).toBe(ENTRY_CRATE);
    // `[lib] name`, which is what `lib<name>.so` and `android.app.lib_name` are
    // both spelled from — not the package name, which uses a hyphen.
    expect(/^name = "([^"]+)"/m.exec(section("lib"))?.[1]).toBe(ENTRY_LIBRARY);
    expect(/^crate-type = \[([^\]]+)\]/m.exec(section("lib"))?.[1].trim()).toBe('"cdylib"');
    expect(ENTRY_SO).toBe(`lib${ENTRY_LIBRARY}.so`);
    const source = await readFile(join(entrySource, "src", "lib.rs"), "utf8");
    // A `pub fn android_main` behind `#[unsafe(no_mangle)]`, which is what
    // `NativeActivity` resolves out of the loaded library, and the reason there
    // is nothing for this package to generate.
    expect(/#\[unsafe\(no_mangle\)\]\s*\npub fn (\w+)/.exec(source)?.[1]).toBe("android_main");
  });

  test("cannot be linked below the API level its audio backend needs", () => {
    // Not a preference. `cargo ndk -P 24` fails with `unable to find library
    // -laaudio`, because the NDK ships libaaudio from API 26 and no earlier,
    // and the runtime's audio backend reaches it. A manifest claiming 24 while
    // the .so binds AAudio is a dlopen failure on a cold start, so the two are
    // the same number and that number is at least 26.
    expect(MIN_SDK).toBeGreaterThanOrEqual(26);
    expect(MIN_SDK).toBeLessThanOrEqual(TARGET_SDK);
  });

  test("is what the generated manifest tells NativeActivity to load", () => {
    const { manifest } = androidProject({ name: "P", applicationId: "com.a.b" });
    expect(manifest).toContain(
      `<meta-data android:name="android.app.lib_name" android:value="${ENTRY_LIBRARY}" />`);
    expect(manifest).toContain("android.app.NativeActivity");
  });

  test("is taken from the environment when it is named", async () => {
    await withWork(async directory => {
      await writeFile(join(directory, "Cargo.toml"), "[package]\nname = \"blitsen-android\"\n");
      expect(await resolveEntryCrate({ BLITSEN_ANDROID_CRATE: directory })).toBe(directory);
      await expect(resolveEntryCrate({ BLITSEN_ANDROID_CRATE: join(directory, "gone") }))
        .rejects.toThrow("has no Cargo.toml");
    });
  });

  test("resolves out of the checkout with nothing set", async () => {
    expect(await resolveEntryCrate({})).toContain(ENTRY_CRATE);
  });
});

describe("the AndroidManifest.xml this build writes", () => {
  const project = () => androidProject({
    name: "Pong Deluxe",
    applicationId: "com.example.pong",
    version: "1.2.3",
    abis: ["arm64-v8a", "x86_64"],
  });

  test("carries the identity Android keys an install by", () => {
    const { manifest, versionCode: code } = project();
    expect(code).toBe(1_002_003);
    expect(manifest).toContain('package="com.example.pong"');
    expect(manifest).toContain(`android:versionCode="${code}"`);
    expect(manifest).toContain('android:versionName="1.2.3"');
    expect(manifest).toContain('android:label="Pong Deluxe"');
    expect(manifest).toContain(`android:minSdkVersion="${MIN_SDK}"`);
    expect(manifest).toContain(`android:targetSdkVersion="${TARGET_SDK}"`);
  });

  test("claims no dex and no extraction, which is what the archive is written for", () => {
    const { manifest } = project();
    // `hasCode="false"` is true because #142 chose android-activity's
    // native-activity backend: the activity class ships with the platform.
    expect(manifest).toContain('android:hasCode="false"');
    // Only legal for a stored, page-aligned .so — see the archive suite.
    expect(manifest).toContain('android:extractNativeLibs="false"');
  });

  test("declares every configuration change, because the failure is silent", () => {
    // #143 ran the paired control: without this attribute a dark-mode change
    // left the process alive, holding its last frame, never painting again.
    const { manifest } = project();
    expect(manifest).toContain(`android:configChanges="${CONFIG_CHANGES}"`);
    // Pinned whole rather than spot-checked. Anything absent from this list is
    // a change the activity is destroyed and recreated for, and the recreation
    // is what stops it painting; there is no subset that is obviously safe.
    expect(CONFIG_CHANGES.split("|")).toEqual(["orientation", "keyboardHidden", "keyboard",
      "screenSize", "screenLayout", "smallestScreenSize", "locale", "layoutDirection", "density",
      "uiMode", "fontScale", "navigation", "mcc", "mnc"]);
  });

  test("is debuggable only when the build is", () => {
    expect(project().manifest).not.toContain("android:debuggable");
    expect(androidProject({ name: "P", applicationId: "com.a.b", debuggable: true }).manifest)
      .toContain('android:debuggable="true"');
  });

  test("has no two hyphens in a comment, which aapt2 refuses the file over", () => {
    // Measured: naming the flag in the generated comment made every Android
    // build fail with `AndroidManifest.xml:2: error: not well-formed (invalid
    // token)`, because `--android` is two hyphens and XML forbids them inside
    // a comment. The rule is easy to break again by writing prose here.
    expect(project().manifest).toMatch(/<!--/);
    expect(/<!--(?:(?!-->)[\s\S])*?--(?!>)/.test(project().manifest)).toBe(false);
  });

  test("escapes a label XML would otherwise read as markup", () => {
    const { manifest } = androidProject({ name: "Tom & \"Jerry\" <b>", applicationId: "com.a.b" });
    expect(manifest).toContain('android:label="Tom &amp; &quot;Jerry&quot; &lt;b&gt;"');
    expect(manifest).not.toContain("<b>");
  });

  test("names no library derived from the application", () => {
    // The library is the crate's, on every build. This was the bug: a name
    // derived from the application ID could only have been produced by a crate
    // the build generated, and there is none.
    const { manifest, library } = androidProject({ name: "P", applicationId: "com.example.pong" });
    expect(library).toBe(ENTRY_LIBRARY);
    expect(manifest).not.toContain("com_example_pong");
  });
});

describe("the archive, which is written here because no tool will store every entry", () => {
  /** A zip reader, so the assertions are about the bytes rather than the writer. */
  function readZip(archive) {
    const end = archive.lastIndexOf(Buffer.from([0x50, 0x4b, 0x05, 0x06]));
    expect(end).toBeGreaterThan(-1);
    const count = archive.readUInt16LE(end + 10);
    let at = archive.readUInt32LE(end + 16);
    const entries = [];
    for (let index = 0; index < count; index += 1) {
      expect(archive.readUInt32LE(at)).toBe(0x02014b50);
      const method = archive.readUInt16LE(at + 10);
      const crc = archive.readUInt32LE(at + 16);
      const size = archive.readUInt32LE(at + 24);
      const nameLength = archive.readUInt16LE(at + 28);
      const offset = archive.readUInt32LE(at + 42);
      const name = archive.subarray(at + 46, at + 46 + nameLength).toString("utf8");
      // Follow the offset into the local header and read the bytes back out.
      expect(archive.readUInt32LE(offset)).toBe(0x04034b50);
      const localName = archive.readUInt16LE(offset + 26);
      expect(archive.subarray(offset + 30, offset + 30 + localName).toString("utf8")).toBe(name);
      const start = offset + 30 + localName + archive.readUInt16LE(offset + 28);
      entries.push({ name, method, crc, size, data: archive.subarray(start, start + size) });
      at += 46 + nameLength + archive.readUInt16LE(at + 30) + archive.readUInt16LE(at + 32);
    }
    return entries;
  }

  const sample = () => [
    { name: "AndroidManifest.xml", data: Buffer.from("binary xml") },
    { name: "assets/blitsen/index.html", data: Buffer.from("<html></html>") },
  ];

  test("stores every entry, and the bytes read back", () => {
    const entries = readZip(storedZip(sample()));
    expect(entries.map(entry => entry.name))
      .toEqual(["AndroidManifest.xml", "assets/blitsen/index.html"]);
    // Method 0 is the whole point: #144's noCompress, and the precondition for
    // android:extractNativeLibs="false".
    expect(entries.map(entry => entry.method)).toEqual([0, 0]);
    expect(entries[1].data.toString()).toBe("<html></html>");
    expect(entries[1].size).toBe(13);
  });

  test("records a CRC an unzip will check", () => {
    const [manifest] = readZip(storedZip(sample()));
    // The known CRC-32 of "binary xml", computed independently of the writer
    // (`python3 -c "import zlib; print(hex(zlib.crc32(b'binary xml')))"`).
    expect(manifest.crc.toString(16)).toBe("f257fe27");
    const corrupted = storedZip([{ name: "a", data: Buffer.from("binary xmm") }]);
    expect(readZip(corrupted)[0].crc).not.toBe(manifest.crc);
  });

  test("is byte-identical between two builds of the same input", () => {
    // Zip's per-entry MS-DOS timestamp is the one field that would otherwise
    // differ, and it is fixed. #71's reproducibility, on this artifact.
    const archive = storedZip(sample());
    expect(archive.equals(storedZip(sample()))).toBe(true);
    // Asserted as the constant rather than only as "two calls agree", because
    // two calls a microsecond apart agree under a clock too.
    for (let at = 0; archive.readUInt32LE(at) === 0x04034b50;) {
      expect(archive.readUInt16LE(at + 10)).toBe(0);
      expect(archive.readUInt16LE(at + 12)).toBe((0 << 9) | (1 << 5) | 1);
      at += 30 + archive.readUInt16LE(at + 26) + archive.readUInt16LE(at + 28)
        + archive.readUInt32LE(at + 18);
    }
  });

  test("refuses what it cannot encode rather than writing a wrong length", () => {
    const many = Array.from({ length: 0x10000 }, (_, index) =>
      ({ name: `f${index}`, data: Buffer.alloc(0) }));
    expect(() => storedZip(many)).toThrow("at most 65535 entries");
  });

  test("puts the manifest, the resources, the libraries and the assets in that order",
    async () => {
      await withWork(async directory => {
        const linked = join(directory, "linked");
        await mkdir(linked, { recursive: true });
        await writeFile(join(linked, "AndroidManifest.xml"), "xml");
        await writeFile(join(linked, "resources.arsc"), "arsc");
        const so = join(directory, "libblitsen_android.so");
        await writeFile(so, "elf");
        const assets = join(directory, "assets", ASSET_ROOT);
        await mkdir(join(assets, "assets"), { recursive: true });
        await writeFile(join(assets, "index.html"), "<html>");
        await writeFile(join(assets, "assets", "app.css"), "body{}");
        const entries = await apkEntries({
          linked,
          libraries: [
            { abi: "x86_64", entry: "lib/x86_64/libblitsen_android.so", source: so },
            { abi: "arm64-v8a", entry: "lib/arm64-v8a/libblitsen_android.so", source: so },
          ],
          assets: join(directory, "assets"),
        });
        expect(entries.map(entry => entry.name)).toEqual([
          "AndroidManifest.xml",
          "resources.arsc",
          // Sorted, not in the order the ABIs were given: readdir order and
          // argument order are both machine-dependent, and the archive is not.
          "lib/arm64-v8a/libblitsen_android.so",
          "lib/x86_64/libblitsen_android.so",
          `assets/${ASSET_ROOT}/assets/app.css`,
          `assets/${ASSET_ROOT}/index.html`,
        ]);
      });
    });

  test("says which piece is missing rather than packaging a hole", async () => {
    await withWork(async directory => {
      const linked = join(directory, "linked");
      const assets = join(directory, "assets");
      await mkdir(linked, { recursive: true });
      await mkdir(assets, { recursive: true });
      await expect(apkEntries({ linked, libraries: [], assets }))
        .rejects.toThrow("aapt2 produced no AndroidManifest.xml");
      await writeFile(join(linked, "AndroidManifest.xml"), "xml");
      await writeFile(join(linked, "resources.arsc"), "arsc");
      await expect(apkEntries({
        linked,
        libraries: [{ abi: "x86_64", entry: "lib/x86_64/x.so", source: join(directory, "gone") }],
        assets,
      })).rejects.toThrow("the x86_64 slice of this APK would be empty");
    });
  });
});

/** A minimal SDK tree: enough for the detector, nothing that could build. */
async function fakeSdk(directory, { ndk = "27.2.12479018",
  tools = ["aapt2", "zipalign", "apksigner"], buildTools = "34.0.0" } = {}) {
  const sdk = join(directory, "Sdk");
  if (ndk) await mkdir(join(sdk, "ndk", ndk), { recursive: true });
  await mkdir(join(sdk, "build-tools", buildTools), { recursive: true });
  for (const tool of tools) await writeFile(join(sdk, "build-tools", buildTools, tool), "");
  await mkdir(join(sdk, "platforms", "android-33"), { recursive: true });
  await writeFile(join(sdk, "platforms", "android-33", "android.jar"), "");
  return sdk;
}

// LIBCLANG_PATH is set so that the detector's answer does not depend on what
// LLVM the machine running the suite happens to have; the search itself is
// tested below, against directories this file makes.
const detected = (sdk, overrides = {}) => detectAndroidToolchain({
  env: { ANDROID_HOME: sdk, LIBCLANG_PATH: "/llvm/lib", ...overrides },
  // Answers by name, so that *which* binary is looked for is part of what this
  // asserts rather than something a stub hides.
  which: name => (name === "cargo-ndk" ? "/somewhere/cargo-ndk" : null),
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
      // Each tool is a path the plan puts in an argv, not a name off PATH: a
      // second SDK earlier on PATH would otherwise silently package the build.
      expect(toolchain.tools.aapt2).toBe(join(sdk, "build-tools", "34.0.0", "aapt2"));
      expect(toolchain.tools.zipalign).toBe(join(sdk, "build-tools", "34.0.0", "zipalign"));
      expect(toolchain.tools.apksigner).toBe(join(sdk, "build-tools", "34.0.0", "apksigner"));
      expect(toolchain.libclang).toBe("/llvm/lib");
      // The cross-compiler driver, by the name that is actually run.
      expect(toolchain.packager).toBe("/somewhere/cargo-ndk");
    });
  });

  test("names what is missing and the command that installs it", async () => {
    await withWork(async directory => {
      await expect(detected(join(directory, "nowhere"))).rejects.toThrow("nothing is there");
      const noNdk = await fakeSdk(join(directory, "a"), { ndk: null });
      await expect(detected(noNdk)).rejects.toThrow("sdkmanager \"ndk;");
      // And it says why it will not fetch one itself.
      await expect(detected(noNdk)).rejects.toThrow("Blitsen does not download it");
      // aapt2 and not aapt: v1 is the tool Google is removing, and nothing on
      // this path runs it now that the archive is written rather than packaged.
      const noAapt2 = await fakeSdk(join(directory, "b"), { tools: ["zipalign", "apksigner"] });
      await expect(detected(noAapt2)).rejects.toThrow("has no aapt2");
      const noSigner = await fakeSdk(join(directory, "d"), { tools: ["aapt2", "zipalign"] });
      await expect(detected(noSigner)).rejects.toThrow("has no apksigner");
      const sdk = await fakeSdk(join(directory, "c"));
      await expect(detectAndroidToolchain({
        env: { ANDROID_HOME: sdk, LIBCLANG_PATH: "/llvm/lib" }, which: () => null,
      })).rejects.toThrow("cargo-ndk is not on PATH");
    });
  });

  test("ANDROID_NDK_HOME outranks the SDK's own", async () => {
    await withWork(async directory => {
      const sdk = await fakeSdk(directory);
      const elsewhere = join(directory, "ndk-r99");
      await mkdir(elsewhere, { recursive: true });
      expect((await detected(sdk, { ANDROID_NDK_HOME: elsewhere })).ndk).toBe(elsewhere);
    });
  });

  test("a libclang is required, because this is the target that needs bindgen", async () => {
    await withWork(async directory => {
      const holding = join(directory, "llvm-99", "lib");
      await mkdir(holding, { recursive: true });
      await writeFile(join(holding, "libclang.so.1"), "");
      const empty = join(directory, "empty");
      await mkdir(empty, { recursive: true });
      expect(await findLibclang({}, [empty, holding])).toBe(holding);
      // A macOS install names it differently, and neither name is `clang`.
      await writeFile(join(empty, "clang"), "");
      expect(await findLibclang({}, [empty])).toBe(null);
      await writeFile(join(empty, "libclang.dylib"), "");
      expect(await findLibclang({}, [empty])).toBe(empty);
      // Set means set: bindgen reads the same variable, so this file does not
      // get to disagree with it.
      expect(await findLibclang({ LIBCLANG_PATH: "/named" }, [])).toBe("/named");
      expect(await findLibclang({}, [])).toBe(null);
    });
  });
});

describe("where cargo will leave the shared objects", () => {
  test("is asked of cargo, not assumed to be <workspace>/target", async () => {
    const asked = [];
    const directory = await cargoTargetDirectory("/checkout/crates/blitsen-android",
      async command => {
        asked.push(command);
        return { code: 0, stdout: JSON.stringify({ target_directory: "/shared/target" }),
          stderr: "" };
      });
    // A wrong guess here does not fail — it packages a stale library from an
    // earlier build — so CARGO_TARGET_DIR and build.target-dir are resolved.
    expect(directory).toBe("/shared/target");
    expect(asked[0]).toEqual(["cargo", "metadata", "--no-deps", "--format-version", "1",
      "--manifest-path", join("/checkout/crates/blitsen-android", "Cargo.toml")]);
  });

  test("refuses to continue on an answer it cannot use", async () => {
    await expect(cargoTargetDirectory("/crate",
      async () => ({ code: 101, stdout: "", stderr: "no such manifest" })))
      .rejects.toThrow("cargo metadata exited 101");
    await expect(cargoTargetDirectory("/crate",
      async () => ({ code: 0, stdout: "{}", stderr: "" })))
      .rejects.toThrow("named no target_directory");
  });
});

describe("the build plan", () => {
  const toolchain = {
    sdk: "/sdk",
    ndk: "/sdk/ndk/27",
    platform: "/sdk/platforms/android-33/android.jar",
    libclang: "/llvm/lib",
    tools: { aapt2: "/sdk/bt/aapt2", zipalign: "/sdk/bt/zipalign", apksigner: "/sdk/bt/apksigner" },
  };
  const plan = (overrides = {}) => apkPlan({
    project: androidProject({ name: "Pong", applicationId: "com.blitsen.pong", version: "1.2.3" }),
    directory: "/build/.Pong.apk.blitsen-android",
    entryCrate: "/checkout/crates/blitsen-android",
    targetDirectory: "/checkout/target",
    toolchain,
    ...overrides,
  });

  test("compiles the entry crate itself, at the API level the manifest claims", () => {
    const { compile } = plan();
    // -p blitsen-android, and no generated crate: #143 established that this is
    // the invocation that produces a working libblitsen_android.so.
    expect(compile.command).toEqual(["cargo", "ndk", "-t", "arm64-v8a", "-t", "x86_64",
      "-P", String(MIN_SDK), "build", "--release",
      "--manifest-path", join("/checkout/crates/blitsen-android", "Cargo.toml"),
      "-p", ENTRY_CRATE]);
    // MIN_SDK and not TARGET_SDK: -P decides which libc symbols the .so may
    // bind, and one built against 33 fails to dlopen on everything below it.
    expect(compile.command).not.toContain(String(TARGET_SDK));
    expect(compile.environment.ANDROID_NDK_HOME).toBe("/sdk/ndk/27");
    expect(compile.environment.LIBCLANG_PATH).toBe("/llvm/lib");
  });

  test("takes the shared objects from where cargo left them", () => {
    expect(plan().libraries).toEqual([
      { abi: "arm64-v8a", triple: "aarch64-linux-android",
        source: join("/checkout/target/aarch64-linux-android/release", ENTRY_SO),
        entry: `lib/arm64-v8a/${ENTRY_SO}` },
      { abi: "x86_64", triple: "x86_64-linux-android",
        source: join("/checkout/target/x86_64-linux-android/release", ENTRY_SO),
        entry: `lib/x86_64/${ENTRY_SO}` },
    ]);
    expect(plan({ release: false }).libraries[0].source)
      .toBe(join("/checkout/target/aarch64-linux-android/debug", ENTRY_SO));
  });

  test("links only the manifest, and asks aapt2 for a directory", () => {
    const { link, paths } = plan();
    // --output-to-dir, so what comes back is AndroidManifest.xml and
    // resources.arsc rather than an archive whose compression is aapt2's.
    expect(link.command).toEqual(["/sdk/bt/aapt2", "link", "-o", paths.linked, "--output-to-dir",
      "-I", "/sdk/platforms/android-33/android.jar", "--manifest", paths.manifest,
      "--min-sdk-version", String(MIN_SDK), "--target-sdk-version", String(TARGET_SDK)]);
    // No -A: the assets go in stored, which is the one thing aapt2 would undo.
    expect(link.command).not.toContain("-A");
  });

  test("aligns to a page, which extractNativeLibs=false requires", () => {
    expect(plan().align.command)
      .toEqual(["/sdk/bt/zipalign", "-f", "-p", "4", plan().paths.unaligned, plan().paths.apk]);
  });

  test("signs with the debug key until a real one is named, and never in the argv", () => {
    const release = plan();
    expect(release.debugSigned).toBe(true);
    expect(release.keystore).toContain("debug.keystore");
    expect(release.sign.command).toContain("--ks-key-alias");
    expect(release.sign.command).toContain("androiddebugkey");
    // `env:` and not the password: an argument is visible in `ps` and lands in
    // shell history and CI logs.
    expect(release.sign.command).toContain("env:BLITSEN_APKSIGNER_KEYSTORE_PASSWORD");
    expect(release.sign.command).not.toContain("android");
    expect(release.sign.environment.BLITSEN_APKSIGNER_KEYSTORE_PASSWORD).toBe("android");
    const signed = plan({ keystore: "/keys/release.jks", keystorePassword: "hunter2" });
    expect(signed.debugSigned).toBe(false);
    expect(signed.sign.command).toContain("/keys/release.jks");
    expect(signed.sign.command).not.toContain("hunter2");
    // No alias, because a keystore holding one key needs none and guessing one
    // fails with "no key with alias" rather than with anything a reader can act
    // on. The key's password defaults to the store's, which is what keytool
    // writes when it is not asked for two.
    expect(signed.sign.command).not.toContain("--ks-key-alias");
    expect(signed.sign.environment.BLITSEN_APKSIGNER_KEYSTORE_PASSWORD).toBe("hunter2");
    expect(signed.sign.environment.BLITSEN_APKSIGNER_KEY_PASSWORD).toBe("hunter2");
  });

  test("names the key inside a store that holds more than one, still not in the argv", () => {
    const signed = plan({
      keystore: "/keys/release.jks", keystorePassword: "hunter2",
      keyAlias: "upload", keyPassword: "different",
    });
    expect(signed.sign.command).toContain("--ks-key-alias");
    expect(signed.sign.command).toContain("upload");
    expect(signed.sign.command).toContain("env:BLITSEN_APKSIGNER_KEY_PASSWORD");
    expect(signed.sign.command).not.toContain("different");
    expect(signed.sign.environment.BLITSEN_APKSIGNER_KEY_PASSWORD).toBe("different");
    // And neither reaches the debug key, whose alias and password are fixed.
    const debug = plan({ keyAlias: "upload", keyPassword: "different" });
    expect(debug.sign.command).toContain("androiddebugkey");
    expect(debug.sign.environment.BLITSEN_APKSIGNER_KEY_PASSWORD).toBe("android");
  });

  test("refuses a keystore whose password was not put in the environment", () => {
    expect(() => plan({ keystore: "/keys/release.jks" }))
      .toThrow("BLITSEN_ANDROID_KEYSTORE_PASSWORD");
    // A debug build is signed too, so the refusal is not a release-only rule —
    // an unsigned APK installs nowhere on either profile.
    expect(() => plan({ keystore: "/keys/release.jks", release: false }))
      .toThrow("BLITSEN_ANDROID_KEYSTORE_PASSWORD");
  });

  test("a debug build is the debug profile and is still signed", () => {
    const debug = plan({ release: false });
    expect(debug.compile.command).not.toContain("--release");
    expect(debug.sign.command).toContain("--ks");
    expect(debug.debugSigned).toBe(true);
  });
});

describe("the debug keystore", () => {
  test("is created when there is none, because otherwise nothing installs", async () => {
    await withWork(async directory => {
      const path = join(directory, "home", ".android", "debug.keystore");
      const commands = [];
      const created = await ensureDebugKeystore(path, async command => {
        commands.push(command);
        await mkdir(join(directory, "home", ".android"), { recursive: true });
        await writeFile(path, "keystore");
        return { code: 0, stdout: "", stderr: "" };
      });
      expect(created).toBe(true);
      expect(commands[0][0]).toBe("keytool");
      expect(commands[0]).toContain("androiddebugkey");
      expect(commands[0]).toContain("CN=Android Debug, O=Android, C=US");
    });
  });

  test("is left alone when there is one", async () => {
    await withWork(async directory => {
      const path = join(directory, "debug.keystore");
      await writeFile(path, "keystore");
      expect(await ensureDebugKeystore(path, async () => {
        throw new Error("keytool should not have run");
      })).toBe(false);
    });
  });

  test("says a JDK is missing rather than failing inside apksigner", async () => {
    await withWork(async directory => {
      await expect(ensureDebugKeystore(join(directory, "none.keystore"),
        async () => ({ code: 127, stdout: "", stderr: "" })))
        .rejects.toThrow("Install a JDK");
    });
  });
});

describe("an Android build, with every subprocess stubbed", () => {
  const stubToolchain = () => async () => ({
    sdk: "/sdk", ndk: "/sdk/ndk/27", buildTools: "/sdk/bt", buildToolsVersion: "34.0.0",
    platform: "/sdk/p", packager: "cargo-ndk", libclang: "/llvm/lib",
    tools: { aapt2: "aapt2", zipalign: "zipalign", apksigner: "apksigner" },
  });

  /**
   * Stands in for the four tools, doing the one thing each leaves behind: a
   * `.so` per triple, a linked directory, an aligned copy, a signature. The
   * archive itself is not stubbed — it is written by the code under test.
   */
  const stubRun = targetDirectory => async command => {
    if (command[0] === "rustup") {
      return { code: 0, stdout: "aarch64-linux-android\nx86_64-linux-android\n", stderr: "" };
    }
    if (command[1] === "metadata") {
      return { code: 0, stdout: JSON.stringify({ target_directory: targetDirectory }), stderr: "" };
    }
    if (command[1] === "ndk") {
      for (const triple of ["aarch64-linux-android", "x86_64-linux-android"]) {
        const at = join(targetDirectory, triple, "release");
        await mkdir(at, { recursive: true });
        await writeFile(join(at, ENTRY_SO), `ELF ${triple}`);
      }
      return { code: 0, stdout: "", stderr: "" };
    }
    if (command[0] === "aapt2") {
      const at = command[command.indexOf("-o") + 1];
      await mkdir(at, { recursive: true });
      await writeFile(join(at, "AndroidManifest.xml"), "binary xml");
      await writeFile(join(at, "resources.arsc"), "arsc");
      return { code: 0, stdout: "", stderr: "" };
    }
    if (command[0] === "zipalign") {
      await writeFile(command.at(-1), await readFile(command.at(-2)));
      return { code: 0, stdout: "", stderr: "" };
    }
    return { code: 0, stdout: "", stderr: "" };
  };

  const withCrate = async directory => {
    const crate = join(directory, "blitsen-android");
    await mkdir(crate, { recursive: true });
    await writeFile(join(crate, "Cargo.toml"), "[package]\nname = \"blitsen-android\"\n");
    return crate;
  };

  test("compiles, links, writes the archive, aligns and signs — then reports it", async () => {
    await withWork(async directory => {
      const root = await application(directory);
      const crate = await withCrate(directory);
      const commands = [];
      const steps = [];
      const stub = stubRun(join(directory, "target"));
      const result = await buildAndroid({
        root,
        name: "Pong",
        outfile: join(directory, "Pong.apk"),
        appVersion: "1.2.3",
        env: { BLITSEN_ANDROID_CRATE: crate },
        run: command => { commands.push(command); return stub(command); },
        detect: stubToolchain(),
        progress: event => steps.push(event),
      });
      // In order, and each is the real argv rather than a name: the installed
      // targets, where cargo will put the output, the cross-compile, the
      // resource link, the alignment, the signature.
      expect(commands.map(command => `${command[0]} ${command[1]}`)).toEqual([
        "rustup target", "cargo metadata", "cargo ndk", "aapt2 link", "zipalign -f",
        "apksigner sign",
      ]);
      // `-p blitsen-android`, and no generated crate anywhere in it.
      expect(commands[2].slice(-2)).toEqual(["-p", ENTRY_CRATE]);
      expect(result.applicationId).toBe("com.blitsen.pong");
      expect(result.versionCode).toBe(versionCode("1.2.3"));
      expect(result.abis).toEqual(["arm64-v8a", "x86_64"]);
      expect(result.assets).toBe(3);
      expect(result.debugSigned).toBe(true);
      expect((await stat(result.outfile)).size).toBeGreaterThan(0);
      // The archive holds one shared object per ABI and the application under
      // assets/blitsen/. This is the claim #148 could not make before: an APK
      // the CLI built with the engine's own entry point in it.
      expect(result.entries.map(entry => entry.name)).toEqual([
        "AndroidManifest.xml", "resources.arsc",
        `lib/arm64-v8a/${ENTRY_SO}`, `lib/x86_64/${ENTRY_SO}`,
        `assets/${ASSET_ROOT}/app.js`,
        `assets/${ASSET_ROOT}/assets/app.css`,
        `assets/${ASSET_ROOT}/${ASSET_INDEX}`,
        `assets/${ASSET_ROOT}/index.html`,
      ]);
      const staging = join(directory, ".Pong.apk.blitsen-android");
      expect(await readFile(join(staging, "AndroidManifest.xml"), "utf8"))
        .toContain('package="com.blitsen.pong"');
      expect(await readFile(join(staging, "assets", ASSET_ROOT, "index.html"), "utf8"))
        .toContain("./assets/app.css");
      // The three notes a reader has to see: what was signed, that every entry
      // is stored (#144's noCompress), and that it is not an AAB.
      const notes = steps.flatMap(step => step.notes ?? []).join("\n");
      expect(notes).toContain("debug key");
      expect(notes).toContain("App Bundle");
      expect(notes).toContain("noCompress");
      expect(notes).toContain("stored rather than deflated");
    });
  });

  test("the artifact is a zip with the engine in it, and nothing deflated", async () => {
    await withWork(async directory => {
      const root = await application(directory);
      const crate = await withCrate(directory);
      const result = await buildAndroid({
        root,
        name: "Pong",
        outfile: join(directory, "Pong.apk"),
        env: { BLITSEN_ANDROID_CRATE: crate },
        run: stubRun(join(directory, "target")),
        detect: stubToolchain(),
      });
      const archive = await readFile(result.outfile);
      expect(archive.subarray(0, 4)).toEqual(Buffer.from([0x50, 0x4b, 0x03, 0x04]));
      // Every local header's compression method, walked out of the file itself.
      let seen = 0;
      for (let at = 0; at + 30 <= archive.length;) {
        if (archive.readUInt32LE(at) !== 0x04034b50) break;
        expect(archive.readUInt16LE(at + 8)).toBe(0);
        seen += 1;
        at += 30 + archive.readUInt16LE(at + 26) + archive.readUInt16LE(at + 28)
          + archive.readUInt32LE(at + 18);
      }
      expect(seen).toBe(result.entries.length);
      // And what the cross-compile produced is in there verbatim, which is the
      // whole of what "this APK contains the engine" means at this level.
      expect(archive.includes(Buffer.from("ELF x86_64-linux-android"))).toBe(true);
    });
  });

  test("a named key reaches apksigner, and the debug keystore is left alone", async () => {
    await withWork(async directory => {
      const root = await application(directory);
      const crate = await withCrate(directory);
      const commands = [];
      const stub = stubRun(join(directory, "target"));
      const result = await buildAndroid({
        root,
        name: "Pong",
        outfile: join(directory, "Pong.apk"),
        keystore: "/keys/release.jks",
        keystorePassword: "hunter2",
        keyAlias: "upload",
        keyPassword: "different",
        env: { BLITSEN_ANDROID_CRATE: crate },
        run: command => { commands.push(command); return stub(command); },
        detect: stubToolchain(),
      });
      const sign = commands.find(command => command[0] === "apksigner");
      expect(sign).toContain("/keys/release.jks");
      expect(sign).toContain("upload");
      expect(sign).not.toContain("hunter2");
      expect(sign).not.toContain("different");
      // keytool is never reached: a build that names a key must not invent one.
      expect(commands.some(command => command[0] === "keytool")).toBe(false);
      expect(result.debugSigned).toBe(false);
      expect(result.keystore).toBe("/keys/release.jks");
    });
  });

  test("a tool that fails stops the build and says which one", async () => {
    await withWork(async directory => {
      const root = await application(directory);
      const crate = await withCrate(directory);
      const stub = stubRun(join(directory, "target"));
      const failing = at => async command => {
        const result = await stub(command);
        return command.includes(at) ? { ...result, code: 3 } : result;
      };
      for (const [at, said] of [["ndk", "cargo ndk exited 3"], ["aapt2", "aapt2 link exited 3"],
        ["zipalign", "zipalign exited 3"], ["apksigner", "apksigner exited 3"]]) {
        await expect(buildAndroid({
          root,
          name: "Pong",
          outfile: join(directory, "Pong.apk"),
          force: true,
          env: { BLITSEN_ANDROID_CRATE: crate },
          run: failing(at),
          detect: stubToolchain(),
        })).rejects.toThrow(said);
      }
    });
  });

  test("refuses to cross-compile for a Rust target that is not installed", async () => {
    await withWork(async directory => {
      const root = await application(directory);
      const crate = await withCrate(directory);
      await expect(buildAndroid({
        root,
        name: "Pong",
        outfile: join(directory, "Pong.apk"),
        env: { BLITSEN_ANDROID_CRATE: crate },
        run: async command => (command[0] === "rustup"
          ? { code: 0, stdout: "x86_64-unknown-linux-gnu\n", stderr: "" }
          : { code: 0, stdout: "", stderr: "" }),
        detect: stubToolchain(),
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
