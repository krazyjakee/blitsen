// The published type definitions, checked against the runtime (issue #74).
//
// The failure worth preventing is editor completion offering a native method
// that no runtime installs: the code compiles, and the call returns `undefined`
// at run time. So these assert the check itself catches drift in both
// directions, rather than only that today's definitions happen to pass.
import { describe, expect, test } from "bun:test";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { checkTypeDefinitions, generateApiManifest, readDeclaredNativeMembers }
  from "../src/api-manifest.mjs";

const DEFINITIONS = join(import.meta.dir, "../src/native/native.d.ts");
const definitions = await readFile(DEFINITIONS, "utf8");
const manifest = await generateApiManifest();

describe("published types", () => {
  test("declare exactly the native members the runtime installs", () => {
    const checked = checkTypeDefinitions(manifest, definitions);
    // Not just "did not throw": a reader that stopped matching would find no
    // members and pass silently, which is the failure this number rules out.
    const implemented = manifest.native.filter(entry => entry.status === "implemented").length;
    expect(checked).toBe(implemented);
    expect(checked).toBeGreaterThan(20);
  });

  test("read each module's members off its own interface", () => {
    const declared = readDeclaredNativeMembers(definitions);
    expect([...declared.keys()].sort()).toEqual([
      "app", "clipboard", "dialog", "hid", "input", "menu", "notify", "os", "tray", "window",
    ]);
    // Each module's members are its own: the clipboard's do not leak into the
    // app's, which is what the per-subpath declaration files are for.
    expect(declared.get("clipboard").has("readText")).toBeTrue();
    expect(declared.get("app").has("readText")).toBeFalse();
    expect(declared.get("app").has("dataDir")).toBeTrue();
    // An inline object type inside a signature is not a member of the interface.
    expect(declared.get("clipboard").has("width")).toBeFalse();
  });

  test("refuse a declared member the runtime does not install", () => {
    const promised = definitions.replace("export interface NativeApp {",
      "export interface NativeApp {\n  teleport?(): void;");
    expect(() => checkTypeDefinitions(manifest, promised))
      .toThrow(/blitsen\/app declares teleport, which the runtime does not install/);
  });

  test("refuse an installed member the definitions do not declare", () => {
    const dropped = definitions.replace(/^ {2}relaunch\?\(\): void;$/m, "");
    expect(dropped).not.toBe(definitions);
    expect(() => checkTypeDefinitions(manifest, dropped))
      .toThrow(/blitsen\/app installs relaunch, which native\.d\.ts does not declare/);
  });

  test("refuse a module that installs members with no interface to declare them", () => {
    const extended = {
      ...manifest,
      native: [...manifest.native, { api: "media.play", module: "media", member: "play",
        status: "implemented" }],
    };
    expect(() => checkTypeDefinitions(extended, definitions))
      .toThrow(/blitsen\/media installs play and has no declared interface/);
  });

  test("say so when the reader can no longer find an interface", () => {
    expect(() => readDeclaredNativeMembers(definitions.replace("export interface NativeWindow {", "")))
      .toThrow(/no longer declares NativeWindow/);
  });
});
