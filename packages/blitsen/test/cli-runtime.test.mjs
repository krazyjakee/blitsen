import { describe, expect, test } from "bun:test";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { main } from "../src/cli.mjs";
import { buildStandalone, runtimeRecord } from "../src/export.mjs";
import { describeRuntime, hostTarget, openRuntime, phase2Binary, resolvePhase2Runtime, resolveRuntime, TARGETS }
  from "../src/runtime.mjs";
import { viteBase, engineBuilt, executableStub, nativeStub, platformPackages, cliVersion, withPlatformPackages, withStubbedExport, capture } from "./cli-support.mjs";

describe("runtime resolution", () => {
  test("adapts an engine with its resolved metadata and the caller's wait strategy", async () => {
    const calls = [];
    class Engine {
      openDirectory(options) {
        calls.push(["open", options]);
        return "opened";
      }
      reloadCSS(file) {
        calls.push(["css", file]);
        return true;
      }
      reloadDirectory() {
        calls.push(["reload"]);
      }
      pumpWindow() {
        calls.push(["pump"]);
        return false;
      }
    }
    const resolved = { path: "/runtime/blitsen.node", source: "test" };
    const waits = [];
    const waitForNextFrame = async delay => waits.push(delay);
    const runtime = openRuntime(resolved, {
      loadAddon(path) {
        expect(path).toBe(resolved.path);
        return { Engine };
      },
      waitForNextFrame,
    });

    expect(runtime.resolved).toBe(resolved);
    expect(runtime.openDirectory({ root: "/app" })).toBe("opened");
    expect(runtime.reloadCSS("app.css")).toBeTrue();
    runtime.reloadDirectory();
    expect(runtime.pumpWindow()).toBeFalse();
    await runtime.waitForNextFrame(12);
    expect(calls).toEqual([
      ["open", { root: "/app" }], ["css", "app.css"], ["reload"], ["pump"],
    ]);
    expect(waits).toEqual([12]);
  });

  test("the bin entry point delegates engine adaptation and keeps Bun's wait strategy", async () => {
    const entrypoint = await readFile(join(import.meta.dir, "../bin/blitsen.mjs"), "utf8");
    expect(entrypoint).toContain("openRuntime(resolved");
    expect(entrypoint).toContain("waitForNextFrame: delay => Bun.sleep(delay)");
    expect(entrypoint).toContain("buildStandalone(options, resolved)");
    expect(entrypoint).not.toContain("new native.Engine");
  });

  test("declares one platform package per target, pinned to this version exactly", async () => {
    const manifest = JSON.parse(await readFile(join(import.meta.dir, "../package.json"), "utf8"));
    const license = await readFile(join(import.meta.dir, "../LICENSE"), "utf8");
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
      expect(platform.files).toContain("LICENSE");
      expect(await readFile(join(platformPackages, target, "LICENSE"), "utf8")).toBe(license);
      // A range would allow a pair that was never built together; see TECH.md §11.
      expect(platform.version).toBe(cliVersion);
      expect(manifest.optionalDependencies[platform.name]).toBe(cliVersion);
    }
  });

  test("picks both binaries from the package matching the target", async () => {
    await withPlatformPackages(
      {
        "linux-x64": { version: cliVersion, phase2: true },
        "darwin-arm64": { version: cliVersion, phase2: true },
      },
      async ({ directory, require }) => {
        for (const target of ["linux-x64", "darwin-arm64"]) {
          for (const [resolve, binary] of [
            [resolveRuntime, "blitsen.node"],
            [resolvePhase2Runtime, phase2Binary(target)],
          ]) {
            expect(await resolve({ target, version: cliVersion, env: {}, require })).toEqual({
              path: join(directory, "node_modules/@blitsen", target, binary),
              target,
              version: cliVersion,
              package: `@blitsen/${target}`,
              source: "package",
            });
          }
        }
      });
  });

  test("names the platform, and says it is unpublished, when its package is absent", async () => {
    await withPlatformPackages({ "linux-x64": { version: cliVersion } }, async ({ require }) => {
      // Not this host's target: an absent package falls back to a checkout's own
      // build, which a runner that just built one has (#134).
      const absent = hostTarget() === "win32-arm64" ? "darwin-x64" : "win32-arm64";
      const missing = resolveRuntime({ target: absent, version: cliVersion, env: {}, require });
      await expect(missing).rejects.toThrow(
        `no Blitsen runtime for ${absent}: @blitsen/${absent} is not installed`);
      await expect(missing)
        .rejects.toThrow("no platform runtime package is published yet");
      // A host outside the six is a different failure: nothing to install at all.
      await expect(resolveRuntime({ target: "freebsd-x64", version: cliVersion, env: {}, require }))
        .rejects.toThrow("Blitsen has no runtime for freebsd-x64: supported targets are darwin-arm64");
    });
  });

  test("pins both installed binaries to the CLI's exact version", async () => {
    await withPlatformPackages(
      { "linux-x64": { version: "0.0.9", phase2: true } },
      async ({ require }) => {
        for (const resolve of [resolveRuntime, resolvePhase2Runtime]) {
          await expect(resolve({
            target: "linux-x64", version: "1.2.3", env: {}, require,
          })).rejects.toThrow("runtime version mismatch: blitsen 1.2.3 requires "
            + "@blitsen/linux-x64 1.2.3, but 0.0.9 is installed");
        }
      });
  });

  test("fetches the binary each resolver names after local sources are exhausted", async () => {
    await withPlatformPackages({}, async ({ directory, require }) => {
      const target = TARGETS.find(candidate => candidate !== hostTarget());
      const cacheDir = join(directory, "cache");
      const cached = join(cacheDir, "runtimes", cliVersion, target);
      await mkdir(cached, { recursive: true });
      for (const [resolve, binary] of [
        [resolveRuntime, "blitsen.node"],
        [resolvePhase2Runtime, phase2Binary(target)],
      ]) {
        const path = join(cached, binary);
        await writeFile(path, "cached runtime");
        expect(await resolve({
          target, version: cliVersion, env: {}, require, fetch: true, cacheDir,
        })).toEqual({ path, target, version: cliVersion, package: `@blitsen/${target}`,
          source: "cache" });
      }
    });
  });

  test("Phase 2 can fall past an installed package from before it carried that binary", async () => {
    const target = TARGETS.find(candidate => candidate !== hostTarget());
    await withPlatformPackages({ [target]: { version: cliVersion } },
      async ({ directory, require }) => {
        const cacheDir = join(directory, "cache");
        const cached = join(cacheDir, "runtimes", cliVersion, target);
        const path = join(cached, phase2Binary(target));
        await mkdir(cached, { recursive: true });
        await writeFile(path, "cached Phase 2 runtime");
        expect(await resolvePhase2Runtime({
          target, version: cliVersion, env: {}, require, fetch: true, cacheDir,
        })).toEqual({ path, target, version: cliVersion, package: `@blitsen/${target}`,
          source: "cache" });
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
    await withPlatformPackages({ "linux-x64": { version: cliVersion } }, async ({ directory, require }) => {
      const addon = join(directory, "explicit.node");
      await writeFile(addon, nativeStub("linux-x64"));
      for (const configured of [addon, pathToFileURL(addon).href]) {
        const notices = [];
        expect(await resolveRuntime({
          target: "linux-x64", version: cliVersion, require,
          env: { BLITSEN_NATIVE_PATH: configured },
          onNotice: notice => notices.push(notice),
        })).toEqual({ path: addon, target: "linux-x64", version: null, package: null,
          source: "environment" });
        expect(notices).toEqual([`blitsen: BLITSEN_NATIVE_PATH overrides package/cache `
          + `resolution for linux-x64 with an unversioned binary: ${addon}`]);
      }
    });
  });

  test("an environment addon must match an explicit target, and says how to stop overriding it", async () => {
    await withPlatformPackages({}, async ({ directory, require }) => {
      const addon = join(directory, "host.node");
      await writeFile(addon, nativeStub("linux-x64"));
      await expect(resolveRuntime({
        target: "darwin-arm64", version: cliVersion, require,
        env: { BLITSEN_NATIVE_PATH: addon }, onNotice: () => {},
      })).rejects.toThrow("the linked runtime is built for linux-x64 (ELF), "
        + "but runtime resolution requested darwin-arm64");
      await expect(resolveRuntime({
        target: "darwin-arm64", version: cliVersion, require,
        env: { BLITSEN_NATIVE_PATH: addon }, onNotice: () => {},
      })).rejects.toThrow("Unset BLITSEN_NATIVE_PATH, or point it at a darwin-arm64 binary");
    });
  });

  test("BLITSEN_RUNTIME_PATH is a visible, target-checked Phase 2 override", async () => {
    await withPlatformPackages({}, async ({ directory, require }) => {
      const runtime = join(directory, "blitsen-runtime");
      await writeFile(runtime, executableStub("linux-arm64"));
      const notices = [];
      expect(await resolvePhase2Runtime({
        target: "linux-arm64", version: cliVersion, require,
        env: { BLITSEN_RUNTIME_PATH: runtime },
        onNotice: notice => notices.push(notice),
      })).toEqual({ path: runtime, target: "linux-arm64", version: null, package: null,
        source: "environment" });
      expect(notices).toEqual([`blitsen: BLITSEN_RUNTIME_PATH overrides package/cache `
        + `resolution for linux-arm64 with an unversioned binary: ${runtime}`]);

      await expect(resolvePhase2Runtime({
        target: "win32-x64", version: cliVersion, require,
        env: { BLITSEN_RUNTIME_PATH: runtime }, onNotice: () => {},
      })).rejects.toThrow("the linked Phase 2 runtime is built for linux-arm64 (ELF), "
        + "but runtime resolution requested win32-x64");
      await expect(resolvePhase2Runtime({
        target: "win32-x64", version: cliVersion, require,
        env: { BLITSEN_RUNTIME_PATH: runtime }, onNotice: () => {},
      })).rejects.toThrow("Unset BLITSEN_RUNTIME_PATH, or point it at a win32-x64 binary");
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
    // This host's target rather than a fixed one: the record is what the export
    // links against, and the exporter refuses a runtime built for anything but
    // the target being built for. Fixed at `linux-x64`, the test asserted the
    // record on an x64 runner and asserted that refusal on an arm64 one (#133).
    const target = hostTarget();
    const runtime = { target, version: "1.2.3", package: `@blitsen/${target}`,
      source: "package" };
    await withStubbedExport(async ({ nativePath, outfile }) => {
      const result = await buildStandalone(
        { root: viteBase, width: 800, height: 600, title: "Base", outfile },
        { ...runtime, path: nativePath });
      expect(result.runtime).toEqual({ ...runtime, path: nativePath });
      // The stamp survives the link, so a shipped executable names its own runtime.
      expect((await readFile(result.outfile)).includes(JSON.stringify(runtime))).toBeTrue();
    });
    const { lines, output } = capture();
    expect(await main(["build", join(import.meta.dir, "../../../examples/pong"),
      "--outfile", "/tmp/blitsen-never"], output,
    { build: async () => ({ outfile: "/tmp/pong", assets: 3, bytes: 123, runtime }) })).toBe(0);
    expect(lines.map(([, line]) => line)).toContain(`Runtime: @blitsen/${target}@1.2.3`);
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
