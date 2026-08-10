import { strict as assert } from "node:assert";
import { createRequire } from "node:module";

const addonPath = process.argv[2];
if (!addonPath) throw new Error("usage: bun native-harness.mjs <addon.node>");
const native = createRequire(import.meta.url)(addonPath);
assert.equal(native.wrapperIdentitySmoke(), true, "Node-API wrappers preserve identity and collect");
const snapshot = JSON.parse(native.runBridgeHarness(
  `<style>#x { display:block; width:100px; height:20px }</style><div id="x">old</div>`,
  `{ const el = document.querySelector("#x");
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
     el.style.width = "140px"; }`,
  320,
  180,
));
const target = snapshot.nodes.find((node) => node.attributes.id === "x");
assert(target, "Rust tree contains #x");
assert.equal(target.text_content, "hi");
assert.equal(target.attributes.class, "done");
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
