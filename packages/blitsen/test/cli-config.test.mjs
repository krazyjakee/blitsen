import { describe, expect, test } from "bun:test";
import { cp, mkdtemp, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { main } from "../src/cli.mjs";
import { CONFIG_SCHEMA, defineConfig, loadConfig, runBuildCommand, validateConfig } from "../src/config.mjs";
import { configFixtures, capture } from "./cli-support.mjs";

// macOS's temporary directory is a symlink — `/var` is `/private/var` — and
// every path the CLI reports has been through `process.cwd()` or `realpath`.
// A test that keeps `mkdtemp`'s spelling compares two names for one directory,
// which is a pass on Linux and Windows and a failure on macOS (#134).
const temporaryDirectory = async prefix => realpath(await mkdtemp(join(tmpdir(), prefix)));

describe("directory CLI", () => {
  test("publishes the schema it validates against", async () => {
    const published = join(import.meta.dir, "../src/config.schema.json");
    expect(JSON.parse(await readFile(published, "utf8"))).toEqual(CONFIG_SCHEMA);
    const config = {
      build: "vite build", output: "dist", name: "My App",
      window: { type: "borderless", resizable: false, transparent: true, alwaysOnTop: true },
      tray: {
        icon: "native/tray.png", tooltip: "My App", openOnClick: true, closeToTray: true,
        contextMenu: [
          { label: "Open", action: "show" }, { action: "separator" },
          { label: "Quit", action: "quit", enabled: true },
        ],
      },
    };
    expect(defineConfig(config)).toEqual(config);
  });

  test("rejects a malformed config naming the key and the file it came from", async () => {
    expect(() => defineConfig({ output: 7 }))
      .toThrow('invalid blitsen config in defineConfig(): "output" must be a string, found a number');
    expect(() => validateConfig({ output: "dist", name: " " }, "/app/package.json"))
      .toThrow('invalid blitsen config in /app/package.json: "name" must not be empty');
    expect(() => validateConfig({}, "/app/package.json"))
      .toThrow('invalid blitsen config in /app/package.json: missing required key "output"');
    expect(() => validateConfig(["dist"], "/app/package.json"))
      .toThrow("invalid blitsen config in /app/package.json: expected an object, found an array");
    expect(() => defineConfig({ output: "dist", addons: "physics.node" }))
      .toThrow('invalid blitsen config in defineConfig(): "addons" must be an array, found a string');
    expect(() => defineConfig({ output: "dist", addons: ["a.node", 7] }))
      .toThrow('invalid blitsen config in defineConfig(): "addons[1]" must be a string, found a number');
    expect(defineConfig({ output: "dist", addons: ["native/physics.node"] }).addons)
      .toEqual(["native/physics.node"]);
    expect(() => defineConfig({ output: "dist", window: { type: "frameless" } }))
      .toThrow('"window.type" must be one of normal, borderless, fullscreen, hidden');
    expect(() => defineConfig({ output: "dist", tray: { icon: "tray.ico" } }))
      .toThrow('"tray.icon" must match \\.png$');
    expect(() => defineConfig({ output: "dist", tray: { icon: "tray.png", nope: true } }))
      .toThrow('"tray.nope" is unknown');
    expect(() => defineConfig({ output: "dist", window: { type: "hidden" } }))
      .toThrow('"window.type" hidden requires a "tray" configuration');
    expect(() => defineConfig({ output: "dist", tray: { icon: "tray.png", closeToTray: true } }))
      .toThrow('"tray.closeToTray" requires a quit action');
    const misspelled = join(configFixtures, "misspelled");
    await expect(loadConfig(misspelled)).rejects.toThrow(
      `invalid blitsen config in ${join(misspelled, "package.json")}: `
      + 'unknown key "outputs" (known keys: build, output, name, addons, window, tray)');
  });

  test("discovers the config in the nearest package.json declaring it", async () => {
    const found = await loadConfig(join(configFixtures, "wrapped"));
    expect(found.root).toBe(join(configFixtures, "wrapped"));
    expect(found.config).toEqual({ build: "node emit-dist.mjs", output: "dist",
      name: "Wrapped App", addons: ["native/greet.node"],
      window: { type: "borderless", resizable: false },
      tray: {
        icon: "native/tray.png", closeToTray: true,
        contextMenu: [{ action: "show" }, { action: "quit" }],
      } });
    // A package.json without the key is not a config, and neither is no package.json.
    const bare = await mkdtemp(join(tmpdir(), "blitsen-config-"));
    try {
      expect(await loadConfig(bare)).toEqual({ path: null, root: null, config: null });
      await writeFile(join(bare, "package.json"), '{"name":"bare"}');
      expect(await loadConfig(bare))
        .toEqual({ path: join(bare, "package.json"), root: null, config: null });
      await writeFile(join(bare, "package.json"), "{ not json");
      await expect(loadConfig(bare)).rejects.toThrow("package.json is not valid JSON");
    } finally {
      await rm(bare, { recursive: true, force: true });
    }
  });

  test("fails the build when the configured command does", async () => {
    const directory = await mkdtemp(join(tmpdir(), "blitsen-command-"));
    try {
      await expect(runBuildCommand("exit 3", directory))
        .rejects.toThrow("build command failed with exit code 3: exit 3");
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  test("runs the configured build and ingests the directory it wrote", async () => {
    const workspace = await temporaryDirectory("blitsen-wrapped-");
    const project = join(workspace, "app");
    await cp(join(configFixtures, "wrapped"), project, { recursive: true });
    const cwd = process.cwd();
    let built;
    try {
      process.chdir(project);
      const here = process.cwd();
      const { lines, output } = capture();
      const runtime = {
        build: async options => {
          built = options;
          return { outfile: options.outfile, assets: 1, bytes: 1 };
        },
      };
      expect(await main(["build"], output, runtime)).toBe(0);
      expect(lines[0][1])
        .toBe(`⓪ build   node emit-dist.mjs (configured in ${join(project, "package.json")})`);
      // The command really ran: Blitsen only knows the directory it left behind.
      expect(await readFile(join(project, "dist/index.html"), "utf8")).toContain("wrapped");
      expect(built.root).toBe(await realpath(join(project, "dist")));
      expect(built.title).toBe("Wrapped App");
      expect(built.outfile).toBe(join(here, "Wrapped App"));
      // Configured addon paths are the user's, relative to their package.json.
      expect(built.addons).toEqual([join(project, "native/greet.node")]);
      expect(built.window).toEqual({ type: "borderless", resizable: false });
      expect(built.tray).toEqual({
        icon: join(project, "native/tray.png"), closeToTray: true,
        contextMenu: [{ action: "show" }, { action: "quit" }],
      });
    } finally {
      process.chdir(cwd);
      await rm(workspace, { recursive: true, force: true });
    }
    // These two spawn a real `node` against a copied fixture, and the default
    // 5s was not enough on the arm64 Windows runner: the test timed out, the
    // `finally` above deleted the workspace, and the process that was still
    // starting reported the build script as missing (#133).
  }, 60_000);

  test("runs the configured build and opens the directory it wrote", async () => {
    const workspace = await temporaryDirectory("blitsen-wrapped-run-");
    const project = join(workspace, "app");
    await cp(join(configFixtures, "wrapped"), project, { recursive: true });
    const cwd = process.cwd();
    let opened;
    try {
      process.chdir(project);
      const { lines, output } = capture();
      let pumps = 0;
      expect(await main([], output, {
        openDirectory: async options => { opened = options; },
        pumpWindow: () => ++pumps < 2,
        waitForNextFrame: async () => {},
      })).toBe(0);
      expect(lines[0][1])
        .toBe(`⓪ build   node emit-dist.mjs (configured in ${join(project, "package.json")})`);
      // The same directory `blitsen build` would have ingested, found the same
      // way: the run proves what ships rather than something beside it.
      expect(opened.root).toBe(await realpath(join(project, "dist")));
      expect(opened.title).toBe("Wrapped App");
      expect(opened.window).toEqual({ type: "borderless", resizable: false });
      expect(opened.tray.icon).toBe(join(project, "native/tray.png"));
    } finally {
      process.chdir(cwd);
      await rm(workspace, { recursive: true, force: true });
    }
  }, 60_000);

  test("still opens the directory you are standing in when it has no config", async () => {
    const directory = await temporaryDirectory("blitsen-unconfigured-run-");
    const cwd = process.cwd();
    try {
      process.chdir(directory);
      await writeFile(join(directory, "index.html"), "<p>hi");
      const { output } = capture();
      let opened;
      let pumps = 0;
      expect(await main([], output, {
        openDirectory: async options => { opened = options; },
        pumpWindow: () => ++pumps < 2,
        waitForNextFrame: async () => {},
      })).toBe(0);
      expect(opened.root).toBe(await realpath(directory));
    } finally {
      process.chdir(cwd);
      await rm(directory, { recursive: true, force: true });
    }
  });

  test("asks for a directory or a config when there is nothing here to build", async () => {
    const directory = await temporaryDirectory("blitsen-unconfigured-");
    const cwd = process.cwd();
    try {
      process.chdir(directory);
      const { lines, output } = capture();
      expect(await main(["build"], output, { build: async () => ({}) })).toBe(1);
      expect(lines[0][1]).toContain("pass one, or add an index.html here");
      expect(lines[0][1]).toContain('add a "blitsen" config to');

      // A directory of static output is already an application — there is no
      // build command to configure, and `blitsen` opens this same directory
      // with no argument, so `blitsen build` exports it with no argument too.
      await writeFile(join(directory, "index.html"), "<p>hi");
      const exported = capture();
      const built = [];
      expect(await main(["build"], exported.output, {
        build: async options => { built.push(options); return { outfile: "app", assets: 1, bytes: 2 }; },
      })).toBe(0);
      expect(built[0].root).toBe(await realpath(directory));
    } finally {
      process.chdir(cwd);
      await rm(directory, { recursive: true, force: true });
    }
  });

  test("reports missing entrypoints and unavailable native addons", async () => {
    const { lines, output } = capture();
    expect(await main([import.meta.dir], output, {})).toBe(1);
    expect(lines[0][1]).toContain("missing or unreadable entrypoint");
  });
});
