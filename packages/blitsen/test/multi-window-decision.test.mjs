// Issue #105 is an architecture decision, not a speculative creation API.
// Keep the user-facing documents, generated capability reason and published
// types aligned on both halves of that decision.
import { describe, expect, test } from "bun:test";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { generateApiManifest, readDeclaredNativeMembers } from "../src/api-manifest.mjs";

const repository = join(import.meta.dir, "../../..");
const readDoc = name => readFile(join(repository, "docs", name), "utf8");

describe("multi-window architecture decision", () => {
  test("is one decided contract across product, compatibility and API guidance", async () => {
    const [product, technical, compatibility, nativeApis] = await Promise.all([
      readDoc("PRODUCT.md"), readDoc("TECH.md"), readDoc("COMPATIBILITY.md"),
      readDoc("NATIVE-APIS.md"),
    ]);
    const decisionLink = "TECH.md#multi-window-contexts-isolated-on-one-ui-thread";

    expect(product).toContain(`(${decisionLink})`);
    expect(compatibility).toContain(`(${decisionLink})`);
    expect(nativeApis).toContain(`(${decisionLink})`);
    expect(product).not.toContain("**Do multiple windows share one JS context**");

    expect(technical).toContain("### Multi-window contexts: isolated on one UI thread");
    expect(technical).toContain("its own global `Window`, DOM tree");
    expect(technical).toContain("Cross-window application communication is asynchronous structured clone");
    expect(technical).toContain("The application session owns the native windows and their contexts");
    expect(technical).toContain("Failures are context-scoped too");
  });

  test("does not accidentally promise the implementation", async () => {
    const manifest = await generateApiManifest();
    const create = manifest.native.find(entry => entry.api === "window.create");
    const [definitions, windowBridge, windowHost] = await Promise.all([
      readFile(join(repository, "packages/blitsen/src/native/native.d.ts"), "utf8"),
      readFile(join(repository, "crates/blitsen-host/src/dom_bridge/window.rs"), "utf8"),
      readFile(join(repository, "crates/blitsen-host/src/native_window.rs"), "utf8"),
    ]);

    expect(create).toMatchObject({ status: "absent" });
    expect(create.reason).toContain("isolated Window, Document, JavaScript heap");
    expect(create.reason).toContain("explicitly transferred MessagePort");
    expect(readDeclaredNativeMembers(definitions).get("window").has("create")).toBeFalse();
    expect(windowBridge).toContain("this singleton must become a calling-context to");
    expect(windowHost).not.toContain("wait on the shared\n    /// versus isolated");
  });
});
