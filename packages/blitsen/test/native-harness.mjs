import { strict as assert } from "node:assert";
import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
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
assert.equal(scriptTarget.attributes["data-dom-content-loaded"], "interactive");
assert.equal(scriptTarget.attributes["data-load"], "complete");
const interactiveSnapshot = JSON.parse(native.runDocumentScriptsHarness(
  join(import.meta.dir, "../../../examples/interactive/index.html"),
  720,
  520,
));
const interactiveDemo = interactiveSnapshot.nodes.find(node => node.attributes.id === "demo");
assert.equal(interactiveDemo.attributes["data-ready"], "true",
  "interactive acceptance example installs its event and animation script");
const pongSnapshot = JSON.parse(native.runDocumentScriptsHarness(
  join(import.meta.dir, "../../../examples/pong/index.html"),
  720,
  520,
));
const pongGame = pongSnapshot.nodes.find(node => node.attributes.id === "game");
assert.equal(pongGame.attributes["data-ready"], "true",
  "Pong installs its input and animation loop from the three-file application");
assert.equal(pongGame.attributes["data-state"], "paused",
  "Pong starts in a playable serve state");
const pongFrames = JSON.parse(native.runDocumentAnimationHarness(
  join(import.meta.dir, "../../../examples/pong/index.html"),
  `__blitsenDispatchKeyboardEvent("keydown", { bubbles: true, cancelable: true,
     key: " ", code: "Space", repeat: false });
   __blitsenDispatchKeyboardEvent("keydown", { bubbles: true, cancelable: true,
     key: "w", code: "KeyW", repeat: false });`,
  60,
  960,
  640,
));
const pongNode = (snapshot, id) => snapshot.nodes.find(node => node.attributes.id === id);
assert.equal(pongNode(pongFrames[0], "game").attributes["data-state"], "playing",
  "Space serves the ball");
assert(pongNode(pongFrames.at(-1), "left-paddle").layout.y
  < pongNode(pongFrames[0], "left-paddle").layout.y, "W moves player one's paddle");
assert.notEqual(pongNode(pongFrames.at(-1), "ball").layout.x,
  pongNode(pongFrames[0], "ball").layout.x, "the ball advances through requestAnimationFrame");
// The game's own #fps readout is deliberately not asserted. The harness feeds
// JavaScript a fixed 1000/60 ms timestep and the game divides frames by those
// timestamps, so the readout reports ~60 however slow the renderer actually is.
// Real frame cost is measured against wall clock by `frames`; determinism of the
// rendered output is gated by `test:determinism`.
let scriptError;
try {
  native.runDocumentScriptsHarness(join(scriptFixture, "error.html"), 320, 180);
} catch (error) {
  scriptError = error;
}
assert(scriptError, "broken external script throws");
assert.match(String(scriptError.stack ?? scriptError), /intentional script fixture failure/);
assert.match(String(scriptError.stack ?? scriptError), /broken\.js/);
await Bun.sleep(15);
assert.equal(globalThis.__blitsenDisposedTimerRan, undefined,
  "document reload cancels timers owned by the previous context");
const snapshot = JSON.parse(native.runBridgeHarness(
  `<style>#x { display:block; width:100px; height:20px }</style><div id="x">old</div>`,
  `{ if (window !== globalThis || window.document !== document || innerWidth !== 320 || innerHeight !== 180 || devicePixelRatio !== 1)
       throw new Error("window identity, document, or initial viewport failed");
     if ("navigator" in window || "localStorage" in window)
       throw new Error("unsupported browser globals must be omitted");
     if (!(location instanceof Location) || !(history instanceof History))
       throw new Error("client-side routers need location and history");
     for (const absent of ["assign", "replace", "reload", "ancestorOrigins"])
       if (absent in location) throw new Error("document navigation must be absent: " + absent);
     let resizeCount = 0;
     window.addEventListener("resize", () => {
       resizeCount++;
       if (innerWidth !== 640 || innerHeight !== 480)
         throw new Error("resize dispatched before viewport synchronization");
       document.getElementById("x").style.width = "160px";
     });
     __blitsenWindowResize("640", "480");
     if (innerWidth !== 640 || innerHeight !== 480 || devicePixelRatio !== 1)
       throw new Error("window viewport did not synchronize after resize");
     if (resizeCount !== 1) throw new Error("resize did not dispatch exactly once");
     const el = document.querySelector("#x");
     const byId = document.getElementById("x");
     const initial = document.querySelectorAll("#x");
     if (el !== byId || !(initial instanceof NodeList) || initial.length !== 1 || initial.item(0) !== el)
       throw new Error("document lookup or wrapper identity failed");
     const created = document.createElement("section");
     created.id = "created";
     created.appendChild(document.createTextNode("new"));
     const staticList = document.querySelectorAll("section");
     const observer = new MutationObserver(() => {});
     observer.observe(document.body, { childList: true, subtree: true, attributes: true });
     document.body.appendChild(created);
     const mutations = observer.takeRecords();
     observer.disconnect();
     if (mutations.length !== 1 || mutations[0].type !== "childList" ||
         mutations[0].target !== document.body || mutations[0].addedNodes.item(0) !== created)
       throw new Error("MutationObserver child-list record failed");
     if (created.nodeType !== 1 || created.nodeName !== "SECTION" || created.ownerDocument !== document ||
         document.nodeType !== 9 || document.defaultView !== window || !(created instanceof HTMLElement))
       throw new Error("framework DOM identity fields failed");
     if (staticList.length !== 0 || !(document.body instanceof Element) || !(document.documentElement instanceof Element))
       throw new Error("document creation, roots, or static NodeList semantics failed");
     el.textContent = "hi";
     el.setAttribute("class", "done");
     el.setAttribute("data-window", "ok"); }`,
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
assert.match(target.inline_style, /width:\s*160px/);
assert.equal(target.layout.width, 160);
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
assert.deepEqual(animatedFrames.map(node => node.layout.x), [18, 28, 38],
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
     let event;
     target.addEventListener("probe", current => { event = current; }, { once: true });
     if (__blitsenInjectMouseEvent("probe", target, { bubbles: true, cancelable: true }) !== false ||
         !event.defaultPrevented || event.currentTarget !== null || event.eventPhase !== 0)
       throw new Error("injected event result or final state");
     const expected = ["window-capture:1", "document-capture:1", "outer-capture:1",
       "target-capture:2", "target-bubble:2", "outer-bubble:3", "document-bubble:3", "window-bubble:3"];
     if (order.join(",") !== expected.join(",")) throw new Error("propagation order: " + order);

     let once = 0;
     const onceCallback = () => once++;
     target.addEventListener("once", onceCallback, { once: true });
     target.addEventListener("once", onceCallback, { once: false, passive: true });
     __blitsenInjectMouseEvent("once", target); __blitsenInjectMouseEvent("once", target);
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
     __blitsenInjectMouseEvent("mutation", target);
     if (mutation.join(",") !== "first") throw new Error("listener removal/addition during dispatch");
     __blitsenInjectMouseEvent("mutation", target);
     if (mutation.join(",") !== "first,first,added") throw new Error("deferred listener addition");

     const oldError = console.error; let reported = 0; console.error = () => reported++;
     let afterThrow = false;
     target.addEventListener("throw", () => { throw new Error("listener failure"); });
     target.addEventListener("throw", () => { afterThrow = true; });
     __blitsenInjectMouseEvent("throw", target); console.error = oldError;
     if (reported !== 1 || !afterThrow) throw new Error("per-listener exception isolation");

     const stopped = [];
     target.addEventListener("stop", event => { stopped.push("first"); event.stopImmediatePropagation(); });
     target.addEventListener("stop", () => stopped.push("peer"));
     outer.addEventListener("stop", () => stopped.push("ancestor"));
     __blitsenInjectMouseEvent("stop", target, { bubbles: true });
     if (stopped.join(",") !== "first") throw new Error("stopImmediatePropagation");

     const propagation = [];
     target.addEventListener("stop-propagation", event => { propagation.push("first"); event.stopPropagation(); });
     target.addEventListener("stop-propagation", () => propagation.push("peer"));
     outer.addEventListener("stop-propagation", () => propagation.push("ancestor"));
     __blitsenInjectMouseEvent("stop-propagation", target, { bubbles: true });
     if (propagation.join(",") !== "first,peer") throw new Error("stopPropagation");

     let passive;
     target.addEventListener("passive", event => event.preventDefault(), { passive: true });
     target.addEventListener("passive", event => { passive = event; });
     if (!__blitsenInjectMouseEvent("passive", target, { cancelable: true }) || passive.defaultPrevented)
       throw new Error("passive listener");
     let handled = false;
     target.addEventListener("object", { handleEvent(event) { handled = event.currentTarget === target; } });
     __blitsenInjectMouseEvent("object", target);
     if (!handled) throw new Error("object event listener");
     target.setAttribute("data-dispatch", "ok"); }`,
  200,
  100,
));
assert.equal(eventDispatch.nodes.find(node => node.attributes.id === "event-target").attributes["data-dispatch"], "ok");

const keyboardDispatch = JSON.parse(native.runBridgeHarness(
  `<button id="first-key">first</button><button id="second-key">second</button>`,
  `{ const first = document.getElementById("first-key");
     const second = document.getElementById("second-key");
     if (document.activeElement !== document.body) throw new Error("initial activeElement");
     const focusOrder = [];
     first.addEventListener("focus", () => focusOrder.push("first-focus"));
     first.addEventListener("blur", () => focusOrder.push("first-blur"));
     second.addEventListener("focus", () => focusOrder.push("second-focus"));
     first.focus();
     if (document.activeElement !== first || focusOrder.join(",") !== "first-focus") throw new Error("focus()");
     let keyRecord;
     first.addEventListener("keydown", event => {
       keyRecord = [event.key, event.code, event.repeat, event.ctrlKey, event.currentTarget === first];
     });
     __blitsenDispatchKeyboardEvent("keydown", { bubbles: true, cancelable: true,
       key: "a", code: "KeyA", repeat: true, ctrlKey: true });
     if (keyRecord.join(",") !== "a,KeyA,true,true,true") throw new Error("native KeyboardEvent dispatch");
     __blitsenDispatchKeyboardEvent("keydown", { bubbles: true, cancelable: true,
       key: "Tab", code: "Tab", repeat: false });
     if (document.activeElement !== second || focusOrder.join(",") !== "first-focus,first-blur,second-focus")
       throw new Error("Tab focus traversal");
     const cancelTab = event => event.preventDefault();
     second.addEventListener("keydown", cancelTab);
     __blitsenDispatchKeyboardEvent("keydown", { bubbles: true, cancelable: true,
       key: "Tab", code: "Tab", repeat: false, shiftKey: true });
     if (document.activeElement !== second) throw new Error("preventDefault Tab");
     second.removeEventListener("keydown", cancelTab);
     __blitsenDispatchKeyboardEvent("keydown", { bubbles: true, cancelable: true,
       key: "Tab", code: "Tab", repeat: false, shiftKey: true });
     if (document.activeElement !== first) throw new Error("Shift+Tab focus traversal");
     first.blur();
     if (document.activeElement !== document.body) throw new Error("blur()");
     first.setAttribute("data-keyboard", "ok"); }`,
  200,
  80,
));
assert.equal(keyboardDispatch.nodes.find(node => node.attributes.id === "first-key").attributes["data-keyboard"], "ok");

const defaultActions = JSON.parse(native.runBridgeHarness(
  `<style>
     #scroller { width:120px; height:60px; overflow:auto }
     #content { height:300px }
   </style>
   <div id="scroller"><div id="content"><button id="focusable"><span id="click-target">target</span></button></div></div>
   <button id="cancelled"><span id="cancel-target">cancel</span></button>`,
  `{ const clickTarget = document.getElementById("click-target");
     const focusable = document.getElementById("focusable");
     let focusDuringBubble;
     window.addEventListener("click", () => { focusDuringBubble = document.activeElement; });
     __blitsenInjectMouseEvent("click", clickTarget, { bubbles: true, cancelable: true });
     if (focusDuringBubble !== document.body || document.activeElement !== focusable)
       throw new Error("click focus must follow bubbling and choose the nearest focusable ancestor");

     focusable.blur();
     const cancelTarget = document.getElementById("cancel-target");
     cancelTarget.addEventListener("click", event => event.preventDefault());
     __blitsenInjectMouseEvent("click", cancelTarget, { bubbles: true, cancelable: true });
     if (document.activeElement !== document.body) throw new Error("preventDefault click focus");

     __blitsenInjectMouseEvent("wheel", clickTarget,
       { bubbles: true, cancelable: true, deltaY: 40 });
     const cancelWheel = event => event.preventDefault();
     clickTarget.addEventListener("wheel", cancelWheel);
     __blitsenInjectMouseEvent("wheel", clickTarget,
       { bubbles: true, cancelable: true, deltaY: 40 });
     clickTarget.removeEventListener("wheel", cancelWheel);

     focusable.focus();
     __blitsenDispatchKeyboardEvent("keydown", { bubbles: true, cancelable: true,
       key: "ArrowDown", code: "ArrowDown" });
     const cancelKey = event => event.preventDefault();
     focusable.addEventListener("keydown", cancelKey);
     __blitsenDispatchKeyboardEvent("keydown", { bubbles: true, cancelable: true,
       key: "ArrowDown", code: "ArrowDown" });
     focusable.removeEventListener("keydown", cancelKey);
     document.getElementById("scroller").setAttribute("data-defaults", "ok"); }`,
  200,
  120,
));
const defaultScroller = defaultActions.nodes.find(node => node.attributes.id === "scroller");
assert.equal(defaultScroller.attributes["data-defaults"], "ok");
assert.equal(defaultScroller.scroll_y, 80,
  "wheel and keyboard scroll the nearest ancestor while prevented defaults do nothing");

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

const layoutReads = JSON.parse(native.runBridgeHarness(
  `<style>
     #metrics { position:absolute; left:11px; top:13px; box-sizing:content-box;
       width:100px; height:50px; padding:10px; border:5px solid; overflow:auto }
     #overflow { width:300px; height:200px }
   </style>
   <div id="metrics"><div id="overflow"></div></div>`,
  `{ const metrics = document.getElementById("metrics");
     metrics.style.width = "140px";
     const rect = metrics.getBoundingClientRect();
     if (JSON.stringify(rect.toJSON()) !== JSON.stringify({
       x: 19, y: 21, width: 170, height: 80, top: 21, right: 189, bottom: 101, left: 19,
     })) throw new Error("getBoundingClientRect returned stale or incorrect geometry: " + JSON.stringify(rect));
     if (metrics.offsetWidth !== 170 || metrics.offsetHeight !== 80 ||
         metrics.clientWidth !== 160 || metrics.clientHeight !== 70)
       throw new Error("offset/client metrics: " + [metrics.offsetWidth, metrics.offsetHeight,
         metrics.clientWidth, metrics.clientHeight].join(","));
     metrics.scrollLeft = 25;
     metrics.scrollTop = 35;
     if (metrics.scrollLeft !== 25 || metrics.scrollTop !== 35)
       throw new Error("scroll offset get/set");
     metrics.style.width = "150px";
     if (metrics.offsetWidth !== 180) throw new Error("second forced layout returned stale width");
     if (__blitsenForcedLayoutsThisFrame() !== 2)
       throw new Error("forced synchronous layout counter");
     __blitsenAnimationFrameTick(0);
     if (__blitsenForcedLayoutsThisFrame() !== 0)
       throw new Error("forced synchronous layout counter did not reset at the frame boundary");
     metrics.setAttribute("data-layout-reads", "ok"); }`,
  400,
  260,
));
const metricsNode = layoutReads.nodes.find(node => node.attributes.id === "metrics");
assert.equal(metricsNode.attributes["data-layout-reads"], "ok");
assert.equal(metricsNode.scroll_x, 25);
assert.equal(metricsNode.scroll_y, 35);
// Absent, not stubbed. The manifest is generated from the bootstrap source;
// this asks the runtime that source produces, so neither `doctor` nor
// COMPATIBILITY.md can claim an API the application would find otherwise —
// including one the Phase 1 host supplies and the Phase 2 engine would not.
const manifest = JSON.parse(await readFile(
  new URL("../src/api-manifest.json", import.meta.url), "utf8"));
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
for (const entry of manifest.apis)
  assert.equal(runtimeSurface.get(entry.api), entry.status === "implemented",
    `${entry.api} is ${entry.status} in the API manifest but the opposite in the runtime`);

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

// Blitsen's own fetch, not the Phase 1 host's. `runBridgeHarness` installs the
// bridge into this very context, so these are the classes an exported
// application sees — which is also why the probe server is `node:http`:
// `Bun.serve` would be handed the replaced `Response`.
const probe = createServer((request, response) => {
  let body = "";
  request.on("data", chunk => { body += chunk; });
  request.on("end", () => {
    if (request.url === "/missing") {
      response.writeHead(404, { "content-type": "text/plain" });
      response.end("gone");
      return;
    }
    response.writeHead(200, { "content-type": "application/json", "x-probe": "kept" });
    response.end(JSON.stringify({
      method: request.method, sent: body, probe: request.headers["x-probe"] ?? null,
    }));
  });
});
await new Promise(resolve => probe.listen(0, "127.0.0.1", resolve));
const probeOrigin = `http://127.0.0.1:${probe.address().port}`;

try {
  const network = JSON.parse(native.runBridgeHarness(
    `<div id="network"></div>`,
    `{ const results = globalThis.__blitsenNetwork = { settled: [] };
       const headers = new Headers([["X-One", "1"]]);
       headers.append("x-one", "2");
       if (headers.get("X-ONE") !== "1, 2" || !headers.has("x-one") || [...headers].length !== 1)
         throw new Error("Headers case-folding or combination");
       headers.delete("X-One");
       if (headers.get("x-one") !== null || headers.has("x-one")) throw new Error("Headers delete");
       const request = new Request("/reports", { method: "post", headers: { "x-probe": "yes" }, body: "payload" });
       if (request.method !== "POST" || request.url !== "blitsen://app/reports" ||
           request.headers.get("content-type") !== "text/plain;charset=UTF-8" || request.bodyUsed)
         throw new Error("Request normalization: " + request.url);
       let bodylessGet;
       try { new Request("/x", { body: "no" }); } catch (error) { bodylessGet = error.constructor.name; }
       if (bodylessGet !== "TypeError") throw new Error("a GET request must refuse a body");

       const response = new Response("hi", { status: 202, statusText: "Accepted",
         headers: { "content-type": "text/plain" } });
       if (response.status !== 202 || !response.ok || response.statusText !== "Accepted" || response.bodyUsed)
         throw new Error("Response construction");
       // Streaming bodies are not in this tier, so the property is absent and
       // a feature test can branch on it rather than on a null.
       if ("body" in response || "clone" in response || "getSetCookie" in Headers.prototype)
         throw new Error("unimplemented body/cookie surface must be absent");

       const blob = new Blob(["chunk-", "one"], { type: "TEXT/plain" });
       if (blob.size !== 9 || blob.type !== "text/plain") throw new Error("Blob assembly");

       const controller = new AbortController();
       if (controller.signal.aborted || !(controller.signal instanceof AbortSignal))
         throw new Error("AbortController signal");

       Promise.all([
         response.text().then(text => ["response-text", text, response.bodyUsed]),
         response.text().then(() => "re-read", error => ["re-read", error.constructor.name]),
         blob.text().then(text => ["blob-text", text]),
         new Response(new Uint8Array([104, 105])).arrayBuffer().then(buffer => ["bytes", buffer.byteLength]),
         Response.json({ n: 7 }).json().then(value => ["json", value.n]),
         fetch("/local.json").then(() => "resolved", error => ["no-server", String(error.message).includes("no server behind it")]),
         fetch("${probeOrigin}/missing").then(async result => ["missing", result.status, result.ok, await result.text()]),
         fetch("${probeOrigin}/echo", { method: "PUT", headers: { "x-probe": "yes" }, body: "payload" })
           .then(async result => ["echo", result.status, result.headers.get("x-probe"), result.url,
             result.redirected, await result.json()]),
         (() => {
           const aborter = new AbortController();
           const pending = fetch("${probeOrigin}/echo", { signal: aborter.signal });
           aborter.abort();
           return pending.then(() => "resolved", error => ["aborted", error.name, controller.signal.aborted]);
         })(),
         fetch("http://127.0.0.1:1/refused").then(() => "resolved", error => ["refused", error.constructor.name]),
       ]).then(settled => { results.settled = settled; results.done = true; });
       document.getElementById("network").setAttribute("data-network", "ok"); }`,
    200,
    100,
  ));
  assert.equal(network.nodes.find(node => node.attributes.id === "network").attributes["data-network"], "ok");

  // The frame turn is the landing point: nothing settles between turns, however
  // long the worker pool has been finished.
  await Bun.sleep(250);
  assert.equal(globalThis.__blitsenNetwork.done, undefined,
    "network results wait for the frame turn rather than arriving between them");
  assert.equal(globalThis.__blitsenAnimationFramesPending(), true,
    "an in-flight request keeps the host turning so its results can land");
  for (let turn = 0; turn < 200 && !globalThis.__blitsenNetwork.done; turn++) {
    globalThis.__blitsenAnimationFrameTick(0);
    await Bun.sleep(5);
  }
  assert.equal(globalThis.__blitsenAnimationFramesPending(), false,
    "a settled queue stops asking for frames");
  const settled = new Map(globalThis.__blitsenNetwork.settled.map(entry => [entry[0], entry]));
  assert.deepEqual(settled.get("response-text"), ["response-text", "hi", true]);
  assert.deepEqual(settled.get("re-read"), ["re-read", "TypeError"], "a body is readable once");
  assert.deepEqual(settled.get("blob-text"), ["blob-text", "chunk-one"]);
  assert.deepEqual(settled.get("bytes"), ["bytes", 2]);
  assert.deepEqual(settled.get("json"), ["json", 7]);
  assert.deepEqual(settled.get("no-server"), ["no-server", true],
    "a document-relative URL has no server behind it and says so");
  assert.deepEqual(settled.get("missing"), ["missing", 404, false, "gone"]);
  assert.deepEqual(settled.get("echo"),
    ["echo", 200, "kept", `${probeOrigin}/echo`, false, { method: "PUT", sent: "payload", probe: "yes" }]);
  assert.deepEqual(settled.get("aborted"), ["aborted", "AbortError", false],
    "AbortController rejects its own request and no other");
  assert.deepEqual(settled.get("refused"), ["refused", "TypeError"]);
  delete globalThis.__blitsenNetwork;
} finally {
  probe.close();
}

console.log("bridge harness passed", process.platform, process.arch, `style=${styled.attributes["data-style-call-us"]}us/call`);
