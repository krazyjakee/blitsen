import { describe, expect, test } from "bun:test";
import { join } from "node:path";
import { createReloadCoordinator, main, packageVersion, parseArgs, resolveApplication } from "../src/cli.mjs";
import { doctorApplication } from "../src/doctor.mjs";
import { rewriteRootRelativeReferences } from "../src/export.mjs";

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
    expect(() => parseArgs(["app", "--width", "nope"])).toThrow("positive integer");
    expect(() => parseArgs(["app", "--force"])).toThrow("only valid with build");
    expect(() => parseArgs(["doctor", "dist", "--outfile", "x"])).toThrow("not valid with doctor");
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
