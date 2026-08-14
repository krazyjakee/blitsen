import { strict as assert } from "node:assert";
import { readFile } from "node:fs/promises";
import { join } from "node:path";

import { native } from "./addon.mjs";

// Absent, not stubbed. The manifest is generated from the bootstrap source;
// this asks the runtime that source produces, so neither `doctor` nor
// COMPATIBILITY.md can claim an API the application would find otherwise —
// including one the Phase 1 host supplies and the Phase 2 engine would not.
//
// A path rather than a URL object, because by the time this file runs the
// bridge has installed Blitsen's `URL` over the host's, and `node:fs` only
// accepts the host's. Application code never sees that seam — this harness runs
// inside the Phase 1 host's own realm, which is the one place the two meet.
const manifest = JSON.parse(await readFile(
  join(import.meta.dirname, "../../src/api-manifest.json"), "utf8"));
const declared = JSON.parse(native.runBridgeHarness(
  `<div id="surface"></div>`,
  `{ globalThis.__blitsenSurface = ${JSON.stringify(manifest.apis)}.map(entry => {
       const owner = entry.kind === "global" ? globalThis
         : entry.owner.split(".").reduce((value, key) => value?.[key], globalThis);
       return [entry.api, Boolean(owner) && (entry.member ?? entry.api) in owner];
     });
     document.getElementById("surface").setAttribute("data-surface", "ok"); }`,
  200,
  100,
));
assert.equal(declared.nodes.find(node => node.attributes.id === "surface").attributes["data-surface"], "ok");
const runtimeSurface = new Map(globalThis.__blitsenSurface);
delete globalThis.__blitsenSurface;
for (const entry of manifest.apis) {
  // What the *engine* does not supply is not answerable here: this harness runs
  // the bridge inside the host's own JavaScript realm, which is not the engine
  // an exported application runs on. `cli-doctor.test.mjs` runs the built
  // runtime and checks those against the engine that is actually there.
  if (entry.origin === "engine") continue;
  assert.equal(runtimeSurface.get(entry.api), entry.status === "implemented",
    `${entry.api} is ${entry.status} in the API manifest but the opposite in the runtime`);
}

const routing = JSON.parse(native.runBridgeHarness(
  `<div id="routing"></div>`,
  `{ const trail = globalThis.__blitsenRouting = { entries: [], pops: [], hashes: [] };
     if (location.href !== "blitsen://app/" || location.pathname !== "/" || location.origin !== "blitsen://app" ||
         location.search !== "" || location.hash !== "" || String(location) !== location.href)
       throw new Error("initial address: " + location.href);
     if (history.length !== 1 || history.state !== null || history.scrollRestoration !== "auto")
       throw new Error("initial history entry");
     history.scrollRestoration = "manual";
     window.addEventListener("popstate", event => trail.pops.push([location.pathname, event.state?.idx ?? null]));
     window.addEventListener("hashchange", event => trail.hashes.push([event.oldURL, event.newURL]));
     history.replaceState({ idx: 0 }, "", "/");
     history.pushState({ idx: 1 }, "", "/reports?range=30d");
     trail.entries.push([location.pathname, location.search, history.state.idx, history.length]);
     history.pushState({ idx: 2 }, "", "detail");
     trail.entries.push([location.pathname, history.length]);
     history.replaceState({ idx: 9 }, "", "./renamed");
     trail.entries.push([location.pathname, history.state.idx, history.length]);
     location.hash = "section";
     trail.entries.push([location.hash, location.href, history.length]);
     // Traversal is a task, exactly as it is in a browser.
     if (trail.pops.length !== 0) throw new Error("history.go must not dispatch synchronously");
     history.go(-2);
     for (const [name, argument] of [["href", "https://example.com/"], ["pathname", "/x"], ["search", "?a=1"]]) {
       let refused;
       try { location[name] = argument; } catch (error) { refused = error.name; }
       if (refused !== "NotSupportedError") throw new Error("assigning location." + name + " must refuse loudly");
     }
     let crossOrigin;
     try { history.pushState(null, "", "https://example.com/x"); }
     catch (error) { crossOrigin = error.name; }
     if (crossOrigin !== "SecurityError") throw new Error("cross-origin history entries must be refused");
     document.getElementById("routing").setAttribute("data-routing", "ok"); }`,
  200,
  100,
));
assert.equal(routing.nodes.find(node => node.attributes.id === "routing").attributes["data-routing"], "ok");
assert.deepEqual(globalThis.__blitsenRouting.entries, [
  ["/reports", "?range=30d", 1, 2],
  ["/detail", 3],
  ["/renamed", 9, 3],
  ["#section", "blitsen://app/renamed#section", 4],
], "pushState, replaceState and location.hash keep an in-memory entry list");
assert.deepEqual(globalThis.__blitsenRouting.hashes,
  [["blitsen://app/renamed", "blitsen://app/renamed#section"]], "a fragment change reports both addresses");
await Bun.sleep(10);
assert.deepEqual(globalThis.__blitsenRouting.pops, [["/reports", 1]],
  "history.go traverses to the earlier entry in a later task");
assert.equal(globalThis.__blitsenRouting.hashes.length, 2, "traversal off a fragment also reports hashchange");
delete globalThis.__blitsenRouting;

