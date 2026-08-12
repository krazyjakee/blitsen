import { describe, expect, test } from "bun:test";
import { readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { main } from "../src/cli.mjs";
import { buildStandalone, runtimeRecord } from "../src/export.mjs";
import { describeRuntime, hostTarget, resolveRuntime, TARGETS } from "../src/runtime.mjs";
import { viteBase, engineBuilt, platformPackages, cliVersion, withPlatformPackages, withStubbedExport, capture } from "./cli-support.mjs";

describe("runtime resolution", () => {
  test("declares one platform package per target, pinned to this version exactly", async () => {
    const manifest = JSON.parse(await readFile(join(import.meta.dir, "../package.json"), "utf8"));
    expect(Object.keys(manifest.optionalDependencies))
      .toEqual(TARGETS.map(target => `@blitsen/${target}`));
    for (const target of TARGETS) {
      const [os, cpu] = target.split("-");
      const platform = JSON.parse(
        await readFile(join(platformPackages, target, "package.json"), "utf8"));
      expect(platform.name).toBe(`@blitsen/${target}`);
      // os/cpu are what make a package manager install only the host's binary.
      expect(platform.os).toEqual([os]);
      expect(platform.cpu).toEqual([cpu]);
      expect(platform.exports["./blitsen.node"]).toBe("./blitsen.node");
      // A range would allow a pair that was never built together; see TECH.md §11.
      expect(platform.version).toBe(cliVersion);
      expect(manifest.optionalDependencies[platform.name]).toBe(cliVersion);
    }
  });

  test("picks the package matching the target, ignoring the others installed", async () => {
    await withPlatformPackages(
      { "linux-x64": { version: cliVersion }, "darwin-arm64": { version: cliVersion } },
      async ({ directory, require }) => {
        for (const target of ["linux-x64", "darwin-arm64"]) {
          expect(await resolveRuntime({ target, version: cliVersion, env: {}, require })).toEqual({
            path: join(directory, "node_modules/@blitsen", target, "blitsen.node"),
            target,
            version: cliVersion,
            package: `@blitsen/${target}`,
            source: "package",
          });
        }
      });
  });

  test("names the platform, and says it is unpublished, when its package is absent", async () => {
    await withPlatformPackages({ "linux-x64": { version: cliVersion } }, async ({ require }) => {
      const missing = resolveRuntime({ target: "win32-arm64", version: cliVersion, env: {}, require });
      await expect(missing).rejects.toThrow(
        "no Blitsen runtime for win32-arm64: @blitsen/win32-arm64 is not installed");
      await expect(missing)
        .rejects.toThrow("no platform runtime package is published yet and only linux-x64 is built");
      // A host outside the six is a different failure: nothing to install at all.
      await expect(resolveRuntime({ target: "freebsd-x64", version: cliVersion, env: {}, require }))
        .rejects.toThrow("Blitsen has no runtime for freebsd-x64: supported targets are darwin-arm64");
    });
  });

  test("refuses a platform package whose version is not the CLI's", async () => {
    await withPlatformPackages({ "linux-x64": { version: "0.0.9" } }, async ({ require }) => {
      await expect(resolveRuntime({ target: "linux-x64", version: "1.2.3", env: {}, require }))
        .rejects.toThrow("runtime version mismatch: blitsen 1.2.3 requires @blitsen/linux-x64 "
          + "1.2.3, but 0.0.9 is installed");
    });
  });

  test("refuses a platform package that carries no addon", async () => {
    await withPlatformPackages({ "linux-x64": { version: cliVersion, binary: false } },
      async ({ require }) => {
        await expect(resolveRuntime({ target: "linux-x64", version: cliVersion, env: {}, require }))
          .rejects.toThrow(`@blitsen/linux-x64@${cliVersion} is installed but carries no blitsen.node`);
      });
  });

  test("takes BLITSEN_NATIVE_PATH ahead of an installed package, in either spelling", async () => {
    await withPlatformPackages({ "linux-x64": { version: cliVersion } }, async ({ require }) => {
      const addon = join(tmpdir(), "explicit.node");
      for (const configured of [addon, pathToFileURL(addon).href]) {
        expect(await resolveRuntime({
          target: "linux-x64", version: cliVersion, require,
          env: { BLITSEN_NATIVE_PATH: configured },
        })).toEqual({ path: addon, target: "linux-x64", version: null, package: null,
          source: "environment" });
      }
    });
  });

  // The repository path every test script uses: an addon this checkout built itself,
  // found without an install and deliberately unversioned.
  test.skipIf(!engineBuilt)("falls back to a checkout's own build", async () => {
    await withPlatformPackages({}, async ({ require }) => {
      const resolved = await resolveRuntime({ version: cliVersion, env: {}, require });
      expect(resolved.source).toBe("repository");
      expect(resolved.target).toBe(hostTarget());
      expect(resolved.version).toBeNull();
      expect(await Bun.file(resolved.path).exists()).toBeTrue();
    });
  });

  test("records the runtime an export linked against, in the artifact and the report", async () => {
    const runtime = { target: "linux-x64", version: "1.2.3", package: "@blitsen/linux-x64",
      source: "package" };
    await withStubbedExport(async ({ nativePath, outfile }) => {
      const result = await buildStandalone(
        { root: viteBase, width: 800, height: 600, title: "Base", outfile },
        { ...runtime, path: nativePath });
      expect(result.runtime).toEqual({ ...runtime, path: nativePath });
      // The stamp survives the link, so a shipped executable names its own runtime.
      expect((await readFile(outfile)).includes(JSON.stringify(runtime))).toBeTrue();
    });
    const { lines, output } = capture();
    expect(await main(["build", join(import.meta.dir, "../../../examples/pong"),
      "--outfile", "/tmp/blitsen-never"], output,
    { build: async () => ({ outfile: "/tmp/pong", assets: 3, bytes: 123, runtime }) })).toBe(0);
    expect(lines.map(([, line]) => line)).toContain("Runtime: @blitsen/linux-x64@1.2.3");
  });

  test("describes a runtime that came from a path as unversioned", () => {
    const record = runtimeRecord("/tmp/blitsen.node");
    expect(record).toEqual({ path: "/tmp/blitsen.node", target: hostTarget(), version: null,
      package: null, source: "path" });
    expect(describeRuntime(record)).toBe(`${hostTarget()} (unversioned, from path)`);
    expect(() => runtimeRecord(null))
      .toThrow("native addon is unavailable; reinstall blitsen for this platform");
  });
});
