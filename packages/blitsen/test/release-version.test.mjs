import { describe, expect, test } from "bun:test";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { checkStagedRuntime, matchingManifestVersion }
  from "../../../scripts/release-version.mjs";
import { repository } from "./build-addon.mjs";

async function fixtures(versions) {
  const directory = await mkdtemp(join(tmpdir(), "blitsen-release-version-"));
  const paths = [];
  for (const [name, version] of Object.entries(versions)) {
    const path = join(directory, `${name}.json`);
    await writeFile(path, `${JSON.stringify({ name, version })}\n`);
    paths.push(path);
  }
  return { directory, paths };
}

describe("native release version", () => {
  test("accepts one exact version across the main and platform manifests", async () => {
    const { directory, paths } = await fixtures({ blitsen: "1.2.3", platform: "1.2.3" });
    try {
      expect(await matchingManifestVersion(paths)).toBe("1.2.3");
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  test("rejects a package mismatch before executing the staged runtime", async () => {
    const { directory, paths } = await fixtures({ blitsen: "1.2.3", platform: "1.2.4" });
    let executed = false;
    try {
      await expect(checkStagedRuntime({
        executable: "/not/run", manifests: paths,
        run: async () => { executed = true; return { stdout: "", stderr: "", exitCode: 0 }; },
      })).rejects.toThrow("release manifest version mismatch");
      expect(executed).toBeFalse();
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  test("rejects an executable whose report differs from both manifests", async () => {
    const { directory, paths } = await fixtures({ blitsen: "1.2.3", platform: "1.2.3" });
    try {
      await expect(checkStagedRuntime({
        executable: "/staged/blitsen-runtime", manifests: paths,
        run: async () => ({ stdout: "blitsen-runtime checkout\n", stderr: "", exitCode: 0 }),
      })).rejects.toThrow('reported "blitsen-runtime checkout", expected "blitsen-runtime 1.2.3"');
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  test("accepts the exact command report used by the runtime session", async () => {
    const { directory, paths } = await fixtures({ blitsen: "2.0.0-beta.1", platform: "2.0.0-beta.1" });
    try {
      expect(await checkStagedRuntime({
        executable: "/staged/blitsen-runtime", manifests: paths,
        run: async () => ({ stdout: "blitsen-runtime 2.0.0-beta.1\n", stderr: "", exitCode: 0 }),
      })).toBe("2.0.0-beta.1");
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  test("the workflow stamps and checks every native target before packing", async () => {
    const workflow = await readFile(join(repository, ".github/workflows/release.yml"), "utf8");
    const build = await readFile(join(repository, "scripts/build-release-runtime.sh"), "utf8");
    for (const target of [
      "darwin-arm64", "darwin-x64", "linux-arm64", "linux-x64", "win32-arm64", "win32-x64",
    ]) {
      expect(workflow).toContain(`- target: ${target}`);
    }
    expect(workflow).toContain("BLITSEN_RELEASE_VERSION: ${{ needs.validate.outputs.release_version }}");
    expect(workflow).toContain('if [ "$PUBLISH" = true ]; then');
    expect(workflow).toContain("Dry run is rehearsing already-published blitsen@$version");
    expect(build).toContain("packages/platforms/*/package.json");
    expect(build).toContain("export BLITSEN_RELEASE_VERSION=$manifest_version");
    expect(workflow.indexOf("name: Check the staged runtime version"))
      .toBeLessThan(workflow.indexOf("name: Pack"));
  });
});
