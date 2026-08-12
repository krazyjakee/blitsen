import { describe, expect, test } from "bun:test";
import { mkdir, stat, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { createReloadCoordinator, main } from "../src/cli.mjs";
import { buildStandalone } from "../src/export.mjs";
import { icon, withStubbedExport, capture } from "./cli-support.mjs";

describe("directory CLI", () => {
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

  test("builds a resolved directory through the standalone exporter, naming each step", async () => {
    const fixture = join(import.meta.dir, "../../../examples/pong");
    let built;
    const runtime = {
      build: async options => {
        built = options;
        options.progress({ step: "collect", detail: "3 embedded assets", notes: ["dropped 1 file"] });
        options.progress({ step: "link", detail: "/tmp/pong" });
        options.progress({ step: "package", detail: "linux: /tmp/pong.desktop", notes: ["note"] });
        return { outfile: "/tmp/pong", assets: 3, bytes: 123 };
      },
    };
    const { lines, output } = capture();
    expect(await main(["build", fixture, "--outfile", "/tmp/pong", "--icon", "app.png",
      "--sign", "codesign"], output, runtime)).toBe(0);
    expect(built.command).toBe("build");
    expect(built.entrypoint.endsWith("examples/pong/index.html")).toBeTrue();
    expect(built.icon).toBe("app.png");
    expect(lines[0][1]).toBe(`① ingest  ${built.entrypoint}`);
    expect(lines[1][1]).toMatch(/^② scan {4}\d+ files, 0 errors, \d+ warnings$/);
    const steps = lines.map(([, line]) => line).filter(line => /^[⓪①②③④⑤]/.test(line));
    expect(steps.slice(2)).toEqual([
      "③ collect 3 embedded assets",
      "④ link    /tmp/pong",
      "⑤ package linux: /tmp/pong.desktop",
    ]);
    expect(lines.some(([, line]) => line === "          dropped 1 file")).toBeTrue();
    expect(lines.some(([, line]) => line === "          note")).toBeTrue();
    expect(lines.at(-2)[1]).toBe("Built /tmp/pong (3 assets, 123 bytes)");
    expect(lines.at(-1)[1]).toContain("not yet cleared for redistribution");
  });

  test("names the offending file and exits non-zero when ingest cannot resolve a reference", async () => {
    await withStubbedExport(async ({ directory, nativePath, outfile }) => {
      const root = join(directory, "app");
      await mkdir(root);
      await writeFile(join(root, "index.html"), '<link rel="stylesheet" href="/assets/gone.css">');
      const { lines, output } = capture();
      const runtime = { build: options => buildStandalone(options, nativePath) };
      expect(await main(["build", root, "--out", outfile], output, runtime)).toBe(1);
      expect(lines.at(-1)).toEqual(["err", "blitsen: unresolved local references in the "
        + "application output:\n  index.html references /assets/gone.css"]);
      expect(await stat(outfile).catch(() => null)).toBeNull();
    });
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

});
