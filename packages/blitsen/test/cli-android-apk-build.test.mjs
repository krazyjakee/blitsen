// End-to-end Android APK orchestration with every external tool stubbed.
import { describe, expect, test } from "bun:test";
import { mkdir, readFile, stat, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { buildAndroid, ENTRY_CRATE, ENTRY_SO, versionCode } from "../src/android.mjs";
import { ASSET_INDEX, ASSET_ROOT } from "../src/android-assets.mjs";
import { androidApplication as application, withAndroidWork as withWork } from "./android-apk-fixtures.mjs";

describe("an Android build, with every subprocess stubbed", () => {
  const stubToolchain = () => async () => ({
    sdk: "/sdk", ndk: "/sdk/ndk/27", buildTools: "/sdk/bt", buildToolsVersion: "34.0.0",
    llvm: "/sdk/ndk/27/llvm", sysroot: "/sdk/ndk/27/llvm/sysroot",
    platform: "/sdk/p", packager: "cargo-ndk", libclang: "/llvm/lib",
    tools: { aapt2: "aapt2", d8: "d8", javac: "javac", zipalign: "zipalign",
      apksigner: "apksigner" },
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
    if (command[0] === "javac") {
      const at = command[command.indexOf("-d") + 1];
      const packageDirectory = join(at, "com", "blitsen", "runtime");
      await mkdir(packageDirectory, { recursive: true });
      for (const name of ["NotificationBridge.class",
        "NotificationBridge$ActivationReceiver.class"]) {
        await writeFile(join(packageDirectory, name), `class ${name}`);
      }
      return { code: 0, stdout: "", stderr: "" };
    }
    if (command[0] === "d8") {
      const at = command[command.indexOf("--output") + 1];
      await mkdir(at, { recursive: true });
      await writeFile(join(at, "classes.dex"), "dex");
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
      // resource link, the alignment, the signature. A clean machine also
      // creates the conventional debug keystore immediately before signing;
      // a developer machine may already have it, so that step is optional here
      // and its full command has dedicated coverage above.
      const commandNames = commands.map(command => `${command[0]} ${command[1]}`);
      expect(commandNames.filter(command => command !== "keytool -genkeypair")).toEqual([
        "rustup target", "cargo metadata", "cargo ndk", "javac -source", "d8 --min-api",
        "aapt2 link", "zipalign -f", "apksigner sign",
      ]);
      const keytool = commandNames.indexOf("keytool -genkeypair");
      expect(keytool === -1 || keytool === commandNames.indexOf("apksigner sign") - 1).toBe(true);
      // `-p blitsen-android`, and no generated crate anywhere in it.
      expect(commands[2].slice(-2)).toEqual(["-p", ENTRY_CRATE]);
      expect(result.applicationId).toBe("com.blitsen.pong");
      expect(result.versionCode).toBe(versionCode("1.2.3"));
      expect(result.abis).toEqual(["arm64-v8a", "x86_64"]);
      expect(result.assets).toBe(4);
      expect(result.debugSigned).toBe(true);
      expect((await stat(result.outfile)).size).toBeGreaterThan(0);
      // The archive holds one shared object per ABI and the application under
      // assets/blitsen/. This is the claim #148 could not make before: an APK
      // the CLI built with the engine's own entry point in it.
      expect(result.entries.map(entry => entry.name)).toEqual([
        "AndroidManifest.xml", "resources.arsc",
        "classes.dex",
        `lib/arm64-v8a/${ENTRY_SO}`, `lib/x86_64/${ENTRY_SO}`,
        `assets/${ASSET_ROOT}/app.js`,
        `assets/${ASSET_ROOT}/assets/app.css`,
        `assets/${ASSET_ROOT}/${ASSET_INDEX}`,
        `assets/${ASSET_ROOT}/blitsen.runtime.json`,
        `assets/${ASSET_ROOT}/index.html`,
      ]);
      const staging = join(directory, ".Pong.apk.blitsen-android");
      expect(await readFile(join(staging, "AndroidManifest.xml"), "utf8"))
        .toContain('package="com.blitsen.pong"');
      expect(await readFile(join(staging, "assets", ASSET_ROOT, "index.html"), "utf8"))
        .toContain("./assets/app.css");
      expect(JSON.parse(await readFile(
        join(staging, "assets", ASSET_ROOT, "blitsen.runtime.json"), "utf8",
      )).storageIdentity).toBe("com.blitsen.pong");
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
      for (const [at, said] of [["ndk", "cargo ndk exited 3"], ["javac", "javac exited 3"],
        ["d8", "d8 exited 3"], ["aapt2", "aapt2 link exited 3"],
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
