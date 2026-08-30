// The conventions the native surface holds to across modules (#365).
//
// Each one was true of some modules and not others until it was written down,
// which is the failure mode a convention has: it is invisible until an
// application hits the module that does it differently. So each is checked
// here against the source rather than restated in a review comment.
import { describe, expect, test } from "bun:test";
import { readFile } from "node:fs/promises";
import { join } from "node:path";

import { generateApiManifest, readBootstrapScript } from "../src/api-manifest.mjs";
import { WINDOW_READBACKS } from "../src/api-manifest/catalogue.mjs";

const repository = join(import.meta.dir, "../../..");
const readSource = name => readFile(join(repository, name), "utf8");
const NATIVE_BOOTSTRAP = "crates/blitsen-host/src/dom_bridge/bootstrap/native.js";

describe("native API conventions", () => {
  test("every window setter has a readback or names what answers for it", async () => {
    const manifest = await generateApiManifest();
    const windowMembers = manifest.native.filter(entry => entry.module === "window");
    const declared = new Set(windowMembers.map(entry => entry.member));
    const setters = windowMembers
      .map(entry => entry.member)
      .filter(member => /^set[A-Z]/.test(member));

    expect(setters.length).toBeGreaterThan(0);
    for (const setter of setters) {
      const pairing = WINDOW_READBACKS[setter];
      expect(pairing, `window.${setter} is unpaired`).toBeDefined();
      if (pairing.readback === undefined) {
        expect(pairing.answeredBy, `window.${setter} names nothing that answers for it`)
          .toBeTruthy();
        continue;
      }
      // Declared is the point: a readback that is absent then owes a reason to
      // NATIVE_ABSENT, which `generateApiManifest` already refuses to skip.
      expect(declared.has(pairing.readback), `window.${pairing.readback} is not declared`)
        .toBe(true);
    }
  });

  test("the module table in NATIVE-APIS.md is the surface the manifest reports", async () => {
    const [manifest, document] = await Promise.all([
      generateApiManifest(), readSource("docs/NATIVE-APIS.md"),
    ]);
    const documented = new Map([...document.matchAll(/^\| `blitsen\/(\w+)` \| (.+) \|$/gm)]
      .map(([, module, members]) =>
        [module, [...members.matchAll(/`(\w+)`/g)].map(([, member]) => member).sort()]));
    const implemented = new Map();
    for (const entry of manifest.native.filter(entry => entry.status === "implemented"))
      implemented.set(entry.module, [...implemented.get(entry.module) ?? [], entry.member].sort());

    expect([...documented.keys()].sort()).toEqual([...implemented.keys()].sort());
    for (const [module, members] of implemented)
      expect(documented.get(module), `blitsen/${module} in NATIVE-APIS.md`).toEqual(members);
  });

  test("one listener registrar serves the whole native surface", async () => {
    const script = await readBootstrapScript();
    // The message is the registrar's, so counting it counts the registrars: a
    // second `onX` written by hand would bring its own copy of this line.
    expect(script.match(/listener must be a function/g)).toHaveLength(1);
  });

  test("no native member is installed against a bare typeof check", async () => {
    // `hosted` exists so an uninstalled host function is never closed over.
    // Reaching for `typeof __blitsen…` directly is the same test written twice.
    expect(await readSource(NATIVE_BOOTSTRAP)).not.toMatch(/typeof __blitsen/);
  });

  test("a mistake in the call throws rather than rejecting", async () => {
    // The rule is the caller's: an argument this layer refuses is refused where
    // it was written. (`GamepadHapticActuator.playEffect` is the standard API
    // and rejects because its specification says so; it is not on this surface.)
    expect(await readSource(NATIVE_BOOTSTRAP)).not.toMatch(/Promise\.reject\(new TypeError/);
  });

  test("a command with nothing to report resolves undefined", async () => {
    expect(await readSource("packages/blitsen/src/native/native.d.ts"))
      .not.toMatch(/Promise<null>/);
  });
});
