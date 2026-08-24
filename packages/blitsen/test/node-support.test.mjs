import { describe, expect, test } from "bun:test";
import { readFile } from "node:fs/promises";
import { join } from "node:path";

const packageDirectory = join(import.meta.dir, "..");
const repository = join(packageDirectory, "../..");

describe("Node support policy", () => {
  test("keeps the declared floor, guide, and CI matrix aligned", async () => {
    const manifest = JSON.parse(await readFile(join(packageDirectory, "package.json"), "utf8"));
    const guide = await readFile(join(repository, "docs/GETTING-STARTED.md"), "utf8");
    const workflow = await readFile(join(repository, ".github/workflows/ci.yml"), "utf8");

    expect(manifest.engines.node).toBe(">=20.11.0");
    expect(guide).toContain("Node.js 20.11.0 or newer");
    expect(workflow).toMatch(/node-version: \["20\.11\.0", "node"\]/);
  });

  test("the supported floor includes import.meta.dirname", async () => {
    const sources = await Promise.all(["runtime.mjs", "android-toolchain.mjs", "api-manifest.mjs"]
      .map(file => readFile(join(packageDirectory, "src", file), "utf8")));
    expect(sources.every(source => source.includes("import.meta.dirname"))).toBeTrue();
  });
});
