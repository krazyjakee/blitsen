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
console.log("bridge harness passed", process.platform, process.arch);
