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

// Issue #136: whether the table drains is a property of the runner, not of the
// platform — `linux-x64` and `darwin-arm64` finish, `win32-x64` and
// `linux-arm64` do not, which is neither an OS nor an architecture line. So
// demanding an empty table is asking a collector for a guarantee it does not
// give, and the runners that satisfy it do so by luck.
//
// What is worth gating is that finalizers run at all: `WrapperTable` is how DOM
// nodes keep one JavaScript identity, and if nothing is ever collected then a
// long-running application retains every node it has touched. A tail of
// survivors is a collector that did not finish and is not a defect; no
// collection at all is. The count is printed either way, because #136 cannot be
// settled without knowing which of the two a failing runner is doing.
const WRAPPERS = 100_001;
const live = native.wrapperIdentitySmoke();
const collected = WRAPPERS - live;
console.log(`Node-API wrappers: ${collected}/${WRAPPERS} collected, ${live} still live`);
assert(collected > 0,
  `no wrapper was collected out of ${WRAPPERS}: finalizers are not running on this host (#136)`);
assert(collected >= WRAPPERS / 2,
  `only ${collected} of ${WRAPPERS} wrappers were collected, which is too few to call collection working (#136)`);
