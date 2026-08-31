// Android's renderer selection is a build-time safety boundary (#151). These
// assertions hold the Cargo feature, Rust cfg and CI qualification path to one
// another without pretending a desktop test can create an Android window.
import { describe, expect, test } from "bun:test";
import { readFile } from "node:fs/promises";
import { join } from "node:path";

const repository = join(import.meta.dir, "../../..");
const source = path => readFile(join(repository, path), "utf8");

describe("the Android renderer policy", () => {
  test("defaults to CPU/softbuffer and names one explicit GPU qualification feature", async () => {
    const [cargo, androidCargo, renderer, entry] = await Promise.all([
      source("crates/blitsen-host/Cargo.toml"),
      source("crates/blitsen-android/Cargo.toml"),
      source("crates/blitsen-host/src/native_window/renderer.rs"),
      source("crates/blitsen-android/src/lib.rs"),
    ]);
    const androidDependencies = /\[target\.'cfg\(target_os = "android"\)'\.dependencies\]([\s\S]*?)\n\[/
      .exec(cargo)?.[1];
    expect(androidDependencies).toContain("anyrender_vello_cpu");
    expect(androidDependencies).toContain("softbuffer_window_renderer");
    expect(cargo).toMatch(/^android-vello-gpu = \[\]$/m);
    expect(androidCargo).toContain(
      'android-vello-gpu = ["blitsen-host/android-vello-gpu"]',
    );
    expect(renderer).toContain(
      'all(target_os = "android", not(feature = "android-vello-gpu"))',
    );
    expect(renderer).toContain(
      '#[cfg(all(target_os = "android", feature = "android-vello-gpu"))]',
    );
    expect(renderer).toContain("reason=Android-safe-default");
    expect(renderer).toContain("qualification=Android-mobile-GPU");
    expect(entry).toContain("reason=Android-safe-default");
    expect(entry).toContain("qualification=Android-mobile-GPU");
  });

  test("keeps the opt-in GPU build compiling and makes emulator coverage gating", async () => {
    const workflow = await source(".github/workflows/ci.yml");
    expect(workflow).toContain("--features android-vello-gpu");
    const job = /\n  android-notifications:\n([\s\S]*?)(?=\n  [a-z][\w-]*:\n|$)/
      .exec(workflow)?.[1];
    expect(job).toBeDefined();
    expect(job).not.toContain("continue-on-error:");
    expect(job).not.toContain("blocked on #151");
    expect(job).toContain("test:android-notify");
  });
});
