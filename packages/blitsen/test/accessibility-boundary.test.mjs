// Accessibility is outside v1 (#102). These assertions bind the resolved
// Cargo feature graph to the public compatibility boundary: documentation must
// not claim there is no platform tree while Blitz's AccessKit adapter is live.
import { describe, expect, test } from "bun:test";
import { readFile } from "node:fs/promises";
import { join } from "node:path";

const repository = join(import.meta.dir, "../../..");
const source = path => readFile(join(repository, path), "utf8");

describe("the v1 accessibility boundary", () => {
  test("disables Blitz's platform adapter centrally while retaining required custom widgets", async () => {
    const [workspace, renderer, host, node, lock] = await Promise.all([
      source("Cargo.toml"),
      source("crates/blitsen-blitz/Cargo.toml"),
      source("crates/blitsen-host/Cargo.toml"),
      source("crates/blitsen-node/Cargo.toml"),
      source("Cargo.lock"),
    ]);

    expect(workspace).toContain('default-features = false, features = ["net", "tracing"]');
    for (const manifest of [renderer, host, node]) expect(manifest).toContain("blitz.workspace = true");

    const domFeatures = /^blitz-dom = \{.*$/m.exec(renderer)?.[0];
    expect(domFeatures).toBeDefined();
    expect(domFeatures).toContain('"custom-widget"');
    expect(domFeatures).not.toContain('"accessibility"');

    expect(lock).not.toContain('name = "accesskit_xplat"');
    expect(lock).not.toContain('name = "accesskit_macos"');
    expect(lock).not.toContain('name = "accesskit_windows"');
    // Upstream custom-widget still activates the internal node model. This is
    // intentionally asserted rather than hidden: only its platform adapter is
    // gone, and removing custom widgets would break canvas/native views.
    expect(lock).toContain('name = "accesskit"');
  });

  test("keeps docs and DOM keyboard focus on the same explicit boundary", async () => {
    const [compatibility, product, platforms, webApis, technical, manifest, element] =
      await Promise.all([
        source("docs/COMPATIBILITY.md"),
        source("docs/PRODUCT.md"),
        source("docs/PLATFORM-SUPPORT.md"),
        source("docs/WEB-APIS.md"),
        source("docs/TECH.md"),
        source("packages/blitsen/src/api-manifest.json").then(JSON.parse),
        source("crates/blitsen-host/src/dom_bridge/bootstrap/element.js"),
      ]);

    expect(compatibility).toContain("Deliberately absent: no roles, accessible names, focus state");
    expect(product).toContain("does not\nenable Blitz's AccessKit platform adapter");
    expect(platforms).toContain("There is deliberately no platform accessibility tree");
    expect(webApis).toContain("Deliberately not exported: screen readers receive no roles");
    expect(technical).toContain("Blitsen disables `blitz/accessibility`");
    expect(technical).toContain("`accesskit_xplat` are absent");

    expect(manifest.apis.find(api => api.api === "Element.tabIndex")?.status)
      .toBe("implemented");
    expect(element).toContain("focus() { if (isFocusable(this)) setFocus(this); }");
  });
});
