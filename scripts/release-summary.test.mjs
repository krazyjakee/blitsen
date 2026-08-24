import { afterEach, describe, expect, test } from "bun:test";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";

const temporaryDirectories = [];

afterEach(async () => {
  await Promise.all(temporaryDirectories.splice(0).map((path) => rm(path, { recursive: true })));
});

function run(args, env = {}) {
  return Bun.spawnSync(["bash", "scripts/release-summary.sh", ...args], {
    env: { ...process.env, ...env },
    stdout: "pipe",
    stderr: "pipe",
  });
}

describe("release input handling", () => {
  test("accepts ordinary npm distribution tags", () => {
    for (const tag of ["latest", "next", "beta-1", "release_candidate.2"]) {
      expect(run(["validate-tag", tag]).exitCode).toBe(0);
    }
  });

  test("rejects shell syntax and semver-shaped tags", () => {
    for (const tag of [
      "bad;printf injected",
      "bad`printf injected`",
      "bad$(printf injected)",
      "bad'quote",
      'bad"quote',
      "1.2.3",
      "v1.2.3",
      "has space",
    ]) {
      expect(run(["validate-tag", tag]).exitCode).not.toBe(0);
    }
  });

  test("renders benign values as data without executing hostile input", async () => {
    const directory = await mkdtemp(join(tmpdir(), "blitsen-release-summary-"));
    temporaryDirectories.push(directory);
    const summary = join(directory, "summary.md");
    const marker = join(directory, "injected");
    const hostile = `bad; touch ${marker}; #`;

    expect(
      run(["summary"], {
        DIST_TAG: hostile,
        GITHUB_STEP_SUMMARY: summary,
        PUBLISHED: "false",
        RELEASE_URL: "",
        VERSION: "0.2.1",
      }).exitCode,
    ).not.toBe(0);
    expect(await Bun.file(marker).exists()).toBe(false);

    expect(
      run(["summary"], {
        DIST_TAG: "next",
        GITHUB_STEP_SUMMARY: summary,
        PUBLISHED: "false",
        RELEASE_URL: "https://example.test/releases/v0.2.1",
        VERSION: "0.2.1",
      }).exitCode,
    ).toBe(0);
    expect(await readFile(summary, "utf8")).toContain("Published: false (tag `next`)");
  });
});
