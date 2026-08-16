import { describe, expect, test } from "bun:test";
import { launcherSource } from "../src/export.mjs";
import { frameDelay } from "../src/frame-pacing.mjs";

describe("frame pacing", () => {
  test("accumulates 60 Hz deadlines and clamps only when more than one frame behind", () => {
    const interval = 1000 / 60;
    const pacing = { nextFrame: 0 };

    expect(frameDelay(pacing, 6)).toBeCloseTo(interval - 6, 10);
    expect(pacing.nextFrame).toBeCloseTo(interval, 10);
    expect(frameDelay(pacing, 20)).toBeCloseTo(interval * 2 - 20, 10);
    expect(pacing.nextFrame).toBeCloseTo(interval * 2, 10);

    const boundary = { nextFrame: 0 };
    expect(frameDelay(boundary, interval * 2)).toBe(0);
    expect(boundary.nextFrame).toBeCloseTo(interval, 10);

    const behind = { nextFrame: 0 };
    expect(frameDelay(behind, interval * 3)).toBe(0);
    expect(behind.nextFrame).toBeCloseTo(interval * 3, 10);
  });

  test("embeds the same pacer while retaining launcher pump, limits, and Bun sleep", () => {
    const source = launcherSource([], {
      layout: "side-loaded",
      assetDirectory: "app.assets",
      runtime: {
        path: "/runtime/blitsen.node",
        target: "linux-x64",
        version: "0.1.0",
        package: "@blitsen/linux-x64",
        source: "package",
      },
      width: 800,
      height: 600,
      title: "Pacing",
    });

    expect(source).toContain(frameDelay.toString());
    expect(source.match(/pacing\.nextFrame < now -/g)).toHaveLength(1);
    expect(source).toContain("while (engine.pumpWindow())");
    expect(source).toContain("if (frames === warmupFrames) started = performance.now()");
    expect(source).toContain("if (frameLimit > 0 && frames >= frameLimit + warmupFrames) break");
    expect(source).toContain("await Bun.sleep(frameDelay(pacing, performance.now()))");
    const ordered = [
      "while (engine.pumpWindow())",
      "frames += 1",
      "if (frames === warmupFrames)",
      "if (frameLimit > 0",
      "await Bun.sleep(frameDelay",
    ].map(fragment => source.indexOf(fragment));
    expect(ordered).toEqual([...ordered].sort((left, right) => left - right));
    expect(ordered.every(index => index >= 0)).toBeTrue();
    expect(() => new Bun.Transpiler({ loader: "js" }).transformSync(source)).not.toThrow();
  });
});
