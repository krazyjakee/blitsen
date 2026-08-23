import { describe, expect, test } from "bun:test";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { gzipSync } from "node:zlib";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { BARE_APP } from "./bare-app.mjs";
import { pinnedPhase2Runtime } from "./measurement-runtime.mjs";
import { comparisonFixture, comparisonSummary, footprint } from "./run-size-comparison.mjs";
import { phase2SizeSummary } from "./size-reports.mjs";

describe("size evidence", () => {
  test("refuses to measure whichever installed runtime happens to resolve", async () => {
    await expect(pinnedPhase2Runtime({ env: {}, resolve: () => {
      throw new Error("resolution should not run");
    } })).rejects.toThrow("requires BLITSEN_RUNTIME_PATH");

    const runtime = await pinnedPhase2Runtime({
      env: { BLITSEN_RUNTIME_PATH: "/checkout/blitsen-runtime" },
      resolve: async ({ env }) => ({ path: env.BLITSEN_RUNTIME_PATH, source: "environment" }),
    });
    expect(runtime.path).toBe("/checkout/blitsen-runtime");

    await expect(pinnedPhase2Runtime({
      env: { BLITSEN_RUNTIME_PATH: "/checkout/blitsen-runtime" },
      resolve: async () => ({ path: "/published/blitsen-runtime", source: "package" }),
    })).rejects.toThrow("resolved from package");
  });

  test("measures complete directory contents with a stated compression proxy", async () => {
    const directory = await mkdtemp(join(tmpdir(), "blitsen-footprint-test-"));
    try {
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
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
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

  test("publishes readable Phase 2 and framework summaries", () => {
    const phase2 = {
      platform: "linux-x64", commit: "abc123", ratio: 2,
      phase1: { bytes: 20_000_000, gzip: 10_000_000 },
      phase2: { bytes: 10_000_000, gzip: 5_000_000 },
      components: { runtimeExecutable: 9_999_000, appPayload: 1_000 },
    };
    expect(phase2SizeSummary(phase2)).toContain("runtime pinned by `BLITSEN_RUNTIME_PATH`");
    expect(comparisonSummary({ platform: "linux-x64", frameworks: {
      blitsen: { installedBytes: 10_000_000, compressedBytes: 5_000_000, files: 1 },
      electron: { installedBytes: 100_000_000, compressedBytes: 50_000_000, files: 10 },
      tauri: { installedBytes: 3_000_000, compressedBytes: 1_000_000, files: 1 },
    } })).toContain("Tauri uses the operating system WebView");
  });

  test("CI records Phase 2 on six targets and comparisons on the primary three", async () => {
    const workflow = await readFile(join(import.meta.dir, "../../../.github/workflows/ci.yml"), "utf8");
    expect(workflow.match(/name: Phase 2 size breakdown/g)?.length).toBe(2);
    expect(workflow).toContain("Measure equivalent bare Electron and Tauri applications");
    expect(workflow).toContain("desktop-size-comparison-${{ matrix.os }}-${{ github.sha }}");
    expect(workflow).toContain("phase2-size-${{ matrix.target }}-${{ github.sha }}");
  });
});
