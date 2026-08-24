// The Rust↔JS bridge is string-keyed: the bootstrap scripts call and install
// `__blitsen*` globals whose other half lives in Rust, and a typo on the JS
// side does not error — it reads as "capability absent" and the feature
// quietly degrades. So every `__blitsen*` name the bootstrap JS mentions must
// be accounted for: either the Rust side names it in a string literal (the
// host installs or looks it up), or the JS itself defines it (an assignment,
// an object-literal property, or a function declaration).
//
// The other direction is deliberately not asserted: Rust names no JS ever
// mentions are fine, because the Rust text includes every cfg branch and every
// platform's half of the bridge.
import { describe, expect, test } from "bun:test";
import { readdir, readFile } from "node:fs/promises";
import { join } from "node:path";

const repository = join(import.meta.dir, "../../..");
const HOST_SRC = "crates/blitsen-host/src";

// Names the JS references that neither side visibly defines, each with the
// reason it is allowed. Empty today; an entry here is a real finding.
const ALLOWLIST = new Set([]);

async function hostFiles(keep) {
  const directory = join(repository, HOST_SRC);
  const names = (await readdir(directory, { recursive: true })).filter(keep);
  const sources = await Promise.all(
    names.map(name => readFile(join(directory, name), "utf8")));
  return sources.join("\n");
}

const bootstrapJs = () => hostFiles(name =>
  name.endsWith(".js") && (name.includes("/bootstrap/") || name.endsWith("/bootstrap.js")));
const rustSource = () => hostFiles(name => name.endsWith(".rs"));

describe("the __blitsen bridge vocabulary", () => {
  test("every name the bootstrap JS references is defined in Rust or in the JS itself", async () => {
    const js = await bootstrapJs();
    const rust = await rustSource();

    const referenced = new Set([...js.matchAll(/__blitsen\w+/g)].map(([name]) => name));
    // Rust's half of a bridge name is always a string literal — a property the
    // host installs, or a key it reads back.
    const rustDefined = new Set([...rust.matchAll(/"(__blitsen\w+)"/g)].map(([, name]) => name));
    // A JS-side definition: `__blitsenX = ...`, `__blitsenX: ...` in an object
    // literal, `function __blitsenX`, or a property explicitly installed on
    // globalThis. `(?!=)` keeps `==` comparisons out.
    const jsDefined = new Set([
      ...[...js.matchAll(/(__blitsen\w+)\s*[:=](?!=)/g)].map(([, name]) => name),
      ...[...js.matchAll(/function\s+(__blitsen\w+)/g)].map(([, name]) => name),
      ...[...js.matchAll(/Object\.defineProperty\(globalThis,\s*"(__blitsen\w+)"/g)]
        .map(([, name]) => name),
    ]);

    // Readers finding nothing must fail: the bridge is ~135 names on each side.
    expect(referenced.size, `no __blitsen names found in the bootstrap JS under ${HOST_SRC}`)
      .toBeGreaterThanOrEqual(100);
    expect(rustDefined.size, `no "__blitsen*" string literals found in the Rust under ${HOST_SRC}`)
      .toBeGreaterThanOrEqual(100);

    const dangling = [...referenced].filter(name =>
      !rustDefined.has(name) && !jsDefined.has(name) && !ALLOWLIST.has(name)).sort();
    expect(dangling, `the bootstrap JS under ${HOST_SRC} references __blitsen names `
      + `that no Rust string literal under ${HOST_SRC} defines and the JS does not `
      + "define itself — a typo here reads as \"capability absent\" rather than "
      + `an error: ${dangling.join(", ")}`).toEqual([]);

    // The allowlist must not outlive its findings.
    for (const name of ALLOWLIST) {
      expect(referenced.has(name) && !rustDefined.has(name) && !jsDefined.has(name),
        `${name} no longer needs its allowlist entry`).toBeTrue();
    }
  });
});
