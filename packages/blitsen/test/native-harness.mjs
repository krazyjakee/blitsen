import { strict as assert } from "node:assert";
import { createRequire } from "node:module";
import { join } from "node:path";

const addonPath = process.argv[2];
if (!addonPath) throw new Error("usage: bun native-harness.mjs <addon.node>");
const native = createRequire(import.meta.url)(addonPath);
assert.equal(native.nodeApiSmoke(), true, "Bun implements the load-bearing Node-API subset");
assert.equal(native.wrapperIdentitySmoke(), true, "Node-API wrappers preserve identity and collect");
const scriptFixture = join(import.meta.dir, "fixtures/scripts");
const scriptSnapshot = JSON.parse(native.runDocumentScriptsHarness(
  join(scriptFixture, "index.html"),
  320,
  180,
));
const scriptTarget = scriptSnapshot.nodes.find((node) => node.attributes.id === "script-target");
assert(scriptTarget, "script fixture target reached the Rust tree");
assert.equal(scriptTarget.attributes["data-order"], "inline,async,defer,module,inline-module");
assert.match(scriptTarget.attributes["data-module-url"], /module\.js$/);
let scriptError;
try {
  native.runDocumentScriptsHarness(join(scriptFixture, "error.html"), 320, 180);
} catch (error) {
  scriptError = error;
}
assert(scriptError, "broken external script throws");
assert.match(String(scriptError.stack ?? scriptError), /intentional script fixture failure/);
assert.match(String(scriptError.stack ?? scriptError), /broken\.js/);
const snapshot = JSON.parse(native.runBridgeHarness(
  `<style>#x { display:block; width:100px; height:20px }</style><div id="x">old</div>`,
  `{ if (window !== globalThis || window.document !== document || innerWidth !== 320 || innerHeight !== 180 || devicePixelRatio !== 1)
       throw new Error("window identity, document, or initial viewport failed");
     if ("location" in window || "history" in window || "navigator" in window || "localStorage" in window)
       throw new Error("unsupported browser globals must be omitted");
     __blitsenWindowResize("640", "480");
     if (innerWidth !== 640 || innerHeight !== 480 || devicePixelRatio !== 1)
       throw new Error("window viewport did not synchronize after resize");
     const el = document.querySelector("#x");
     const byId = document.getElementById("x");
     const initial = document.querySelectorAll("#x");
     if (el !== byId || !(initial instanceof NodeList) || initial.length !== 1 || initial.item(0) !== el)
       throw new Error("document lookup or wrapper identity failed");
     const created = document.createElement("section");
     created.id = "created";
     created.appendChild(document.createTextNode("new"));
     const staticList = document.querySelectorAll("section");
     document.body.appendChild(created);
     if (staticList.length !== 0 || !(document.body instanceof Element) || !(document.documentElement instanceof Element))
       throw new Error("document creation, roots, or static NodeList semantics failed");
     el.textContent = "hi";
     el.setAttribute("class", "done");
     el.setAttribute("data-window", "ok");
     el.style.width = "140px"; }`,
  320,
  180,
));
const target = snapshot.nodes.find((node) => node.attributes.id === "x");
assert(target, "Rust tree contains #x");
assert(snapshot.invalidation.restyled_nodes > 0, "frame exposes restyled-node scope");
assert(snapshot.invalidation.relaid_out_nodes >= snapshot.invalidation.restyled_nodes,
  "layout invalidation propagates through ancestors");
assert.equal(snapshot.invalidation.full_document, false, "Blitz incremental layout is active");
assert.equal(target.text_content, "hi");
assert.equal(target.attributes.class, "done");
assert.equal(target.attributes["data-window"], "ok");
assert.match(target.inline_style, /width:\s*140px/);
assert.equal(target.layout.width, 140);
const created = snapshot.nodes.find((node) => node.attributes.id === "created");
assert(created, "document-created element reached the Rust tree");
assert.equal(created.text_content, "new");

const treeSnapshot = JSON.parse(native.runBridgeHarness(
  `<body><div id="a"><i id="one">one</i><i id="two">two</i></div><div id="b"></div></body>`,
  `{ const expect = (condition, message) => { if (!condition) throw new Error(message); };
     const a = document.getElementById("a");
     const b = document.getElementById("b");
     const one = document.getElementById("one");
     const two = document.getElementById("two");
     const three = document.createElement("i"); three.id = "three";
     expect(a.appendChild(three) === three && a.childNodes.item(2) === three && three.parentNode === a, "appendChild");
     const zero = document.createElement("i"); zero.id = "zero";
     expect(a.insertBefore(zero, one) === zero && a.firstChild === zero && zero.nextSibling === one, "insertBefore");
     b.appendChild(two);
     expect(two.parentNode === b && ![...a.childNodes].includes(two), "move detaches old parent");
     const removed = a.removeChild(one);
     expect(removed === one && removed.parentNode === null && !removed.isConnected, "removeChild");
     zero.remove();
     expect(zero.parentNode === null && !zero.isConnected, "remove");
     const replacement = document.createElement("strong"); replacement.id = "replacement";
     three.replaceWith(replacement);
     expect(a.firstChild === replacement && replacement.nextSibling === null && three.parentNode === null, "replaceWith");
     a.setAttribute("data-tree", "ok"); }`,
  320,
  180,
));
const treeById = new Map(treeSnapshot.nodes.map((node) => [node.attributes.id, node]));
assert.equal(treeById.get("a").attributes["data-tree"], "ok");
assert.equal(treeById.get("replacement").parent, treeById.get("a").handle);
assert.equal(treeById.get("two").parent, treeById.get("b").handle);
for (const removedId of ["zero", "one", "three"])
  assert.equal(treeById.has(removedId), false, `${removedId} is detached from the Rust document tree`);

const contentSnapshot = JSON.parse(native.runBridgeHarness(
  `<style>#content > .wide { display:block; width:240px; height:30px }</style><div id="content"><b>A</b></div>`,
  `{ const content = document.getElementById("content");
     if (content.textContent !== "A") throw new Error("textContent getter");
     content.textContent = "a < b & c";
     if (content.innerHTML !== "a &lt; b &amp; c" || content.childNodes.length !== 1)
       throw new Error("textContent setter or escaped serialization");
     const detachedText = content.firstChild;
     content.innerHTML = '<span id="replacement-content" class="wide">A &amp; B</span><em>tail</em>';
     if (content.textContent !== "A & Btail" || detachedText.parentNode !== null || detachedText.isConnected)
       throw new Error("contextual innerHTML replacement");
     if (content.innerHTML !== '<span id="replacement-content" class="wide">A &amp; B</span><em>tail</em>')
       throw new Error("innerHTML serialization");
     content.setAttribute("data-content", "ok"); }`,
  320,
  180,
));
const contentById = new Map(contentSnapshot.nodes.map((node) => [node.attributes.id, node]));
assert.equal(contentById.get("content").attributes["data-content"], "ok");
assert.equal(contentById.get("replacement-content").layout.width, 240);

const attributeSnapshot = JSON.parse(native.runBridgeHarness(
  `<style>#attr { display:block; width:100px; height:10px } .active { width:220px !important }</style><div id="attr"></div>`,
  `{ const element = document.getElementById("attr");
     if (element.getAttribute("title") !== null || element.hasAttribute("title")) throw new Error("missing attribute");
     element.setAttribute("title", "hello");
     if (element.getAttribute("title") !== "hello" || !element.hasAttribute("title")) throw new Error("set/get/has attribute");
     element.removeAttribute("title");
     if (element.getAttribute("title") !== null || element.hasAttribute("title")) throw new Error("remove attribute");
     element.id = "renamed";
     if (element.id !== "renamed" || document.getElementById("attr") !== null || document.getElementById("renamed") !== element)
       throw new Error("reflected id or live ID lookup");
     element.className = "base";
     element.classList.add("active", "base");
     if (!element.classList.contains("active") || element.className !== "base active") throw new Error("classList add/contains");
     if (element.classList.toggle("active") || !element.classList.toggle("active", true)) throw new Error("classList toggle");
     element.classList.add("forced");
     element.classList.remove("base");
     const beforeInvalid = element.className;
     let syntaxError = false;
     try { element.classList.add("valid", "two words"); } catch (error) { syntaxError = error.name === "SyntaxError"; }
     if (!syntaxError || element.className !== beforeInvalid) throw new Error("classList token validation must be atomic");
     element.setAttribute("data-attributes", "ok"); }`,
  320,
  180,
));
const reflected = attributeSnapshot.nodes.find((node) => node.attributes.id === "renamed");
assert(reflected, "reflected ID reaches the authoritative tree");
assert.equal(reflected.attributes.class, "active forced");
assert.equal(reflected.attributes["data-attributes"], "ok");
assert.equal(reflected.layout.width, 220, "class mutation triggers the real Blitz cascade");

const styleSnapshot = JSON.parse(native.runBridgeHarness(
  `<style>#styled { display:block; width:90px; height:10px }</style><div id="styled"></div>`,
  `{ const element = document.getElementById("styled");
     const style = element.style;
     if (style.width !== "" || style.getPropertyValue("width") !== "") throw new Error("inline reads must exclude computed style");
     style.left = "40px";
     style.backgroundColor = "red";
     style.cssFloat = "left";
     style.setProperty("TOP", "12px");
     if (style.left !== "40px") throw new Error("camelCase left: " + style.left);
     if (style.getPropertyValue("background-color") !== "red") throw new Error("camelCase backgroundColor: " + style.getPropertyValue("background-color"));
     if (style.cssFloat !== "left") throw new Error("cssFloat: " + style.cssFloat);
     if (style.removeProperty("top") !== "12px" || style.getPropertyValue("top") !== "") throw new Error("removeProperty");
     style.width = "10px";
     style.width = "definitely-invalid";
     if (style.width !== "10px") throw new Error("invalid values must preserve the old declaration");
     const started = performance.now();
     for (let index = 0; index < 1000; index++) style.height = (10 + index % 10) + "px";
     element.setAttribute("data-style-call-us", String(Math.round((performance.now() - started) * 1000 / 1000)));
     style.cssText = "left: 5px; color: green; width: definitely-invalid";
     if (style.getPropertyValue("left") !== "5px" || style.getPropertyValue("color") !== "green" || style.getPropertyValue("width") !== "" || !style.cssText.includes("left: 5px"))
       throw new Error("cssText get/set or invalid declaration filtering");
     element.setAttribute("data-style", "ok"); }`,
  320,
  180,
));
const styled = styleSnapshot.nodes.find((node) => node.attributes.id === "styled");
assert.equal(styled.attributes["data-style"], "ok");
assert.match(styled.inline_style, /left:\s*5px/);
assert.doesNotMatch(styled.inline_style, /definitely-invalid/);
assert.equal(styled.layout.width, 90);

const acceptanceHtml =
  `<style>#x { width: 180px; height: 80px; background: #ef4444 }</style><div id="x">old</div>`;
const acceptanceScript = `{ const painted = document.querySelector("#x");
  painted.textContent = "hi";
  painted.style.backgroundColor = "#22c55e"; }`;
const paintedSnapshot = JSON.parse(native.runBridgeHarness(
  acceptanceHtml,
  acceptanceScript,
  320,
  180,
));
const green = paintedSnapshot.paint_colors.find((color) => color.rgba === "#22c55eff");
assert(green?.pixels > 10_000, "post-mutation frame paints the expected green panel");
const mutatedPng = Buffer.from(native.renderBridgeHarnessPng(
  acceptanceHtml,
  acceptanceScript,
  320,
  180,
), "base64");
const baselinePng = Buffer.from(native.renderBridgeHarnessPng(
  acceptanceHtml,
  ``,
  320,
  180,
), "base64");
assert.deepEqual([...mutatedPng.subarray(0, 8)], [137, 80, 78, 71, 13, 10, 26, 10]);
assert.notDeepEqual(mutatedPng, baselinePng, "post-mutation PNG differs from the parsed frame");
console.log("bridge harness passed", process.platform, process.arch, `style=${styled.attributes["data-style-call-us"]}us/call`);
