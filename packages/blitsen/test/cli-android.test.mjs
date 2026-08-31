// `blitsen build --android`: what the application becomes (issue #148).
//
// The half of the Android build that is about the *application* — the files
// that go under `assets/`, the listing that travels with them, the identity
// the artifact is keyed by, and the command line that asks for all of it. The
// other half, which is about the artifact and the machine that produces it, is
// in `cli-android-apk.test.mjs`; the two are split because the file was over
// this repository's length ceiling, and this is the seam it wanted.
//
// **The constants are checked against the Rust that reads them.** `apk.rs` is
// the reader for the index this writes, and there is no build step that could
// derive one side from the other, so the first suite parses the Rust and fails
// if the two have drifted. Three string literals and a schema in two languages
// is exactly the shape of thing that silently disagrees.
import { describe, expect, test } from "bun:test";
import { readFile, stat, writeFile } from "node:fs/promises";
import { join } from "node:path";
import {
  ANDROID_ABIS, ANDROID_NOTICES_FILE, androidNotices, androidProject, applicationId, DEFAULT_ABIS,
  resolveAbis, versionCode,
} from "../src/android.mjs";
import {
  ASSET_INDEX, ASSET_ROOT, INDEX_VERSION, assetIndex, stageAndroidAssets,
} from "../src/android-assets.mjs";
import { MIN_SDK } from "../src/android-toolchain.mjs";
import { main, parseArgs } from "../src/cli.mjs";
import { changed, decodeFrame, describe as describeFrame } from "./run-android-smoke.mjs";
import { capture } from "./cli-support.mjs";
import {
  androidApplication as application, withAndroidWork as withWork,
} from "./android-apk-fixtures.mjs";

const apkSource = join(import.meta.dir, "../../../crates/blitsen-host/src/apk.rs");

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
  test("are staged uncompressed, under the name the host reads", async () => {
    await withWork(async directory => {
      const source = join(directory, "NOTICES.txt");
      await writeFile(source, "THIRD-PARTY NOTICES\n");
      const notices = await androidNotices({ BLITSEN_NOTICES_PATH: source });
      // Forced before it was chosen. `aapt` v1 strips `.gz` from an asset name
      // and inflates the contents — measured on a real APK — so every artifact
      // built through it reported itself uncleared while carrying the notices
      // it owes. That packager is gone, and the name stays for a reason rather
      // than a workaround: every entry in the archive is stored, so a gzip
      // inside it compresses nothing and costs an inflate on the one read.
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

  test("--android grades the application against Android's module table", async () => {
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
