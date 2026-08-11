import { describe, expect, test } from "bun:test";
import { mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createReloadCoordinator, main, packageVersion, parseArgs, resolveApplication } from "../src/cli.mjs";
import { doctorApplication } from "../src/doctor.mjs";
import { buildStandalone, planIngest, rewriteRootRelativeReferences } from "../src/export.mjs";

const viteBase = join(import.meta.dir, "fixtures/vite-base");

// Bun.build --compile refuses to start without the addon file, but never loads
// it, so a placeholder is enough to exercise the whole export pipeline.
async function withStubbedExport(run) {
  const directory = await mkdtemp(join(tmpdir(), "blitsen-export-test-"));
  const nativePath = join(directory, "blitsen.node");
  await writeFile(nativePath, "// placeholder addon\n");
  try {
    return await run({ directory, nativePath, outfile: join(directory, "App") });
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}

function capture() {
  const lines = [];
  return {
    lines,
    output: {
      log: (line) => lines.push(["out", line]),
      error: (line) => lines.push(["err", line]),
    },
  };
}

describe("directory CLI", () => {
  test("prints help", async () => {
    const { lines, output } = capture();
    expect(await main(["--help"], output)).toBe(0);
    expect(lines[0][1]).toContain("Usage: blitsen <directory>");
  });

  test("parses native window flags", () => {
    expect(parseArgs(["app", "--width", "1024", "--height", "720", "--title", "Demo"]))
      .toEqual({ command: "run", directory: "app", width: 1024, height: 720, title: "Demo" });
    expect(parseArgs(["build", "dist", "--outfile", "Demo", "--force"]))
      .toEqual({ command: "build", directory: "dist", width: 800, height: 600,
        title: "Blitsen", outfile: "Demo", force: true });
    expect(parseArgs(["doctor", "dist", "--json"]))
      .toEqual({ command: "doctor", directory: "dist", width: 800, height: 600,
        title: "Blitsen", json: true });
    expect(parseArgs(["build", "dist", "--include", "*.txt", "--include", "meta/**",
      "--assets", "side-loaded"]))
      .toEqual({ command: "build", directory: "dist", width: 800, height: 600,
        title: "Blitsen", include: ["*.txt", "meta/**"], assets: "side-loaded" });
    expect(() => parseArgs(["app", "--width", "nope"])).toThrow("positive integer");
    expect(() => parseArgs(["app", "--force"])).toThrow("only valid with build");
    expect(() => parseArgs(["doctor", "dist", "--outfile", "x"])).toThrow("not valid with doctor");
    expect(() => parseArgs(["build", "dist", "--assets", "inline"])).toThrow("embedded or side-loaded");
    expect(() => parseArgs(["app", "--include", "*.txt"])).toThrow("only valid with build");
  });

  test("resolves an index", async () => {
    const fixture = join(import.meta.dir, "../../../spikes/s7/fixture");
    const app = await resolveApplication(fixture);
    expect(app.entrypoint.endsWith("fixture/index.html")).toBeTrue();
  });

  test("reports the manifest version rather than a literal", async () => {
    const { lines, output } = capture();
    expect(await main(["--version"], output)).toBe(0);
    expect(lines[0][1]).toBe(await packageVersion());
  });

  test("diagnoses output outside the strict compatibility profile", async () => {
    const fixtures = join(import.meta.dir, "fixtures/doctor");
    const compatible = await doctorApplication(join(fixtures, "compatible"));
    expect(compatible).toMatchObject({ profile: "v0-strict", errors: 0, warnings: 0, files: 3 });

    const { lines, output } = capture();
    expect(await main(["doctor", join(fixtures, "unsupported")], output)).toBe(1);
    expect(lines.some(([, line]) => line.includes("HTML_CANVAS") && line.includes("native viewport")))
      .toBeTrue();
    expect(lines.some(([, line]) => line.includes("WEB_STORAGE") && line.includes("filesystem")))
      .toBeTrue();
    expect(lines.at(-1)[1]).toContain("errors, 0 warnings");
  });

  test("normalizes Vite root-relative HTML and CSS references during ingest", () => {
    expect(rewriteRootRelativeReferences(
      '<script src="/assets/app.js?v=1"></script><a href="/settings">x</a>',
      "index.html",
    )).toBe('<script src="./assets/app.js?v=1"></script><a href="/settings">x</a>');
    expect(rewriteRootRelativeReferences(
      '.hero{background:url("/assets/hero.png#main")}',
      "assets/app.css",
    )).toBe('.hero{background:url("./hero.png#main")}');
  });

  // The fixture is shaped like minified bundler output with base "/app/": an
  // unspaced side-effect import, a base-prefixed import(), a transitive
  // @import, and new URL(…, import.meta.url).
  test("walks the module and stylesheet graph from the HTML entrypoint", async () => {
    const plan = await planIngest(viteBase);
    expect(plan.files.map(file => file.relative)).toEqual([
      "assets/chunk-BASE.js",
      "assets/index-BASE.css",
      "assets/index-BASE.js",
      "assets/lazy-BASE.js",
      "assets/panel.svg",
      "assets/theme.css",
      "index.html",
    ]);
    expect(plan.unreferenced).toEqual(["assets/index-BASE.js.map", "assets/orphan.txt"]);
  });

  test("keeps unreferenced output that an --include glob asks for", async () => {
    const plan = await planIngest(viteBase, { include: ["assets/*.txt"] });
    expect(plan.files.some(file => file.relative === "assets/orphan.txt")).toBeTrue();
    expect(plan.files.some(file => file.relative === "assets/index-BASE.js.map")).toBeFalse();
    expect(plan.unreferenced).toEqual(["assets/index-BASE.js.map"]);
    const everything = await planIngest(viteBase, { include: ["**"] });
    expect(everything.files).toHaveLength(9);
    expect(everything.unreferenced).toEqual([]);
  });

  test("resolves a custom bundler base against the real output layout", async () => {
    const plan = await planIngest(viteBase);
    const resolutions = plan.resolutions.get("index.html");
    expect(resolutions.get("/app/assets/index-BASE.js")).toBe("assets/index-BASE.js");
    const source = await readFile(join(viteBase, "index.html"), "utf8");
    const rewritten = rewriteRootRelativeReferences(source, "index.html",
      path => resolutions.get(path) ?? null);
    expect(rewritten).toContain('src="./assets/index-BASE.js"');
    expect(rewritten).toContain('href="./assets/index-BASE.css"');
    // Navigation targets are not subresources and stay exactly as authored.
    expect(rewritten).toContain('<a href="/app/docs">');
  });

  test("fails the build on references it cannot resolve inside the output", async () => {
    const directory = await mkdtemp(join(tmpdir(), "blitsen-missing-"));
    try {
      await writeFile(join(directory, "index.html"), '<link rel="stylesheet" href="/assets/gone.css">');
      await expect(planIngest(directory)).rejects.toThrow("index.html references /assets/gone.css");
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  test("refuses to build output with compatibility errors", async () => {
    const fixture = join(import.meta.dir, "fixtures/doctor/remote");
    let built = false;
    const { lines, output } = capture();
    const runtime = { build: async () => { built = true; return {}; } };
    expect(await main(["build", fixture, "--outfile", "/tmp/blitsen-never"], output, runtime)).toBe(1);
    expect(built).toBeFalse();
    expect(lines.some(([, line]) => line.includes("ASSET_REMOTE"))).toBeTrue();
    expect(lines.at(-1)[1]).toContain("1 compatibility error blocks this build");
  });

  test("hashes collected assets and compiles the same input to identical bytes", async () => {
    await withStubbedExport(async ({ nativePath, outfile }) => {
      const options = { root: viteBase, width: 800, height: 600, title: "Base", outfile, force: true };
      const first = await buildStandalone(options, nativePath);
      const bytes = await readFile(outfile);
      const second = await buildStandalone(options, nativePath);

      expect(first.layout).toBe("embedded");
      expect(first.assets).toBe(7);
      expect(first.unreferenced).toEqual(["assets/index-BASE.js.map", "assets/orphan.txt"]);
      expect(first.manifest.every(asset => /^[0-9a-f]{64}$/.test(asset.hash))).toBeTrue();
      // The hash covers the staged copy, so it reflects the rewritten references.
      const staged = first.manifest.find(asset => asset.path === "assets/index-BASE.css");
      expect(staged.hash).toBe(new Bun.CryptoHasher("sha256")
        .update('#root { background: url("./panel.svg") }\n@import "./theme.css";\n')
        .digest("hex"));
      expect(second.manifest).toEqual(first.manifest);
      // Byte equality holds for one input directory, output path, working
      // directory and Bun version: Bun records the compiled entrypoint's path.
      expect(Buffer.compare(bytes, await readFile(outfile))).toBe(0);
    });
  });

  test("lays assets out beside the executable when asked", async () => {
    await withStubbedExport(async ({ nativePath, outfile }) => {
      const result = await buildStandalone({
        root: viteBase, width: 800, height: 600, title: "Base", outfile, assets: "side-loaded",
      }, nativePath);
      expect(result.layout).toBe("side-loaded");
      expect(result.assetDirectory).toBe(`${outfile}.assets`);
      expect((await readdir(result.assetDirectory)).sort()).toEqual(["assets", "index.html"]);
      const side = await readFile(join(result.assetDirectory, "index.html"), "utf8");
      expect(side).toContain('src="./assets/index-BASE.js"');
      expect(new Bun.CryptoHasher("sha256").update(side).digest("hex"))
        .toBe(result.manifest.find(asset => asset.path === "index.html").hash);
    });
  });

  test("opens a resolved directory through the native runtime", async () => {
    const fixture = join(import.meta.dir, "../../../spikes/s7/fixture");
    let opened;
    let pumps = 0;
    const runtime = {
      openDirectory: async (options) => { opened = options; },
      pumpWindow: () => ++pumps < 3,
      waitForNextFrame: async () => {},
    };
    expect(await main([fixture, "--title", "Fixture"], console, runtime)).toBe(0);
    expect(opened.title).toBe("Fixture");
    expect(opened.entrypoint.endsWith("index.html")).toBeTrue();
    expect(pumps).toBe(3);
  });

  test("builds a resolved directory through the standalone exporter", async () => {
    const fixture = join(import.meta.dir, "../../../examples/pong");
    let built;
    const runtime = {
      build: async options => {
        built = options;
        return { outfile: "/tmp/pong", assets: 3, bytes: 123 };
      },
    };
    const { lines, output } = capture();
    expect(await main(["build", fixture, "--outfile", "/tmp/pong"], output, runtime)).toBe(0);
    expect(built.command).toBe("build");
    expect(built.entrypoint.endsWith("examples/pong/index.html")).toBeTrue();
    expect(lines[0][1]).toContain("Built /tmp/pong (3 assets, 123 bytes)");
    expect(lines[1][1]).toContain("not yet cleared for redistribution");
  });

  test("yields timer macrotasks and microtasks before the next native pump", async () => {
    const fixture = join(import.meta.dir, "../../../spikes/s7/fixture");
    const order = [];
    let pumps = 0;
    const runtime = {
      openDirectory: async () => {},
      pumpWindow: () => {
        order.push(`pump:${++pumps}`);
        return pumps < 2;
      },
      waitForNextFrame: () => new Promise(resolve => setTimeout(() => {
        order.push("timer");
        Promise.resolve().then(() => order.push("microtask"));
        resolve();
      }, 0)),
    };
    expect(await main([fixture], console, runtime)).toBe(0);
    expect(order).toEqual(["pump:1", "timer", "microtask", "pump:2"]);
  });

  test("subtracts render work from the 60 Hz wait budget", async () => {
    const fixture = join(import.meta.dir, "../../../spikes/s7/fixture");
    const waits = [];
    let pumps = 0;
    const originalNow = performance.now;
    const times = [0, 6, 20, 41];
    performance.now = () => times.shift() ?? 41;
    try {
      const runtime = {
        openDirectory: async () => {},
        pumpWindow: () => ++pumps < 3,
        waitForNextFrame: async delay => waits.push(delay),
      };
      expect(await main([fixture], console, runtime)).toBe(0);
    } finally {
      performance.now = originalNow;
    }
    expect(waits[0]).toBeCloseTo(1000 / 60 - 6, 5);
    expect(waits[1]).toBeCloseTo(1000 / 30 - 20, 5);
  });

  test("debounces CSS swaps and escalates mixed batches to one document reload", async () => {
    const calls = [];
    const coordinator = createReloadCoordinator({
      reloadCSS: async file => calls.push(["css", file]),
      reloadDirectory: async () => calls.push(["document"]),
    }, console, 5);
    coordinator.notify("styles/app.css");
    coordinator.notify("styles/app.css");
    coordinator.notify("styles/theme.CSS");
    await Bun.sleep(15);
    await coordinator.settled();
    expect(calls).toEqual([["css", "styles/app.css"], ["css", "styles/theme.CSS"]]);

    coordinator.notify("styles/app.css");
    coordinator.notify("src/app.js");
    coordinator.notify("index.html");
    await Bun.sleep(15);
    await coordinator.settled();
    expect(calls.at(-1)).toEqual(["document"]);
    expect(calls.filter(call => call[0] === "document")).toHaveLength(1);
    coordinator.close();
  });

  test("falls back to a document reload when no stylesheet link matched", async () => {
    const calls = [];
    const coordinator = createReloadCoordinator({
      reloadCSS: async file => { calls.push(["css", file]); return false; },
      reloadDirectory: async () => calls.push(["document"]),
    }, console, 5);
    coordinator.notify("styles/imported.css");
    await Bun.sleep(15);
    await coordinator.settled();
    expect(calls).toEqual([["css", "styles/imported.css"], ["document"]]);
    coordinator.close();
  });

  test("reports missing entrypoints and unavailable native addons", async () => {
    const { lines, output } = capture();
    expect(await main([import.meta.dir], output, {})).toBe(1);
    expect(lines[0][1]).toContain("missing or unreadable entrypoint");
  });
});
