import { describe, expect, test } from "bun:test";
import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, join } from "node:path";
import { promisify } from "node:util";

import { buildPayload, buildTrailer, linkBundle, readBundle, FORMAT_VERSION } from "../src/bundle.mjs";
import { injectMachOPayload, machOPayloadOffset } from "../src/macho.mjs";
import { buildStandalone } from "../src/export.mjs";
import { compileAddon, compiler, exportedName, withStubbedExport } from "./cli-support.mjs";

const run = promisify(execFile);
const REPO = new URL("../../../", import.meta.url).pathname;

function machoFixture(cpu) {
  const page = cpu === 0x0100000c ? 0x4000 : 0x1000;
  const linkeditAt = page;
  const linkeditBytes = 64;
  const inheritedSignatureBytes = 256;
  const signatureAt = linkeditAt + linkeditBytes;
  const commands = [];
  const segment = ({ name, vmaddr, vmsize, fileoff, filesize }) => {
    const command = Buffer.alloc(72);
    command.writeUInt32LE(0x19, 0);
    command.writeUInt32LE(72, 4);
    command.write(name, 8, 16, "ascii");
    command.writeBigUInt64LE(BigInt(vmaddr), 24);
    command.writeBigUInt64LE(BigInt(vmsize), 32);
    command.writeBigUInt64LE(BigInt(fileoff), 40);
    command.writeBigUInt64LE(BigInt(filesize), 48);
    command.writeUInt32LE(7, 56);
    command.writeUInt32LE(name === "__TEXT" ? 5 : 1, 60);
    return command;
  };
  commands.push(segment({
    name: "__TEXT", vmaddr: 0x100000000, vmsize: page,
    fileoff: 0, filesize: page,
  }));
  const symtab = Buffer.alloc(24);
  symtab.writeUInt32LE(0x2, 0);
  symtab.writeUInt32LE(24, 4);
  symtab.writeUInt32LE(linkeditAt, 8);
  symtab.writeUInt32LE(signatureAt - 32, 16);
  symtab.writeUInt32LE(32, 20);
  commands.push(symtab);
  const signature = Buffer.alloc(16);
  signature.writeUInt32LE(0x1d, 0);
  signature.writeUInt32LE(16, 4);
  signature.writeUInt32LE(signatureAt, 8);
  signature.writeUInt32LE(inheritedSignatureBytes, 12);
  commands.push(signature);
  commands.push(segment({
    name: "__LINKEDIT", vmaddr: 0x100000000 + page, vmsize: page,
    fileoff: linkeditAt, filesize: linkeditBytes + inheritedSignatureBytes,
  }));
  const commandBytes = Buffer.concat(commands);
  const executable = Buffer.alloc(signatureAt + inheritedSignatureBytes);
  executable.writeUInt32LE(0xfeedfacf, 0);
  executable.writeInt32LE(cpu, 4);
  executable.writeUInt32LE(3, 8);
  executable.writeUInt32LE(2, 12);
  executable.writeUInt32LE(commands.length, 16);
  executable.writeUInt32LE(commandBytes.length, 20);
  executable.writeUInt32LE(0x200085, 24);
  commandBytes.copy(executable, 32);
  executable.fill(0x5a, linkeditAt, signatureAt);
  executable.fill(0xa5, signatureAt);
  return { executable, linkeditAt, page };
}

function machoCommands(executable) {
  const commands = [];
  let offset = 32;
  for (let index = 0; index < executable.readUInt32LE(16); index += 1) {
    const type = executable.readUInt32LE(offset);
    const size = executable.readUInt32LE(offset + 4);
    const name = type === 0x19
      ? executable.subarray(offset + 8, offset + 24).toString("ascii").replace(/\0.*$/, "")
      : null;
    commands.push({ type, size, offset, name });
    offset += size;
  }
  return commands;
}

function expectAdHocSignature(executable, command) {
  const signatureAt = executable.readUInt32LE(command.offset + 8);
  const signatureSize = executable.readUInt32LE(command.offset + 12);
  expect(signatureAt + signatureSize).toBe(executable.length);
  expect(executable.readUInt32BE(signatureAt)).toBe(0xfade0cc0);
  expect(executable.readUInt32BE(signatureAt + 12)).toBe(0);
  const directory = signatureAt + executable.readUInt32BE(signatureAt + 16);
  expect(executable.readUInt32BE(directory)).toBe(0xfade0c02);
  expect(executable.readUInt32BE(directory + 12) & 2).toBe(2);
  const hashes = directory + executable.readUInt32BE(directory + 16);
  const slots = executable.readUInt32BE(directory + 28);
  const codeLimit = executable.readUInt32BE(directory + 32);
  expect(codeLimit).toBe(signatureAt);
  for (let slot = 0; slot < slots; slot += 1) {
    const page = executable.subarray(slot * 4096, Math.min((slot + 1) * 4096, codeLimit));
    const expected = createHash("sha256").update(page).digest();
    expect(executable.subarray(hashes + slot * 32, hashes + (slot + 1) * 32)).toEqual(expected);
  }
}

// Issue #88: the format has two implementations — this package writes it, and
// the shipped runtime reads it. Nothing keeps them honest except a test that
// exercises both against the same bytes, so that is what these are.
describe("Phase 2 link step", () => {
  const application = new Map([
    ["index.html", Buffer.from("<!doctype html><body><main id=x>waiting</main>")],
    ["assets/app.js", Buffer.from("document.querySelector('#x').textContent = 'linked'")],
    ["assets/logo.png", Buffer.from([0x89, 0x50, 0x4e, 0x47])],
  ]);

  async function link(files = application) {
    const directory = await mkdtemp(join(tmpdir(), "blitsen-bundle-"));
    const runtime = join(directory, "runtime");
    // Not the real runtime: what is under test is the container, and a stub
    // keeps the test independent of whether Rust has been built.
    await writeFile(runtime, Buffer.alloc(4096, 0x7f));
    const output = join(directory, "MyApp");
    const report = await linkBundle({ runtime, output, files });
    return { directory, output, report };
  }

  test("writes a payload the reader accepts, and reads it back unchanged", async () => {
    const { directory, output, report } = await link();
    try {
      expect(report.files).toBe(3);
      expect(report.totalBytes).toBe((await readFile(output)).length);

      const bundle = readBundle(await readFile(output));
      expect(bundle.version).toBe(FORMAT_VERSION);
      expect(bundle.verified).toBe(true);
      expect(bundle.digest).toBe(report.digest);
      expect([...bundle.files.keys()]).toEqual(["assets/app.js", "assets/logo.png", "index.html"]);
      expect(bundle.files.get("index.html").toString()).toBe(
        "<!doctype html><body><main id=x>waiting</main>");
      expect([...bundle.files.get("assets/logo.png")]).toEqual([0x89, 0x50, 0x4e, 0x47]);
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  test("links the same input to the same bytes", async () => {
    const first = await link();
    const second = await link();
    try {
      expect(first.report.digest).toBe(second.report.digest);
      expect(await readFile(first.output)).toEqual(await readFile(second.output));
    } finally {
      await rm(first.directory, { recursive: true, force: true });
      await rm(second.directory, { recursive: true, force: true });
    }
  });

  test("survives a code signature appended after the trailer", async () => {
    const { directory, output } = await link();
    try {
      const signed = Buffer.concat([await readFile(output), Buffer.alloc(9000, 0xa5)]);
      const bundle = readBundle(signed);
      expect(bundle.verified).toBe(true);
      expect(bundle.files.size).toBe(3);
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  test("puts a Darwin payload in a real segment before __LINKEDIT and ad-hoc signs it", () => {
    for (const cpu of [0x01000007, 0x0100000c]) {
      const { executable, linkeditAt, page } = machoFixture(cpu);
      const payload = Buffer.from("a payload that belongs to __BLITSEN");
      const trailer = Buffer.alloc(64, 0xa5);
      const section = Buffer.concat([payload, trailer]);
      expect(machOPayloadOffset(executable)).toBe(linkeditAt);
      const linked = injectMachOPayload(executable, section);
      const commands = machoCommands(linked);
      const embedded = commands.find(command => command.name === "__BLITSEN");
      const linkedit = commands.find(command => command.name === "__LINKEDIT");
      const symtab = commands.find(command => command.type === 0x2);
      const signature = commands.find(command => command.type === 0x1d);

      expect(linked.readBigUInt64LE(embedded.offset + 40)).toBe(BigInt(linkeditAt));
      expect(linked.readBigUInt64LE(embedded.offset + 48)).toBe(BigInt(page));
      expect(linked.readUInt32LE(embedded.offset + 64)).toBe(0);
      expect(linked.subarray(linkeditAt, linkeditAt + payload.length)).toEqual(payload);
      expect(linked.subarray(linkeditAt + page - trailer.length, linkeditAt + page)).toEqual(trailer);
      expect(linked.readBigUInt64LE(linkedit.offset + 40)).toBe(BigInt(linkeditAt + page));
      expect(Number(linked.readBigUInt64LE(linkedit.offset + 40)
        + linked.readBigUInt64LE(linkedit.offset + 48))).toBe(linked.length);
      expect(linked.readUInt32LE(symtab.offset + 8)).toBe(linkeditAt + page);
      expectAdHocSignature(linked, signature);
    }
  });

  test("a Mach-O link is read through the same payload and trailer contract", async () => {
    for (const cpu of [0x01000007, 0x0100000c]) {
      const directory = await mkdtemp(join(tmpdir(), "blitsen-bundle-macho-"));
      try {
        const runtime = join(directory, "runtime");
        const output = join(directory, "MyApp");
        await writeFile(runtime, machoFixture(cpu).executable);
        const report = await linkBundle({ runtime, output, files: application });
        const bytes = await readFile(output);
        const bundle = readBundle(bytes);
        expect(bundle.verified).toBe(true);
        expect(bundle.digest).toBe(report.digest);
        expect(bundle.offset).toBe(machoFixture(cpu).linkeditAt);
        expect(bundle.files.get("index.html").toString()).toContain("waiting");
        expect(report.totalBytes).toBe(bytes.length);
      } finally {
        await rm(directory, { recursive: true, force: true });
      }
    }
  });

  test("refuses a path that would escape the application", async () => {
    for (const escape of ["../secret", "/etc/passwd", "a/../../b", "", "a//b"]) {
      expect(() => buildPayload(new Map([[escape, Buffer.from("x")]]))).toThrow();
    }
  });

  test("is not fooled by the magic appearing inside the runtime", async () => {
    const directory = await mkdtemp(join(tmpdir(), "blitsen-bundle-decoy-"));
    try {
      const runtime = join(directory, "runtime");
      await writeFile(runtime, Buffer.concat([
        Buffer.alloc(2048, 0x7f),
        Buffer.from("BLITSEN\x1a", "latin1"),
        Buffer.from("BLITSEN\0", "latin1"),
        Buffer.alloc(2048, 0x7f),
      ]));
      expect(readBundle(await readFile(runtime))).toBe(null);
      const output = join(directory, "MyApp");
      await linkBundle({ runtime, output, files: application });
      expect(readBundle(await readFile(output)).files.size).toBe(3);
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  // The one that matters: the Rust reader is what a shipped application uses,
  // so agreement with it is the whole point of the format. Skipped rather than
  // failed when the runtime has not been built, so this file still runs in the
  // JavaScript-only CI job.
  test("the Rust runtime reads what this package writes", async () => {
    const runtime = join(REPO, "target/debug/blitsen-runtime");
    if (!(await Bun.file(runtime).exists())) return;
    const directory = await mkdtemp(join(tmpdir(), "blitsen-bundle-rust-"));
    try {
      const output = join(directory, "MyApp");
      const report = await linkBundle({ runtime, output, files: application });
      const { stdout } = await run(output, ["--bundle-report"]);
      const runtimeReport = JSON.parse(stdout);
      expect(runtimeReport.bundled).toBe(true);
      expect(runtimeReport.verified).toBe(true);
      expect(runtimeReport.formatVersion).toBe(FORMAT_VERSION);
      expect(runtimeReport.digest).toBe(report.digest);
      expect(runtimeReport.payloadBytes).toBe(report.payloadBytes);
      expect(runtimeReport.files.map(file => file.path))
        .toEqual(["assets/app.js", "assets/logo.png", "index.html"]);
      expect(runtimeReport.files.find(file => file.path === "assets/logo.png").bytes).toBe(4);
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  // Which host an export links into is a size decision everywhere except where
  // it is a capability one, and that case is worth holding down: an export that
  // took the small host and then could not load the addon it carries would be a
  // smaller application that does not run.
  //
  // Both applications here are written out rather than taken from a fixture,
  // because what is under test is exactly the difference between them.
  const CLASSIC_APP = "<!doctype html><html><body><script>document.title='ok'</script></body></html>";
  const MODULE_APP = '<!doctype html><html><body><script type="module" src="./app.js"></script></body></html>';

  async function staticApp(directory, html, extra = {}) {
    const root = join(directory, "dist");
    await mkdir(root, { recursive: true });
    await writeFile(join(root, "index.html"), html);
    for (const [name, contents] of Object.entries(extra)) {
      await writeFile(join(root, name), contents);
    }
    return root;
  }

  test("links the small host for an application any engine can run", async () => {
    await withStubbedExport(async ({ directory, outfile, nativePath }) => {
      const trayIcon = Buffer.from(
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAFgQIAffRr7QAAAABJRU5ErkJggg==",
        "base64",
      );
      const root = await staticApp(directory, CLASSIC_APP, {
        "tray.png": trayIcon, "open.png": trayIcon, "theme.png": trayIcon,
      });
      const window = { type: "borderless", resizable: false, alwaysOnTop: true };
      const tray = {
        icon: join(root, "tray.png"), tooltip: "Classic", openOnClick: true,
        contextMenu: [
          { id: "open", label: "Open", iconIndex: 0 },
          { type: "submenu", label: "Theme", iconIndex: 1, menu: [
            { type: "radio", id: "light", label: "Light", group: "theme", checked: true },
            { type: "radio", id: "dark", label: "Dark", group: "theme" },
          ] },
          { action: "quit" },
        ],
        menuIcons: [join(root, "open.png"), join(root, "theme.png")],
      };
      // The application menu carries no assets, so it travels as the tree the
      // configuration declared rather than as recorded bundle names.
      const menu = {
        menu: [
          { type: "submenu", role: "application", label: "Classic", menu: [
            { type: "role", role: "about" }, { type: "role", role: "quit" },
          ] },
          { type: "submenu", label: "File", menu: [{ id: "new", label: "New" }] },
        ],
      };
      const built = await buildStandalone(
        { root, width: 800, height: 600, title: "Classic", outfile, window, tray, menu },
        nativePath);
      expect(built.host).toBe("blitsen");
      // Linked by appending to the runtime, so the artifact carries the bundle.
      const bundle = readBundle(await readFile(built.outfile));
      expect(bundle).not.toBeNull();
      expect(bundle.files.get("blitsen.tray.png")).toEqual(trayIcon);
      expect(bundle.files.get("blitsen.tray-menu.0.png")).toEqual(trayIcon);
      expect(bundle.files.get("blitsen.tray-menu.1.png")).toEqual(trayIcon);
      const runtime = JSON.parse(bundle.files.get("blitsen.runtime.json").toString("utf8"));
      expect(runtime.window).toEqual(window);
      expect(runtime.tray).toEqual({
        ...tray,
        icon: "blitsen.tray.png",
        menuIcons: ["blitsen.tray-menu.0.png", "blitsen.tray-menu.1.png"],
      });
      expect(runtime.menu).toEqual(menu);
      // No `--bundle-id`, so nothing registered an identity for this artifact
      // and there is none for a notification activation to be addressed to.
      expect(runtime.activation).toBeNull();
      const side = await buildStandalone({
        root, width: 800, height: 600, title: "Classic", outfile: join(directory, "Side"),
        window, tray, assets: "side-loaded",
      }, nativePath);
      expect(await readFile(join(side.assetDirectory, "blitsen.tray.png"))).toEqual(trayIcon);
      expect(await readFile(join(side.assetDirectory, "blitsen.tray-menu.0.png"))).toEqual(trayIcon);
      expect(await readFile(join(side.assetDirectory, "blitsen.tray-menu.1.png"))).toEqual(trayIcon);

      // Issue #252. The identity a notification activation is addressed to has
      // to be inside the executable the platform starts, because the process it
      // starts has no other way to learn it: the runtime is a generic host and
      // the artifact's path is not an identity.
      const identified = await buildStandalone({
        root, width: 800, height: 600, title: "Classic", outfile: join(directory, "Identified"),
        bundleId: "com.example.classic", platform: "linux",
      }, nativePath);
      const record = readBundle(await readFile(identified.outfile));
      expect(JSON.parse(record.files.get("blitsen.runtime.json").toString("utf8")).activation)
        .toEqual({ identity: "com.example.classic", entry: basename(identified.outfile) });
    });
  }, 120_000);

  test("rejects missing and non-PNG nested tray assets before linking", async () => {
    await withStubbedExport(async ({ directory, outfile, nativePath }) => {
      const png = Buffer.from(
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAFgQIAffRr7QAAAABJRU5ErkJggg==",
        "base64",
      );
      const root = await staticApp(directory, CLASSIC_APP, { "tray.png": png, "bad.png": "not png" });
      const base = {
        root, width: 800, height: 600, title: "Tray", outfile,
        tray: {
          icon: join(root, "tray.png"), contextMenu: [{ id: "open", label: "Open", iconIndex: 0 }],
          menuIcons: [join(root, "missing.png")],
        },
      };
      await expect(buildStandalone(base, nativePath)).rejects.toThrow("tray menu icon 1 does not exist");
      base.tray.menuIcons = [join(root, "bad.png")];
      await expect(buildStandalone(base, nativePath)).rejects.toThrow("icon is not a PNG file");
    });
  });

  test("cleans deterministic staging when linking fails after collection", async () => {
    await withStubbedExport(async ({ directory, outfile, nativePath, runtimePath }) => {
      const root = await staticApp(directory, CLASSIC_APP);
      await writeFile(runtimePath, "not an executable\n");
      const events = [];
      await expect(buildStandalone({
        root, width: 800, height: 600, title: "Broken", outfile,
        progress: event => events.push(event),
      }, nativePath)).rejects.toThrow("BLITSEN_RUNTIME_PATH does not name a supported executable");

      const destination = exportedName(outfile);
      const staging = join(directory, `.${basename(destination)}.blitsen-build`);
      expect(events.map(event => event.step)).toEqual(["collect"]);
      expect(await stat(staging).catch(() => null)).toBeNull();
      expect(await stat(destination).catch(() => null)).toBeNull();
    });
  });

  // The case that decides what most users get. A module application used to be
  // able to force the Bun host, back when the Phase 2 runtime loaded
  // JavaScriptCore at run time and the library it found might have no module
  // entry point. The shipped runtime links QuickJS-ng statically and its module
  // loader is stock, so module scripts no longer change the answer — on any
  // target, including the cross-target builds nothing here can run.
  test("links the small host for a module application on the shipping engine", async () => {
    await withStubbedExport(async ({ directory, outfile, nativePath }) => {
      const root = await staticApp(directory, MODULE_APP, { "app.js": "export const x = 1;\n" });
      const built = await buildStandalone(
        { root, width: 800, height: 600, title: "Module", outfile }, nativePath);
      expect(built.host).toBe("blitsen");
      expect(readBundle(await readFile(built.outfile))).not.toBeNull();
    });
  }, 120_000);

  test.skipIf(!compiler)("links Bun for an application carrying a Node-API addon", async () => {
    await withStubbedExport(async ({ directory, outfile, nativePath }) => {
      const png = Buffer.from(
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAFgQIAffRr7QAAAABJRU5ErkJggg==",
        "base64",
      );
      const root = await staticApp(directory, CLASSIC_APP, { "tray.png": png, "item.png": png });
      const addon = compileAddon(directory);
      const events = [];
      const built = await buildStandalone({
        root, width: 800, height: 600, title: "Addon", outfile, addons: [addon],
        tray: {
          icon: join(root, "tray.png"),
          contextMenu: [{ id: "open", label: "Open", iconIndex: 0 }],
          menuIcons: [join(root, "item.png")],
        },
        progress: event => events.push(event),
      }, nativePath);
      expect(built.host).toBe("bun");
      expect(built.addons).toEqual(["greet.node"]);
      expect(built.manifest.map(file => file.path)).toContain("blitsen.tray-menu.0.png");
      expect(events.find(event => event.step === "collect").notes.join("\n"))
        .toContain("95 MB larger");
    });
  }, 120_000);

  test.skipIf(!compiler)("refuses a host that cannot load the addon it was asked to carry", async () => {
    await withStubbedExport(async ({ directory, nativePath, outfile }) => {
      const root = await staticApp(directory, CLASSIC_APP);
      const addon = compileAddon(directory);
      const previous = process.env.BLITSEN_HOST;
      process.env.BLITSEN_HOST = "blitsen";
      try {
        await expect(buildStandalone(
          { root, width: 800, height: 600, title: "Base", outfile, addons: [addon] }, nativePath))
          .rejects.toThrow("BLITSEN_HOST=blitsen cannot load a carried native addon");
      } finally {
        if (previous === undefined) delete process.env.BLITSEN_HOST;
        else process.env.BLITSEN_HOST = previous;
      }
    });
  }, 120_000);

  test("a trailer is exactly the bytes the format specifies", () => {
    const payload = buildPayload(new Map([["a.js", Buffer.from("x")]]));
    const trailer = buildTrailer(payload, 100);
    expect(trailer.length).toBe(64);
    expect(Number(trailer.readBigUInt64LE(32))).toBe(100);
    expect(Number(trailer.readBigUInt64LE(40))).toBe(payload.length);
    expect(trailer.readUInt32LE(48)).toBe(FORMAT_VERSION);
    expect(trailer.subarray(56).toString("latin1")).toBe("BLITSEN\x1a");
  });
});
