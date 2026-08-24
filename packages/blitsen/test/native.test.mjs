import { afterEach, describe, expect, test } from "bun:test";
import { NATIVE_MODULES, nativeModule } from "../src/native/module.mjs";
import { blitsenEsbuild, blitsenRollup, blitsenWebpackExternals } from "../src/bundler.mjs";

const RUNTIME = Symbol.for("blitsen.native");
const install = surface => { globalThis[RUNTIME] = surface; };
afterEach(() => { delete globalThis[RUNTIME]; });

describe("native module namespaces", () => {
  test("throw outside the runtime, naming what was reached for", () => {
    const dialog = nativeModule("dialog");
    expect(() => dialog.openFile).toThrow(/blitsen\/dialog requires the Blitsen runtime/);
    expect(() => dialog.openFile).toThrow(/"openFile" was accessed/);
  });

  test("report unimplemented capability as absent, so feature detection works", () => {
    install({ dialog: { openFile: () => "chosen" } });
    const dialog = nativeModule("dialog");
    expect(dialog.openFile()).toBe("chosen");
    // The whole point of principle 4: a capability this version lacks is undefined,
    // not a function that throws when called.
    expect(dialog.saveFile).toBeUndefined();
    expect("saveFile" in dialog).toBe(false);
    expect("openFile" in dialog).toBe(true);
  });

  test("a module the runtime installed nothing for is empty, not an error", () => {
    install({});
    expect(nativeModule("tray").setIcon).toBeUndefined();
    expect(Object.keys(nativeModule("tray"))).toEqual([]);
  });

  test("enumerate what the runtime actually installed", () => {
    install({ os: { locale: "en-GB", idleTime: () => 0 } });
    expect(Object.keys(nativeModule("os")).sort()).toEqual(["idleTime", "locale"]);
  });

  test("are read-only", () => {
    install({ app: {} });
    expect(() => { nativeModule("app").quit = 1; }).toThrow(/read-only/);
  });

  test("answer await and toString without pretending to be in the runtime", () => {
    const app = nativeModule("app");
    expect(app.then).toBeUndefined();
    expect(Object.prototype.toString.call(app)).toBe("[object BlitsenNative(app)]");
  });
});

describe("bundler compatibility helpers", () => {
  test("reject known native specifiers with the package subpath that replaces them", () => {
    const plugin = blitsenRollup();
    plugin.error = message => { throw new Error(message); };
    expect(() => plugin.resolveId("native:dialog")).toThrow(/import "blitsen\/dialog" instead/);
    expect(blitsenRollup().resolveId("./local.js")).toBeNull();

    const errors = [];
    blitsenEsbuild().setup({
      onResolve: (_filter, callback) => errors.push(callback({ path: "native:window" })),
    });
    expect(errors[0].errors[0].text).toMatch(/import "blitsen\/window" instead/);
  });

  test("reject an unknown native specifier", () => {
    const errors = [];
    const plugin = blitsenRollup();
    plugin.error = message => { throw new Error(message); };
    expect(() => plugin.resolveId("native:sqlite")).toThrow(/unknown native module/);

    blitsenEsbuild().setup({
      onResolve: (_filter, callback) => errors.push(callback({ path: "native:fs" })),
    });
    expect(errors[0].errors[0].text).toMatch(/unknown native module "native:fs"/);
  });

  test("explain that native modules have no subpaths", () => {
    const plugin = blitsenRollup();
    plugin.error = message => { throw new Error(message); };
    expect(() => plugin.resolveId("native:window/extra")).toThrow(/no subpaths/);
  });

  test("webpack refuses native specifiers and passes through anything else", () => {
    const externals = blitsenWebpackExternals();
    let error;
    let result;
    externals({ request: "native:window" }, (received, value) => {
      error = received;
      result = value;
    });
    expect(error.message).toMatch(/import "blitsen\/window" instead/);
    expect(result).toBeUndefined();
    externals({ request: "react" }, (received, value) => {
      error = received;
      result = value;
    });
    expect(error).toBeUndefined();
    expect(result).toBeUndefined();
  });
});

test("every declared module has a subpath export", async () => {
  const manifest = await Bun.file(new URL("../package.json", import.meta.url)).json();
  for (const name of NATIVE_MODULES) {
    expect(manifest.exports[`./${name}`]).toBeDefined();
  }
});
