import { describe, expect, test } from "bun:test";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { compareReleaseBuilds, hashReleaseArtifacts }
  from "../../../scripts/compare-release-builds.mjs";
import { repository } from "./build-addon.mjs";

describe("release reproducibility", () => {
  test("compares both unsigned artifacts and names the first differing byte", async () => {
    const directory = await mkdtemp(join(tmpdir(), "blitsen-repro-"));
    const first = join(directory, "first");
    const second = join(directory, "second");
    await mkdir(first);
    await mkdir(second);
    await Bun.write(join(first, "addon"), Buffer.from([1, 2, 3, 4]));
    await Bun.write(join(first, "runtime"), Buffer.from([5, 6, 7, 8]));
    await Bun.write(join(second, "addon"), Buffer.from([1, 2, 3, 4]));
    await Bun.write(join(second, "runtime"), Buffer.from([5, 6, 9, 8]));
    try {
      const lines = [];
      await expect(compareReleaseBuilds({
        firstRoot: first, secondRoot: second, library: "addon", executable: "runtime",
        output: line => lines.push(line), summaryPath: null,
      })).rejects.toThrow("first differing byte 2");
      expect(lines.join("\n")).toContain("clean build A blitsen.node:");
      expect(lines.join("\n")).toContain("clean build B runtime:");
      expect(lines.join("\n")).toMatch(/[0-9a-f]{64}/);
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  test("records matching SHA-256 hashes without changing the artifacts", async () => {
    const directory = await mkdtemp(join(tmpdir(), "blitsen-repro-hash-"));
    try {
      await writeFile(join(directory, "addon"), "addon");
      await writeFile(join(directory, "runtime"), "runtime");
      const before = await readFile(join(directory, "addon"));
      const records = await hashReleaseArtifacts({
        root: directory, library: "addon", executable: "runtime", output: () => {},
        summaryPath: null,
      });
      expect(records).toHaveLength(2);
      expect(records.every(record => /^[0-9a-f]{64}$/.test(record.hash))).toBeTrue();
      expect(await readFile(join(directory, "addon"))).toEqual(before);
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  test("gates one pinned native runner per executable format before signing", async () => {
    const workflow = await readFile(join(repository, ".github/workflows/release.yml"), "utf8");
    const build = await readFile(join(repository, "scripts/build-release-runtime.sh"), "utf8");
    const selected = [...workflow.matchAll(
      /- target: ([^\n]+)\n(?: {12}[^\n]+\n){3} {12}reproducible: true/g,
    )].map(match => match[1]);
    expect(selected).toEqual(["linux-x64", "darwin-x64", "win32-x64"]);
    expect(workflow).toContain("scripts/build-release-runtime.sh");
    expect(workflow).toContain("scripts/compare-release-builds.mjs --compare");
    expect(workflow.indexOf("name: Verify unsigned reproducibility"))
      .toBeLessThan(workflow.indexOf("name: Sign (macOS)"));
    expect(build).toContain("--remap-path-prefix=");
    expect(build).toContain("-ffile-prefix-map=");
    expect(build).toContain("/pathmap:");
    expect(build).toContain("/Brepro");
    expect(build).toContain("SOURCE_DATE_EPOCH");
  });
});
