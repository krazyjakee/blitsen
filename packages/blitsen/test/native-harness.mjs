import { strict as assert } from "node:assert";
import { createRequire } from "node:module";

const addonPath = process.argv[2];
if (!addonPath) throw new Error("usage: bun native-harness.mjs <addon.node>");
const native = createRequire(import.meta.url)(addonPath);
const snapshot = JSON.parse(native.runBridgeHarness(
  `<style>#x { display:block; width:100px; height:20px }</style><div id="x">old</div>`,
  `const el = document.querySelector("#x");
   el.textContent = "hi";
   el.setAttribute("class", "done");
   el.style.width = "140px";`,
  320,
  180,
));
const target = snapshot.nodes.find((node) => node.attributes.id === "x");
assert(target, "Rust tree contains #x");
assert.equal(target.text_content, "hi");
assert.equal(target.attributes.class, "done");
assert.match(target.inline_style, /width:\s*140px/);
assert.equal(target.layout.width, 140);
const mutatedPng = Buffer.from(native.renderBridgeHarnessPng(
  `<style>#x { width: 180px; height: 80px; background: #ef4444 }</style><div id="x">old</div>`,
  `{ const painted = document.querySelector("#x");
     painted.textContent = "hi";
     painted.style.backgroundColor = "#22c55e"; }`,
  320,
  180,
), "base64");
const baselinePng = Buffer.from(native.renderBridgeHarnessPng(
  `<style>#x { width: 180px; height: 80px; background: #ef4444 }</style><div id="x">old</div>`,
  ``,
  320,
  180,
), "base64");
assert.deepEqual([...mutatedPng.subarray(0, 8)], [137, 80, 78, 71, 13, 10, 26, 10]);
assert.notDeepEqual(mutatedPng, baselinePng, "post-mutation PNG differs from the parsed frame");
console.log("bridge harness passed", process.platform, process.arch);
