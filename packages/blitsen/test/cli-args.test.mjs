import { describe, expect, test } from "bun:test";
import { join } from "node:path";
import { main, packageVersion, parseArgs, resolveApplication } from "../src/cli.mjs";
import { TARGETS } from "../src/runtime.mjs";
import { icon, capture } from "./cli-support.mjs";

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
    expect(parseArgs(["build", "dist", "--addon", "native/physics.node"]).addons)
      .toEqual([join(process.cwd(), "native/physics.node")]);
    expect(() => parseArgs(["app", "--addon", "physics.node"])).toThrow("only valid with build");
    expect(parseArgs(["build", "dist", "--icon", "app.png", "--bundle-id", "com.example.pong",
      "--app-version", "1.2.3", "--sign", "codesign -s ID"]))
      .toEqual({ command: "build", directory: "dist", width: 800, height: 600, title: "Blitsen",
        icon: "app.png", bundleId: "com.example.pong", appVersion: "1.2.3", sign: "codesign -s ID" });
    expect(() => parseArgs(["app", "--icon", "app.png"])).toThrow("only valid with build");
  });

  test("names the application once for the title, the output file and the metadata", () => {
    expect(parseArgs(["build", "dist", "--out", "Demo", "--name", "My App"]))
      .toEqual({ command: "build", directory: "dist", width: 800, height: 600,
        title: "My App", name: "My App", outfile: "Demo" });
    // An explicit --title wins over the name it would otherwise default to.
    expect(parseArgs(["build", "dist", "--name", "My App", "--title", "Window"]).title).toBe("Window");
    expect(parseArgs(["build", "dist", "--title", "Window", "--name", "My App"]).title).toBe("Window");
    expect(() => parseArgs(["app", "--name", "My App"])).toThrow("only valid with build");
  });

  // Cross-target export is accepted now (#72): the target's runtime is fetched
  // on demand and the launcher is compiled for that target's Bun. What is still
  // refused is a triple Blitsen has no runtime for at all, and that refusal has
  // to name the six so the reader does not have to guess the spelling.
  test("accepts every supported target and refuses anything else", () => {
    const host = `${process.platform}-${process.arch}`;
    const other = host === "linux-x64" ? "darwin-arm64" : "linux-x64";
    expect(parseArgs(["build", "dist", "--target", host]).target).toBe(host);
    expect(parseArgs(["build", "dist", "--target", other]).target).toBe(other);
    for (const target of TARGETS) {
      expect(parseArgs(["build", "dist", "--target", target]).target).toBe(target);
    }
    expect(() => parseArgs(["build", "dist", "--target", "sunos-x64"]))
      .toThrow("unknown --target sunos-x64");
    expect(() => parseArgs(["build", "dist", "--target", "sunos-x64"])).toThrow("linux-x64");
  });

  test("requires a directory for every command except a configured build", () => {
    expect(parseArgs(["build"]))
      .toEqual({ command: "build", directory: null, width: 800, height: 600, title: "Blitsen" });
    expect(() => parseArgs(["doctor"])).toThrow("missing application directory");
    expect(() => parseArgs(["--width", "800"])).toThrow("missing application directory");
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

});
