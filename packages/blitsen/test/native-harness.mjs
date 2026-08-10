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

const animation = JSON.parse(native.runAnimationHarness(
  `<style>#animated { position:absolute; left:0; top:0; width:20px; height:20px; background:red }</style><div id="animated"></div>`,
  `{ const animated = document.getElementById("animated");
     const timestamps = [];
     let count = 0;
     const cancelled = requestAnimationFrame(() => { throw new Error("cancelAnimationFrame failed"); });
     cancelAnimationFrame(cancelled);
     const step = timestamp => {
       timestamps.push(timestamp);
       count++;
       animated.style.left = (count * 10) + "px";
       animated.setAttribute("data-frame", String(count));
       animated.setAttribute("data-times", timestamps.join(","));
       if (count < 3) requestAnimationFrame(step);
     };
     requestAnimationFrame(step); }`,
  3,
  100,
  60,
));
const animatedFrames = animation.map(frame => frame.nodes.find(node => node.attributes.id === "animated"));
assert.deepEqual(animatedFrames.map(node => node.attributes["data-frame"]), ["1", "2", "3"],
  "callbacks registered during rAF wait for the next frame");
assert.deepEqual(animatedFrames.map(node => node.layout.x), [10, 20, 30],
  "rAF mutations land in each frame being built");
const animationTimestamps = animatedFrames.at(-1).attributes["data-times"].split(",").map(Number);
assert.equal(animationTimestamps.length, 3);
assert(animationTimestamps.every((timestamp, index) => index === 0 || timestamp > animationTimestamps[index - 1]),
  "animation timestamps are monotonic DOMHighResTimeStamp values");

native.runBridgeHarness(
  `<div></div>`,
  `{ globalThis.__blitsenTimerOrder = [];
     setTimeout((first, second) => {
       __blitsenTimerOrder.push("timeout:" + first + ":" + second);
       Promise.resolve().then(() => __blitsenTimerOrder.push("microtask"));
     }, 0, "a", 2);
     const cancelled = setTimeout(() => __blitsenTimerOrder.push("cancelled"), 0);
     clearTimeout(cancelled);
     let intervals = 0;
     const interval = setInterval(() => {
       __blitsenTimerOrder.push("interval:" + ++intervals);
       if (intervals === 2) clearInterval(interval);
     }, 2); }`,
  100,
  60,
);
await Bun.sleep(25);
assert.deepEqual(globalThis.__blitsenTimerOrder.slice(0, 2), ["timeout:a:2", "microtask"],
  "Bun timer arguments are forwarded and microtasks drain after the macrotask");
assert.deepEqual(globalThis.__blitsenTimerOrder.filter(entry => entry.startsWith("interval")),
  ["interval:1", "interval:2"], "intervals repeat and clearInterval stops them");
assert(!globalThis.__blitsenTimerOrder.includes("cancelled"), "clearTimeout cancels the callback");
delete globalThis.__blitsenTimerOrder;

const eventSurface = JSON.parse(native.runBridgeHarness(
  `<div id="event-surface"></div>`,
  `{ const target = document.getElementById("event-surface");
     const event = new Event("submit", { bubbles: true, cancelable: true });
     if (event.type !== "submit" || event.target !== null || event.currentTarget !== null ||
         event.eventPhase !== 0 || !event.bubbles || !event.cancelable || event.defaultPrevented ||
         typeof event.timeStamp !== "number") throw new Error("Event property surface");
     event.preventDefault();
     if (!event.defaultPrevented) throw new Error("preventDefault");
     const mouse = new MouseEvent("click", { clientX: 10, clientY: 11, offsetX: 2, offsetY: 3,
       screenX: 50, screenY: 60, button: 1, buttons: 2, ctrlKey: true, shiftKey: true });
     if (mouse.clientX !== 10 || mouse.clientY !== 11 || mouse.offsetX !== 2 || mouse.offsetY !== 3 ||
         mouse.screenX !== 50 || mouse.screenY !== 60 || mouse.button !== 1 || mouse.buttons !== 2 ||
         !mouse.ctrlKey || !mouse.shiftKey || mouse.altKey || mouse.metaKey) throw new Error("MouseEvent property surface");
     const keyboard = new KeyboardEvent("keydown", { key: "A", code: "KeyA", repeat: true,
       altKey: true, metaKey: true });
     if (keyboard.key !== "A" || keyboard.code !== "KeyA" || !keyboard.repeat ||
         keyboard.ctrlKey || keyboard.shiftKey || !keyboard.altKey || !keyboard.metaKey)
       throw new Error("KeyboardEvent property surface");
     const detail = { answer: 42 };
     if (new CustomEvent("answer", { detail }).detail !== detail) throw new Error("CustomEvent detail");
     for (const unsupported of ["pageX", "pageY", "which", "charCode", "relatedTarget", "isTrusted"])
       if (unsupported in mouse || unsupported in keyboard || unsupported in event)
         throw new Error("unsupported event property must be absent: " + unsupported);
     target.setAttribute("data-events", "ok"); }`,
  100,
  60,
));
assert.equal(eventSurface.nodes.find(node => node.attributes.id === "event-surface").attributes["data-events"], "ok");

const eventDispatch = JSON.parse(native.runBridgeHarness(
  `<div id="outer"><button id="event-target">go</button></div>`,
  `{ const outer = document.getElementById("outer");
     const target = document.getElementById("event-target");
     if (!(target instanceof EventTarget) || !(document instanceof EventTarget) ||
         typeof window.addEventListener !== "function") throw new Error("EventTarget installation");
     const order = [];
     const listen = (current, name, capture = false) => current.addEventListener("probe", function(event) {
       if (this !== current || event.currentTarget !== current || event.target !== target)
         throw new Error("listener target identity");
       order.push(name + ":" + event.eventPhase);
       if (name === "outer-bubble") event.preventDefault();
     }, capture);
     listen(window, "window-capture", true);
     listen(document, "document-capture", true);
     listen(outer, "outer-capture", true);
     listen(target, "target-capture", true);
     listen(target, "target-bubble");
     listen(outer, "outer-bubble");
     listen(document, "document-bubble");
     listen(window, "window-bubble");
     const event = new CustomEvent("probe", { bubbles: true, cancelable: true, detail: 42 });
     if (target.dispatchEvent(event) !== false || !event.defaultPrevented || event.currentTarget !== null ||
         event.eventPhase !== 0 || event.detail !== 42) throw new Error("dispatchEvent result or final state");
     const expected = ["window-capture:1", "document-capture:1", "outer-capture:1",
       "target-capture:2", "target-bubble:2", "outer-bubble:3", "document-bubble:3", "window-bubble:3"];
     if (order.join(",") !== expected.join(",")) throw new Error("propagation order: " + order);

     let once = 0;
     const onceCallback = () => once++;
     target.addEventListener("once", onceCallback, { once: true });
     target.addEventListener("once", onceCallback, { once: false, passive: true });
     target.dispatchEvent(new Event("once")); target.dispatchEvent(new Event("once"));
     if (once !== 1) throw new Error("once or de-duplication");

     const mutation = [];
     const added = () => mutation.push("added");
     const removed = () => mutation.push("removed");
     target.addEventListener("mutation", () => {
       mutation.push("first");
       target.removeEventListener("mutation", removed);
       target.addEventListener("mutation", added);
     });
     target.addEventListener("mutation", removed);
     target.dispatchEvent(new Event("mutation"));
     if (mutation.join(",") !== "first") throw new Error("listener removal/addition during dispatch");
     target.dispatchEvent(new Event("mutation"));
     if (mutation.join(",") !== "first,first,added") throw new Error("deferred listener addition");

     const oldError = console.error; let reported = 0; console.error = () => reported++;
     let afterThrow = false;
     target.addEventListener("throw", () => { throw new Error("listener failure"); });
     target.addEventListener("throw", () => { afterThrow = true; });
     target.dispatchEvent(new Event("throw")); console.error = oldError;
     if (reported !== 1 || !afterThrow) throw new Error("per-listener exception isolation");

     const stopped = [];
     target.addEventListener("stop", event => { stopped.push("first"); event.stopImmediatePropagation(); });
     target.addEventListener("stop", () => stopped.push("peer"));
     outer.addEventListener("stop", () => stopped.push("ancestor"));
     target.dispatchEvent(new Event("stop", { bubbles: true }));
     if (stopped.join(",") !== "first") throw new Error("stopImmediatePropagation");

     const passive = new Event("passive", { cancelable: true });
     target.addEventListener("passive", event => event.preventDefault(), { passive: true });
     if (!target.dispatchEvent(passive) || passive.defaultPrevented) throw new Error("passive listener");
     let handled = false;
     target.addEventListener("object", { handleEvent(event) { handled = event.currentTarget === target; } });
     target.dispatchEvent(new Event("object"));
     if (!handled) throw new Error("object event listener");
     target.setAttribute("data-dispatch", "ok"); }`,
  200,
  100,
));
assert.equal(eventDispatch.nodes.find(node => node.attributes.id === "event-target").attributes["data-dispatch"], "ok");

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
