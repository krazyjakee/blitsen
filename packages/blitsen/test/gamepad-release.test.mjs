import { describe, expect, test } from "bun:test";
import { readFile } from "node:fs/promises";
import { join } from "node:path";

const repository = join(import.meta.dir, "../../..");

function hostDependencyBlocks(workflow) {
  return [...workflow.matchAll(
    /- name: Install Blitz system dependencies[\s\S]*?sudo apt-get install -y --no-install-recommends \\\n([\s\S]*?)(?=\n\s{6}- (?:name:|run:|uses:))/g,
  )]
    .map(match => match[1])
    // The layout-only job does not compile blitsen-host or gilrs.
    .filter(block => block.includes("libasound2-dev"));
}

describe("desktop gamepad release prerequisites", () => {
  test("every Linux host build declares libudev headers", async () => {
    for (const path of [".github/workflows/ci.yml", ".github/workflows/release.yml"]) {
      const workflow = await readFile(join(repository, path), "utf8");
      const blocks = hostDependencyBlocks(workflow);
      expect(blocks.length).toBeGreaterThan(0);
      for (const block of blocks) expect(block).toContain("libudev-dev");
    }
  });

  test("gilrs remains desktop-target-gated", async () => {
    const manifest = await readFile(
      join(repository, "crates/blitsen-host/Cargo.toml"), "utf8");
    expect(manifest).toContain(
      "[target.'cfg(any(target_os = \"linux\", target_os = \"windows\", target_os = \"macos\"))'.dependencies]",
    );
    expect(manifest).toContain('gilrs = "0.11.2"');
  });

  test("standard haptic promises have no JavaScript duration clock", async () => {
    const bootstrap = await readFile(join(
      repository, "crates/blitsen-host/src/dom_bridge/bootstrap/gamepad.js"), "utf8");
    expect(bootstrap).not.toContain("setTimeout");
    expect(bootstrap).toContain("String(startDelay)");
    expect(bootstrap).toContain("command.resolve(message.result)");
  });
});
