import { describe, expect, test } from "bun:test";
import { readFile, writeFile } from "node:fs/promises";
import { gzipSync } from "node:zlib";
import { join } from "node:path";

import { BARE_APP } from "./bare-app.mjs";
import { pinnedInterpreter } from "./measurement-runtime.mjs";
import { withTemporaryDirectory } from "./cli-support.mjs";
import { comparisonFixture, comparisonSummary, footprint } from "./run-size-comparison.mjs";
import { runtimeSizeSummary } from "./size-reports.mjs";

describe("size evidence", () => {
  test("refuses to measure whichever installed runtime happens to resolve", async () => {
    await expect(pinnedInterpreter({ env: {}, resolve: () => {
      throw new Error("resolution should not run");
    } })).rejects.toThrow("requires BLITSEN_RUNTIME_PATH");

    const runtime = await pinnedInterpreter({
      env: { BLITSEN_RUNTIME_PATH: "/checkout/blitsen-runtime" },
      resolve: async ({ env }) => ({ path: env.BLITSEN_RUNTIME_PATH, source: "environment" }),
    });
    expect(runtime.path).toBe("/checkout/blitsen-runtime");

    await expect(pinnedInterpreter({
      env: { BLITSEN_RUNTIME_PATH: "/checkout/blitsen-runtime" },
      resolve: async () => ({ path: "/published/blitsen-runtime", source: "package" }),
    })).rejects.toThrow("resolved from package");
  });

  test("measures complete directory contents with a stated compression proxy", async () => {
    await withTemporaryDirectory("blitsen-footprint-test-", async directory => {
      const first = Buffer.from("first fixture");
      const second = Buffer.from("second fixture");
      await writeFile(join(directory, "first"), first);
      await writeFile(join(directory, "second"), second);
      expect(await footprint(directory)).toEqual({
        installedBytes: first.length + second.length,
        compressedBytes: gzipSync(first, { level: 9 }).length
          + gzipSync(second, { level: 9 }).length,
        files: 2,
      });
    });
  });

  test("uses one exact application and pinned comparison versions", async () => {
    expect(BARE_APP).toBe(await readFile(join(comparisonFixture, "web/index.html"), "utf8"));
    const electron = JSON.parse(
      await readFile(join(comparisonFixture, "electron/package.json"), "utf8"),
    );
    expect(electron.author).toBeTruthy();
    const tools = JSON.parse(await readFile(join(comparisonFixture, "package.json"), "utf8"));
    expect(tools.devDependencies).toEqual({
      "@electron/packager": "20.3.0",
      "@tauri-apps/cli": "2.11.4",
      electron: "43.4.1",
    });
    const tauri = await readFile(join(comparisonFixture, "tauri/src-tauri/Cargo.toml"), "utf8");
    expect(tauri).toContain('tauri = { version = "=2.11.5", features = [] }');
  });

  test("publishes readable Bun and framework summaries", () => {
    const phase2 = {
      platform: "linux-x64", commit: "abc123", bun: "1.3.14",
      runtime: { bytes: 10_000_000, gzip: 5_000_000 },
      components: { nativeAddon: 9_999_000, appPayload: 1_000 },
    };
    expect(runtimeSizeSummary(phase2)).toContain("Bun 1.3.14");
    expect(comparisonSummary({ platform: "linux-x64", frameworks: {
      blitsen: { installedBytes: 10_000_000, compressedBytes: 5_000_000, files: 1 },
      electron: { installedBytes: 100_000_000, compressedBytes: 50_000_000, files: 10 },
      tauri: { installedBytes: 3_000_000, compressedBytes: 1_000_000, files: 1 },
    } })).toContain("Tauri uses the operating system WebView");
  });

  test("CI records Phase 2 on six targets and comparisons on the primary three", async () => {
    const workflow = await readFile(join(import.meta.dir, "../../../.github/workflows/ci.yml"), "utf8");
    expect(workflow.match(/name: Bun runtime size breakdown/g)?.length).toBe(2);
    expect(workflow).toContain("Measure equivalent bare Electron and Tauri applications");
    expect(workflow).toContain("desktop-size-comparison-${{ matrix.os }}-${{ github.sha }}");
    expect(workflow).toContain("runtime-size-${{ matrix.target }}-${{ github.sha }}");
  });
});
