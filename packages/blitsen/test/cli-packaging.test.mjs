import { describe, expect, test } from "bun:test";
import { copyFile, cp, mkdir, mkdtemp, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";
import { buildStandalone, describeExecutableBinary, describeNativeBinary, notificationActivation }
  from "../src/export.mjs";
import { activationEntryPoint, developmentBundle, developmentIdentifier,
  notificationActivatorClsid, packageBuild, signArgv, signArtifact } from "../src/packaging.mjs";
import { viteBase, addonFixtures, icon, signHook, compiler, engineAddon, engineBuilt, compileAddon, elfHeader, executableStub, exportedName, nativeStub, withStubbedExport, withArtifact } from "./cli-support.mjs";

describe("directory CLI", () => {
  test("reads the container header a .node must have to load on this host", () => {
    expect(describeNativeBinary(elfHeader({ machine: 0x3e })))
      .toEqual({ format: "ELF", platform: "linux", architectures: ["x64"] });
    expect(describeNativeBinary(elfHeader({ machine: 0x1234 })).architectures).toEqual(["0x1234"]);
    // An executable, an archive or a text file renamed .node is not a library.
    expect(describeNativeBinary(elfHeader({ type: 2 }))).toBeNull();
    expect(describeNativeBinary(Buffer.alloc(64, 0x41))).toBeNull();
    expect(describeNativeBinary(Buffer.from("// placeholder addon\n"))).toBeNull();
  });

  test("reads an executable's header as an executable, not as a library", () => {
    // The Phase 2 runtime is the other question the same headers answer, and the
    // two must not be interchangeable: an addon where the runtime belongs would
    // link into an artifact that cannot start.
    for (const target of ["linux-x64", "darwin-arm64", "win32-x64"]) {
      const platform = target.slice(0, target.lastIndexOf("-"));
      const architecture = target.slice(target.lastIndexOf("-") + 1);
      expect(describeExecutableBinary(executableStub(target)))
        .toMatchObject({ platform, architectures: [architecture] });
      // A shared library is not an executable — except on ELF, where a
      // position-independent executable is the same type as one.
      expect(describeNativeBinary(executableStub(target))).toBeNull();
      if (platform !== "linux") expect(describeExecutableBinary(nativeStub(target))).toBeNull();
    }
  });

  test("refuses a .node it cannot load instead of exporting a launch crash", async () => {
    await withStubbedExport(async ({ directory, nativePath, outfile }) => {
      const build = addons => buildStandalone({
        root: viteBase, width: 800, height: 600, title: "Base", outfile, force: true, addons,
      }, nativePath);
      const foreign = join(directory, "foreign.node");
      // Foreign to *this* host: 0xb7 is aarch64 and 0x3e is x86-64, so an arm64
      // runner gets the x86-64 fixture and the addon is refused there too. It
      // was fixed at aarch64, which made this a pass on x64 and a silent
      // no-op-turned-failure on the arm64 targets a release publishes (#133).
      const [machine, foreignTarget] = process.arch === "arm64"
        ? [0x3e, "linux-x64"]
        : [0xb7, "linux-arm64"];
      await writeFile(foreign, elfHeader({ machine }));
      await expect(build([foreign])).rejects.toThrow("native addon foreign.node is built for "
        + `${foreignTarget} (ELF), but this export runs on ${process.platform}-${process.arch}`);
      const text = join(directory, "notes.node");
      await writeFile(text, "not a library\n");
      await expect(build([text]))
        .rejects.toThrow("notes.node is not a native addon: a .node file must be an ELF");
      await expect(build([icon])).rejects.toThrow(`a native addon must be a .node file: ${icon}`);
      await expect(build([join(directory, "absent.node")]))
        .rejects.toThrow("native addon does not exist:");
      // Two addons cannot both claim one name in the application tree.
      const other = join(directory, "nested", "foreign.node");
      await mkdir(dirname(other), { recursive: true });
      await copyFile(foreign, other);
      await expect(build([foreign, other]))
        .rejects.toThrow("two native addons would both be exported as foreign.node");
    });
  });

  test.skipIf(!compiler)("refuses a host shared library that is not a Node-API addon", async () => {
    await withStubbedExport(async ({ directory, nativePath, outfile }) => {
      const plain = compileAddon(directory, "plain.c", "plain.node");
      await expect(buildStandalone({
        root: viteBase, width: 800, height: 600, title: "Base", outfile, addons: [plain],
      }, nativePath)).rejects.toThrow("native addon plain.node does not export "
        + "napi_register_module_v1: Blitsen loads Node-API addons, not V8/NAN addons");
    });
  });

  test.skipIf(!compiler)("carries a declared addon into both asset layouts", async () => {
    await withStubbedExport(async ({ directory, nativePath, outfile }) => {
      // Declared from outside the ingested directory, which is where an addon
      // lives: node_modules/<package>/build/Release, target/release.
      const addon = compileAddon(directory);
      const events = [];
      const embedded = await buildStandalone({
        root: viteBase, width: 800, height: 600, title: "Base", outfile,
        addons: [addon], progress: event => events.push(event),
      }, nativePath);
      expect(embedded.addons).toEqual(["greet.node"]);
      expect(embedded.assets).toBe(10);
      expect(embedded.manifest.find(asset => asset.path === "greet.node").native).toBeTrue();
      expect(events[0].notes[1]).toBe("carried 1 native addon: greet.node "
        + "(load one from a module script with createRequire(import.meta.url))");

      const side = await buildStandalone({
        root: viteBase, width: 800, height: 600, title: "Base", assets: "side-loaded",
        outfile: join(directory, "Side"), addons: [addon],
      }, nativePath);
      // The staged bytes are the compiled library, unmodified: dlopen reads them.
      expect(Buffer.compare(await readFile(join(side.assetDirectory, "greet.node")),
        await readFile(addon))).toBe(0);
    });
  });

  test.skipIf(!compiler)("keeps a declared addon in its place inside the output", async () => {
    await withStubbedExport(async ({ directory, nativePath, outfile }) => {
      const root = join(directory, "dist");
      await cp(viteBase, root, { recursive: true });
      const addon = compileAddon(join(root, "assets"));
      const result = await buildStandalone({
        root, width: 800, height: 600, title: "Base", outfile, addons: [addon],
      }, nativePath);
      // The specifier the application was written against still resolves, and a
      // declared file is carried rather than reported as unreachable and dropped.
      expect(result.addons).toEqual(["assets/greet.node"]);
      expect(result.unreferenced).toEqual(["assets/index-BASE.js.map", "assets/orphan.txt"]);
    });
  });

  test.skipIf(!compiler || !engineBuilt)(
    "loads a carried addon from the exported executable", async () => {
      const workspace = await mkdtemp(join(tmpdir(), "blitsen-addon-export-"));
      try {
        const root = join(workspace, "app");
        await cp(join(addonFixtures, "app"), root, { recursive: true });
        const addon = compileAddon(workspace);
        const assertLoaded = `(() => {
          const text = document.getElementById("greeting").textContent;
          if (text !== "blitsen-addon-ok") throw new Error("addon did not load: " + text);
        })()`;
        for (const assets of ["embedded", "side-loaded"]) {
          const outfile = join(workspace, `AddonApp-${assets}`);
          const result = await buildStandalone({
            root, width: 400, height: 300, title: "Addon", outfile, assets, addons: [addon],
          }, engineAddon);
          expect(result.addons).toEqual(["greet.node"]);
          // Run from an unrelated directory: the addon is found through the export,
          // not through the working directory it happened to be built in.
          const run = Bun.spawnSync({
            cmd: [result.outfile],
            cwd: tmpdir(),
            env: {
              PATH: "",
              BLITSEN_STANDALONE_CHECK: "1",
              BLITSEN_STANDALONE_CHECK_DELAY: "250",
              BLITSEN_STANDALONE_CHECK_ASSERT: assertLoaded,
            },
            stdout: "pipe",
            stderr: "pipe",
          });
          expect(run.stderr.toString()).toBe("");
          expect(run.exitCode).toBe(0);
        }
      } finally {
        await rm(workspace, { recursive: true, force: true });
      }
    }, 120_000);

  test("packages a Linux desktop entry, icon and signature into the build", async () => {
    await withStubbedExport(async ({ directory, nativePath, outfile }) => {
      const events = [];
      const result = await buildStandalone({
        root: viteBase, width: 800, height: 600, title: "Pong Deluxe", outfile,
        icon, sign: signHook, platform: "linux", progress: event => events.push(event),
      }, nativePath);
      // The artifact, not the path asked for: a Windows target is named `.exe`,
      // and every name below is derived from the executable rather than assumed.
      const artifact = exportedName(outfile);
      const stem = basename(artifact);
      expect(result.outfile).toBe(artifact);
      // Steps ③–⑤ announce themselves as they finish, with what they produced.
      expect(events.map(event => event.step)).toEqual(["collect", "link", "package"]);
      expect(events[0].detail).toBe("9 embedded assets");
      expect(events[0].notes[0]).toBe("dropped 2 files unreachable from index.html "
        + "(--include <glob> keeps them): assets/index-BASE.js.map, assets/orphan.txt");
      expect(events[1].detail).toBe(artifact);
      expect(events[2].detail).toBe(`linux: ${result.packaging.artifacts.join(", ")}`);
      expect(events[2].notes).toEqual([`signed ${artifact} with: ${signHook}`]);
      expect(result.packaging.artifacts)
        .toEqual([join(directory, `${stem}.desktop`), join(directory, `${stem}.png`)]);
      const entry = await readFile(join(directory, `${stem}.desktop`), "utf8");
      expect(entry).toContain("[Desktop Entry]\nType=Application\n");
      expect(entry).toContain("Name=Pong Deluxe\n");
      // Read back through the desktop-entry quoting rules rather than compared
      // to a raw path: backslashes are reserved, so a Windows path arrives
      // quoted and escaped. The rules themselves are the next test's subject.
      const exec = /^Exec=(.*)$/m.exec(entry)[1];
      // `%u` remains the non-D-Bus fallback and protocol-handler argument; a
      // packaged notification activation uses ActivateAction instead (#252).
      expect(exec.endsWith(" %u")).toBe(true);
      const command = exec.slice(0, -" %u".length);
      expect(command.startsWith('"') ? command.slice(1, -1).replace(/\\(.)/g, "$1") : command)
        .toBe(artifact);
      // This file is what a notification's `desktop-entry` hint names, so the
      // entry has to say that the application uses notifications — that is what
      // lists it in the desktop's notification settings.
      expect(entry).toContain("X-GNOME-UsesNotifications=true\n");
      expect(entry).toContain(`Icon=${join(directory, `${stem}.png`)}\n`);
      // Linux takes the PNG as it is; only Windows and macOS need a container.
      expect(Buffer.compare(await readFile(join(directory, `${stem}.png`)), await readFile(icon)))
        .toBe(0);
      expect(result.signed).toEqual({ command: signHook, artifact });
      expect(await readFile(`${artifact}.signed`, "utf8")).toBe(`${artifact}\n`);
    });
  });

  test("quotes a desktop Exec holding reserved characters and omits an absent icon", async () => {
    await withArtifact(async ({ directory, executable }) => {
      await packageBuild({ platform: "linux", executable, title: "Pong & Co" });
      const entry = await readFile(join(directory, "Pong Deluxe.desktop"), "utf8");
      // `\` is an escape character in a desktop entry, so a Windows path is
      // written doubled. The expectation reads the format rather than the host.
      expect(entry).toContain(`Exec="${executable.replaceAll("\\", "\\\\")}" %u\n`);
      expect(entry).toContain("Name=Pong & Co\n");
      expect(entry).not.toContain("Icon=");
    }, "Pong Deluxe");
  });

  // Issue #252. The identity a notification activation is addressed to, and the
  // name the platform's own notification service knows the entry point by.
  // Linux now uses the identity too: D-Bus activation requires the bus name,
  // desktop filename and service filename to be the same string.
  test("names the activation entry point each platform's notification service uses", () => {
    expect(activationEntryPoint({
      platform: "linux", identifier: "com.example.pong",
    })).toEqual({ identity: "com.example.pong", entry: "com.example.pong" });
    expect(activationEntryPoint({
      platform: "win32", identifier: "com.example.pong",
    })).toEqual({ identity: "com.example.pong", entry: "com.example.pong" });
    expect(activationEntryPoint({
      platform: "darwin", identifier: "com.example.pong",
    })).toEqual({ identity: "com.example.pong", entry: "com.example.pong" });
    // An identity nobody chose is one two applications could share, and
    // notification permission is granted per identity — so the `.app` bundle's
    // `com.blitsen.<title>` fallback deliberately does not become one here.
    expect(activationEntryPoint({
      platform: "darwin", identifier: null,
    })).toBeNull();
    expect(() => activationEntryPoint({
      platform: "linux", identifier: "not one name",
    })).toThrow("not a Linux D-Bus application name");
    expect(() => activationEntryPoint({
      platform: "win32", identifier: "not\\one\\identity",
    })).toThrow("not a Windows AppUserModelID");
    expect(notificationActivatorClsid("com.example.Pong"))
      .toBe("{022c05fc-9e96-52d4-89fa-e74df195db71}");
  });

  test("packages Linux D-Bus activation beside the desktop entry", async () => {
    await withArtifact(async ({ directory, executable }) => {
      const result = await packageBuild({
        platform: "linux", executable, title: "Pong", identifier: "com.example.Pong",
      });
      const desktop = join(directory, "com.example.Pong.desktop");
      const service = join(directory, "com.example.Pong.service");
      expect(result.artifacts).toEqual([desktop, service]);
      const desktopText = await readFile(desktop, "utf8");
      const desktopExecutable = /^Exec=(.+) %u$/m.exec(desktopText)?.[1];
      expect(desktopExecutable).toContain("Pong");
      expect(desktopText).toContain("\nDBusActivatable=true\n");
      expect(await readFile(service, "utf8")).toBe(
        `[D-BUS Service]\nName=com.example.Pong\nExec=${desktopExecutable}\n`);

      const backend = await readFile(join(import.meta.dir,
        "../../../crates/blitsen-host/src/native_window/notify/linux_portal.rs"), "utf8");
      expect(backend).toContain('"org.freedesktop.host.portal.Registry"');
      expect(backend).toContain('"org.freedesktop.Application"');
      expect(backend).toContain('const ACTION: &str = "app.blitsen-notification"');
      expect(backend).toContain("only the notification portal may activate");
      const session = await readFile(join(import.meta.dir,
        "../../../crates/blitsen-host/src/native_window/session.rs"), "utf8");
      expect(session).toContain("self.application.notify.detach()");
    });
  });

  // The other half of the same contract: what a platform entry point hands the
  // process it starts. Both hosts read it with this function — the Phase 1
  // launcher inlines it — so a launch that carries no envelope is an ordinary
  // launch on either.
  test("reads the activation envelope a platform entry point launched with", () => {
    const envelope = '{"nonce":"a1","identity":"com.example.pong","id":"n1",'
      + '"platform":"linux","entry":"Pong"}';
    expect(notificationActivation(["/opt/pong/Pong", "--notification-activation", envelope]))
      .toBe(envelope);
    expect(notificationActivation(["/opt/pong/Pong"])).toBeNull();
    // A field code the desktop substituted nothing for leaves the flag with no
    // value behind it, which is not an activation and must not be read as one.
    expect(notificationActivation(["/opt/pong/Pong", "--notification-activation"])).toBeNull();
  });

  // Issue #247. An application that opens a raw HID device needs something the
  // application itself cannot grant, so packaging writes the file a distributor
  // installs or signs with — and writes nothing at all when HID is not used.
  test("writes the udev rule and entitlements raw HID access needs", async () => {
    await withArtifact(async ({ directory, executable }) => {
      const linux = await packageBuild({ platform: "linux", executable, title: "Pong", hid: true });
      const rules = join(directory, "Pong.hid.rules");
      expect(linux.artifacts).toContain(rules);
      const rule = await readFile(rules, "utf8");
      expect(rule).toContain('SUBSYSTEM=="hidraw"');
      expect(rule).toContain('TAG+="uaccess"');
      // The template is inert until its IDs are replaced, and says so rather
      // than shipping a rule that matches a device nobody chose.
      expect(rule).toContain("Replace the vendor and product IDs");
      expect(linux.notes.join(" ")).toContain("udev rule");
    });
    await withArtifact(async ({ directory, executable }) => {
      const macos = await packageBuild({ platform: "darwin", executable, title: "Pong", hid: true });
      const entitlements = join(directory, "Pong.app.entitlements");
      expect(macos.artifacts).toContain(entitlements);
      expect(await readFile(entitlements, "utf8"))
        .toContain("<key>com.apple.security.device.usb</key>");
      expect(macos.notes.join(" ")).toContain("codesign --entitlements");
    });
    await withArtifact(async ({ directory, executable }) => {
      const linux = await packageBuild({ platform: "linux", executable, title: "Pong" });
      expect(linux.artifacts).not.toContain(join(directory, "Pong.hid.rules"));
      expect(await stat(join(directory, "Pong.hid.rules")).catch(() => null)).toBeNull();
    });
  });

  test("produces a macOS .app bundle with an Info.plist, PkgInfo and .icns", async () => {
    await withArtifact(async ({ directory, executable }) => {
      const sideLoaded = join(directory, "Pong.assets");
      await mkdir(sideLoaded);
      await writeFile(join(sideLoaded, "index.html"), "<!doctype html>");
      const result = await packageBuild({
        platform: "darwin", executable, title: "Pong Deluxe", icon,
        version: "1.2.3", assetDirectory: sideLoaded,
      });
      const bundle = join(directory, "Pong.app");
      expect(result).toMatchObject({
        bundle,
        executable: join(bundle, "Contents/MacOS/Pong"),
        assetDirectory: join(bundle, "Contents/MacOS/Pong.assets"),
        artifacts: [bundle],
        notes: [],
      });
      expect((await readdir(join(bundle, "Contents"))).sort())
        .toEqual(["Info.plist", "MacOS", "PkgInfo", "Resources"]);
      // Side-loaded assets resolve from the executable's directory, so they move
      // into the bundle with it.
      expect(await readdir(result.assetDirectory)).toEqual(["index.html"]);
      // A mode is POSIX: Windows reports none, and the bundle is being built
      // for macOS from whatever host happens to run the test.
      if (process.platform !== "win32") {
        expect((await stat(result.executable)).mode & 0o111).toBeGreaterThan(0);
      }
      expect(await readFile(join(bundle, "Contents/PkgInfo"), "utf8")).toBe("APPL????");

      const plist = await readFile(join(bundle, "Contents/Info.plist"), "utf8");
      expect(plist.startsWith('<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE plist PUBLIC')).toBeTrue();
      expect(plist).toContain("<key>CFBundleExecutable</key>\n  <string>Pong</string>");
      expect(plist).toContain("<key>CFBundleIdentifier</key>\n  <string>com.blitsen.pong-deluxe</string>");
      expect(plist).toContain("<key>CFBundleIconFile</key>\n  <string>Pong.icns</string>");
      expect(plist).toContain("<key>CFBundlePackageType</key>\n  <string>APPL</string>");
      expect(plist).toContain("<key>CFBundleShortVersionString</key>\n  <string>1.2.3</string>");
      expect(plist).toContain("<key>NSUserNotificationAlertStyle</key>\n  <string>alert</string>");
      expect(plist).toContain("<key>NSHighResolutionCapable</key>\n  <true/>");
      expect(plist.trimEnd().endsWith("</plist>")).toBeTrue();

      const png = await readFile(icon);
      const icns = await readFile(join(bundle, "Contents/Resources/Pong.icns"));
      expect(icns.subarray(0, 4).toString("ascii")).toBe("icns");
      expect(icns.readUInt32BE(4)).toBe(icns.length);
      expect(icns.subarray(8, 12).toString("ascii")).toBe("ic08");
      expect(icns.readUInt32BE(12)).toBe(png.length + 8);
      expect(Buffer.compare(icns.subarray(16), png)).toBe(0);
    });
  });

  // Issue #253. The bundle a development run re-executes into: the identity is
  // the whole point of it, so what is asserted is that it has one, that it is
  // Blitsen's own rather than any installed application's, and that it is not
  // the identity the same application would export under.
  test("gives the development host a signed bundle with an identity of its own", async () => {
    await withArtifact(async ({ directory, executable }) => {
      const home = join(directory, "cache");
      const made = await developmentBundle({
        directory: home, name: "Pong Deluxe",
        identifier: developmentIdentifier("Pong Deluxe"),
        launcher: executable, version: "1.2.3", sign: signHook,
      });
      const bundle = join(home, "pong-deluxe.app");
      expect(made).toEqual({
        bundle,
        executable: join(bundle, "Contents/MacOS/pong-deluxe"),
        identifier: "com.blitsen.dev.pong-deluxe",
        rebuilt: true,
      });
      // Never the exported application's identifier: notification permission is
      // recorded per identifier, and a development grant must not read as one
      // the shipped application was given.
      expect(made.identifier).not.toBe("com.blitsen.pong-deluxe");
      expect(developmentIdentifier("Terminal")).toBe("com.blitsen.dev.terminal");

      expect((await readdir(join(bundle, "Contents"))).sort())
        .toEqual(["Info.plist", "MacOS", "PkgInfo"]);
      const plist = await readFile(join(bundle, "Contents/Info.plist"), "utf8");
      expect(plist).toContain("<key>CFBundleIdentifier</key>\n  <string>com.blitsen.dev.pong-deluxe</string>");
      expect(plist).toContain("<key>CFBundleExecutable</key>\n  <string>pong-deluxe</string>");
      // The application still names itself; only the file had to be spellable.
      expect(plist).toContain("<key>CFBundleName</key>\n  <string>Pong Deluxe</string>");
      // The interpreter is copied, not linked: signing rewrites what it covers,
      // and the machine's own interpreter is not Blitsen's to rewrite.
      expect(await readFile(made.executable, "utf8")).toBe(await readFile(executable, "utf8"));
      if (process.platform !== "win32") {
        expect((await stat(made.executable)).mode & 0o111).toBeGreaterThan(0);
      }
      expect(await readFile(`${bundle}.signed`, "utf8")).toBe(`${bundle}\n`);

      // Reused until something about the identity changes, and rebuilt when it
      // does — a development run happens far more often than the interpreter
      // under it moves.
      await rm(`${bundle}.signed`);
      const again = await developmentBundle({
        directory: home, name: "Pong Deluxe",
        identifier: developmentIdentifier("Pong Deluxe"),
        launcher: executable, version: "1.2.3", sign: signHook,
      });
      expect(again.rebuilt).toBeFalse();
      expect(await stat(`${bundle}.signed`).catch(() => null)).toBeNull();
      const renamed = await developmentBundle({
        directory: home, name: "Pong Deluxe", identifier: "com.example.pong",
        launcher: executable, version: "1.2.3", sign: signHook,
      });
      expect(renamed.rebuilt).toBeTrue();
      expect(await readFile(join(bundle, "Contents/Info.plist"), "utf8"))
        .toContain("<string>com.example.pong</string>");
    });
  });

  // A stamp written before the signature would hand the next run a bundle macOS
  // refuses, and say nothing about why.
  test("leaves no development bundle behind when signing it fails", async () => {
    await withArtifact(async ({ directory, executable }) => {
      const home = join(directory, "cache");
      const build = () => developmentBundle({
        directory: home, name: "Pong", identifier: "com.example.pong",
        launcher: executable, version: "1.2.3", sign: "false",
      });
      await expect(build()).rejects.toThrow("signing command failed with exit code 1: false");
      expect(await stat(join(home, "pong.app.launcher")).catch(() => null)).toBeNull();
      await expect(build()).rejects.toThrow("signing command failed");
      await expect(developmentBundle({
        directory: home, name: "Pong", identifier: "com.example.pong",
        launcher: join(directory, "absent"), version: null, sign: signHook,
      })).rejects.toThrow("the interpreter to bundle does not exist:");
    });
  });

  test("writes a Windows application manifest and .ico beside the executable", async () => {
    await withArtifact(async ({ directory, executable }) => {
      const result = await packageBuild({
        platform: "win32", executable, title: "Pong Deluxe", icon, version: "1.2.3",
      });
      expect(result.artifacts)
        .toEqual([`${executable}.manifest`, join(directory, "Pong.ico")]);
      expect(result.executable).toBe(executable);
      expect(result.notes[0]).toContain("not embedded in the executable");

      const manifest = await readFile(`${executable}.manifest`, "utf8");
      expect(manifest).toContain('<assemblyIdentity type="win32" name="pong-deluxe" version="1.2.3.0"/>');
      expect(manifest).toContain("<description>Pong Deluxe</description>");
      expect(manifest).toContain('<requestedExecutionLevel level="asInvoker" uiAccess="false"/>');
      expect(manifest).toContain(">permonitorv2,permonitor</dpiAwareness>");
      expect(manifest).toContain(">UTF-8</activeCodePage>");
      expect(manifest.trimEnd().endsWith("</assembly>")).toBeTrue();

      const png = await readFile(icon);
      const ico = await readFile(join(directory, "Pong.ico"));
      expect([ico.readUInt16LE(0), ico.readUInt16LE(2), ico.readUInt16LE(4)]).toEqual([0, 1, 1]);
      // 256 pixels is stored as 0 in the single-byte width and height fields.
      expect([ico[6], ico[7], ico.readUInt16LE(12)]).toEqual([0, 0, 32]);
      expect(ico.readUInt32LE(14)).toBe(png.length);
      expect(ico.readUInt32LE(18)).toBe(22);
      expect(Buffer.compare(ico.subarray(22), png)).toBe(0);
    });
  });

  test("packages the registered Windows toast COM activator for an identified app", async () => {
    await withArtifact(async ({ executable }) => {
      const result = await packageBuild({
        platform: "win32", executable, title: "Pong Deluxe", identifier: "com.example.Pong",
      });
      const registration = `${executable}.notification-register.ps1`;
      expect(result.artifacts).toEqual([`${executable}.manifest`, registration]);
      expect(result.notes.join(" ")).toContain("per-user COM registration");
      const text = await readFile(registration, "utf8");
      const clsid = "{022c05fc-9e96-52d4-89fa-e74df195db71}";
      expect(text).toContain("HKCU:\\Software\\Classes\\AppUserModelId\\com.example.Pong");
      expect(text).toContain(`New-ItemProperty -Path $appId -Name 'CustomActivator' -Value '${clsid}'`);
      expect(text).toContain(`HKCU:\\Software\\Classes\\CLSID\\${clsid}\\LocalServer32`);
      expect(text).toContain("--notification-com-server");
      expect(text).toContain(`Join-Path $PSScriptRoot '${basename(executable)}'`);
    });
  });

  test("refuses icons and hosts it cannot package, and existing artifacts", async () => {
    await withArtifact(async ({ directory, executable }) => {
      await expect(packageBuild({ platform: "sunos", executable, title: "Pong" }))
        .rejects.toThrow("packaging is not supported on sunos");
      await expect(packageBuild({ platform: "linux", executable, title: "Pong", icon: "app.icns" }))
        .rejects.toThrow("linux icons must be .png or .svg");
      await expect(packageBuild({
        platform: "darwin", executable, title: "Pong",
        icon: join(import.meta.dir, "fixtures/icons/app-16.png"),
      })).rejects.toThrow("macOS icons need a PNG of 128, 256, 512, 1024 pixels");
      await expect(packageBuild({ platform: "linux", executable, title: "Pong", icon: "missing.png" }))
        .rejects.toThrow("icon file does not exist: missing.png");
      // A refused icon is refused before anything is written.
      expect(await readdir(directory)).toEqual(["Pong"]);

      await packageBuild({ platform: "linux", executable, title: "Pong", icon });
      await expect(packageBuild({ platform: "linux", executable, title: "Pong", icon }))
        .rejects.toThrow(`output already exists: ${join(directory, "Pong.desktop")}`);
      await packageBuild({ platform: "linux", executable, title: "Pong", icon, force: true });
    });
  });

  test("hands the signing hook the artifact and fails the build when it rejects it", async () => {
    // The interpreter is this machine's: the hook runs here, whatever platform
    // the artifact is for.
    expect(signArgv("codesign -s ID", "/out/My App.app")).toEqual(process.platform === "win32"
      ? ["cmd", "/c", 'codesign -s ID "/out/My App.app"']
      : ["sh", "-c", 'codesign -s ID "$@"', "sh", "/out/My App.app"]);
    await withArtifact(async ({ executable }) => {
      // The hook is a command, not a shell fragment we interpolate into: the
      // artifact arrives as one positional argument however it is spelled.
      const bundle = `${executable} Deluxe.app`;
      await writeFile(bundle, "");
      expect(await signArtifact({ command: signHook, artifact: bundle }))
        .toEqual({ command: signHook, artifact: bundle });
      expect(await readFile(`${bundle}.signed`, "utf8")).toBe(`${bundle}\n`);
      await expect(signArtifact({ command: "false", artifact: executable }))
        .rejects.toThrow("signing command failed with exit code 1: false");
    });
  });

});
