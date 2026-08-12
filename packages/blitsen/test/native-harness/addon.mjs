import { strict as assert } from "node:assert";
import { createRequire } from "node:module";
import { join } from "node:path";

// The addon under test, loaded into this realm rather than a child one: the
// bridge installs itself into whatever context calls it, so the classes the
// checks below reach for are the ones an exported application would see.
export const addonPath = process.argv[2];
if (!addonPath) throw new Error("usage: bun native-harness.mjs <addon.node>");

// The `test/` directory, so a section can name a fixture the way it did when
// all of this was one file beside them.
export const testDir = join(import.meta.dir, "..");

export const native = createRequire(import.meta.url)(addonPath);

assert.equal(native.nodeApiSmoke(), true, "Bun implements the load-bearing Node-API subset");
assert.equal(native.wrapperIdentitySmoke(), true, "Node-API wrappers preserve identity and collect");
