import { describe, expect, test } from "bun:test";
import { join } from "node:path";
import { main, parseArgs, resolveApplication, resolveAsset } from "../src/cli.mjs";

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
      .toEqual({ directory: "app", width: 1024, height: 720, title: "Demo" });
    expect(() => parseArgs(["app", "--width", "nope"])).toThrow("positive integer");
  });

  test("resolves an index and relative assets", async () => {
    const fixture = join(import.meta.dir, "../../../spikes/s7/fixture");
    const app = await resolveApplication(fixture);
    expect(app.entrypoint.endsWith("fixture/index.html")).toBeTrue();
    expect((await resolveAsset(app.root, app.entrypoint, "src/main.js")).endsWith("src/main.js"))
      .toBeTrue();
    await expect(resolveAsset(app.root, app.entrypoint, "/src/main.js")).rejects.toThrow(
      "must be relative",
    );
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

  test("reports missing entrypoints and unavailable native addons", async () => {
    const { lines, output } = capture();
    expect(await main([import.meta.dir], output, {})).toBe(1);
    expect(lines[0][1]).toContain("missing or unreadable entrypoint");
  });
});
