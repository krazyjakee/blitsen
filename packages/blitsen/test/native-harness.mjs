import { strict as assert } from "node:assert";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
import { createRequire } from "node:module";
import { homedir, tmpdir } from "node:os";
import { basename, isAbsolute, join } from "node:path";

import { loadApiManifest } from "../src/api-manifest.mjs";
// The specifier layer an application imports, exercised against the namespace
// the addon installs into this realm rather than against a stand-in.
import app from "../src/native/app.mjs";
import clipboard from "../src/native/clipboard.mjs";
import dialog from "../src/native/dialog.mjs";
import windowModule from "../src/native/window.mjs";

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
     if ("indexedDB" in window || "IntersectionObserver" in window)
       throw new Error("unsupported browser globals must be omitted");
     if (!(navigator instanceof Navigator) || !(localStorage instanceof Storage))
       throw new Error("the host's navigator and storage must be replaced by Blitsen's own");
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

// Form controls. The whole of this is the attribute/property distinction: the
// attribute is the control's default and the property is its current state, so
// each half of every pair below is asserted to move without the other.
const formControls = JSON.parse(native.runBridgeHarness(
  `<form id="form">
     <input id="text" name="who" value="start">
     <input id="box" type="checkbox" checked>
     <input id="radio-a" type="radio" name="pick" checked><input id="radio-b" type="radio" name="pick">
     <textarea id="notes">typed in</textarea>
     <select id="choice"><option id="first" value="a">A</option>
       <option id="second" value="b" selected>B</option></select>
     <button id="send" type="submit" value="go">Send</button>
   </form>`,
  `{ const expect = (actual, wanted, what) => {
       if (JSON.stringify(actual) !== JSON.stringify(wanted))
         throw new Error(what + ": " + JSON.stringify(actual) + " is not " + JSON.stringify(wanted));
     };
     const byId = id => document.getElementById(id);

     const text = byId("text");
     expect(text.value, "start", "value starts at the attribute");
     expect(text.defaultValue, "start", "defaultValue is the attribute");
     text.value = "edited";
     expect(text.value, "edited", "the property holds what was assigned");
     expect(text.getAttribute("value"), "start", "assigning value must not write the attribute");
     expect(text.defaultValue, "start", "defaultValue still reads the attribute");
     text.setAttribute("value", "new default");
     expect(text.value, "edited", "a later attribute write must not clobber the value");
     expect(text.defaultValue, "new default", "defaultValue follows the attribute");
     expect([text.type, text.name, text.disabled, text.form === byId("form")],
       ["text", "who", false, true], "the reflected basics");

     const box = byId("box");
     expect([box.checked, box.defaultChecked], [true, true], "checked starts at the attribute");
     box.checked = false;
     expect([box.checked, box.hasAttribute("checked"), box.defaultChecked], [false, true, true],
       "checkedness and the checked attribute diverge");
     box.removeAttribute("checked");
     expect([box.checked, box.defaultChecked], [false, false], "removing the default leaves the state");
     box.setAttribute("checked", "");
     expect([box.checked, box.defaultChecked], [false, true], "restoring the default leaves the state");

     byId("radio-b").checked = true;
     expect(byId("radio-a").checked, false, "a radio group has one member checked");

     const notes = byId("notes");
     expect([notes.value, notes.defaultValue], ["typed in", "typed in"],
       "a textarea's child text is its value and its default");
     notes.value = "rewritten";
     expect([notes.value, notes.defaultValue, notes.textContent],
       ["rewritten", "typed in", "typed in"], "a textarea's value leaves its children alone");

     const choice = byId("choice");
     const second = byId("second");
     expect([choice.options.length, choice.length], [2, 2], "options is a collection of the options");
     expect([choice.value, choice.selectedIndex], ["b", 1], "the select reads its selected option");
     expect([choice.options[0].index, second.index], [0, 1], "an option's index is its position");
     expect([choice.options[0].text, choice.options[0].value], ["A", "a"], "option text and value");
     choice.value = "a";
     expect([choice.value, choice.selectedIndex, choice.selectedOptions.length], ["a", 0, 1],
       "assigning the select's value moves the selection");
     expect([second.selected, second.hasAttribute("selected"), second.defaultSelected],
       [false, true, true], "selectedness and the selected attribute diverge");
     expect(choice.querySelector(":checked") === choice.options[0], true,
       "the selected option is the one :checked matches");
     const added = document.createElement("option");
     added.value = "c";
     choice.appendChild(added);
     expect(choice.options.length, 3, "a re-read of options sees what was added");

     const form = byId("form");
     expect(form.elements.length, 7, "form.elements lists the controls it owns");
     expect("submit" in form, false, "the navigating half of submission stays absent");
     let submits = 0;
     let submitters = [];
     form.addEventListener("submit", event => {
       submits++;
       submitters.push(event.submitter && event.submitter.id);
       event.preventDefault();
     });
     form.requestSubmit();
     __blitsenInjectMouseEvent("click", byId("send"), { bubbles: true, cancelable: true });
     expect([submits, submitters], [2, [null, "send"]],
       "requestSubmit and a submit button both raise a cancelable submit event");
     expect([byId("send").value, byId("send").type], ["go", "submit"], "a button's value and type");

     // The legacy event factory, in the shape Svelte's custom_event helper uses.
     const legacy = document.createEvent("CustomEvent");
     legacy.initCustomEvent("ping", true, true, { n: 7 });
     let detail = null;
     form.addEventListener("ping", event => { detail = event.detail.n; });
     form.dispatchEvent(legacy);
     expect([legacy.type, legacy.bubbles, detail], ["ping", true, 7],
       "createEvent + initCustomEvent produce a dispatchable event carrying its detail");
     let refused;
     try { document.createEvent("MouseEvents"); } catch (error) { refused = error.name; }
     expect(refused, "NotSupportedError", "an interface the factory does not answer is refused");

     // A control's own activation runs only when the click was not cancelled.
     const cancel = event => event.preventDefault();
     box.addEventListener("click", cancel);
     __blitsenInjectMouseEvent("click", box, { bubbles: true, cancelable: true });
     expect(box.checked, false, "a cancelled click does not toggle the checkbox");
     box.removeEventListener("click", cancel);
     __blitsenInjectMouseEvent("click", box, { bubbles: true, cancelable: true });
     expect(box.checked, true, "clicking a checkbox toggles it");

     form.setAttribute("data-form-controls", "ok"); }`,
  400,
  300,
));
assert.equal(formControls.nodes.find(node => node.attributes.id === "form")
  .attributes["data-form-controls"], "ok");

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

// The surface real framework builds reach for on their first render: node kinds
// the HTML parser makes but `createElement` cannot, element-scoped selection,
// and fragments. Asserted by behaviour, because presence is what the manifest
// check above already covers.
const domSurface = JSON.parse(native.runBridgeHarness(
  `<div id="surface"><span class="child" data-role="one">A</span><b>B</b></div>
   <template id="tpl"><tr><td>cell</td></tr><!--anchor--><span class="cloned">clone</span></template>`,
  `{ const expect = (condition, message) => { if (!condition) throw new Error(message); };
     const root = document.getElementById("surface");
     const span = root.querySelector(".child");
     expect(span === root.children[0] && root.querySelectorAll("span, b").length === 2 &&
       root.querySelector("#surface") === null, "element-scoped selection excludes its own scope");
     expect(span.matches(".child") && !span.matches("#surface"), "matches");
     expect(span.closest("#surface") === root && span.closest("nav") === null, "closest");
     expect(root.children.length === 2 && root.contains(span) && !span.contains(root) &&
       span.parentElement === root && root.lastChild.previousSibling === span, "tree walks");
     span.dataset.moreInfo = "two";
     expect(span.dataset.role === "one" && span.getAttribute("data-more-info") === "two" &&
       Object.keys(span.dataset).join() === "role,moreInfo" && "moreInfo" in span.dataset,
       "dataset maps data- attributes both ways");

     const comment = document.createComment("v-if");
     expect(comment.nodeType === 8 && comment.nodeName === "#comment" && comment instanceof Comment &&
       comment.textContent === "v-if", "comment node");
     root.appendChild(comment);
     expect(root.innerHTML.endsWith("<!--v-if-->") && root.childNodes.length === 3 &&
       root.children.length === 2, "a comment is in the tree but is not an element");
     let refusedComment;
     try { document.createComment("a-->b"); } catch (error) { refusedComment = true; }
     expect(refusedComment, "comment data that would close the comment early is refused");

     const svg = document.createElementNS("http://www.w3.org/2000/svg", "linearGradient");
     svg.id = "gradient";
     expect(svg.namespaceURI === "http://www.w3.org/2000/svg" && svg.tagName === "linearGradient" &&
       svg instanceof SVGElement, "SVG elements keep their namespace and their case");
     expect(document.createElement("DIV").tagName === "DIV" &&
       document.createElement("div").namespaceURI === "http://www.w3.org/1999/xhtml", "HTML folds case");
     root.appendChild(svg);

     const template = document.getElementById("tpl");
     const content = template.content;
     expect(template instanceof HTMLTemplateElement && content instanceof DocumentFragment &&
       content.nodeType === 11 && template.childNodes.length === 0,
       "template contents belong to the fragment, not to the element");
     expect(content.childNodes.length === 3 && content.querySelector("td").textContent === "cell",
       "a template parses children an ordinary element would discard");
     const clone = content.cloneNode(true);
     const cloned = clone.querySelector(".cloned");
     expect(clone !== content && cloned !== content.querySelector(".cloned") &&
       clone.childNodes.length === 3, "a fragment clones deeply and independently");
     root.appendChild(clone);
     expect(clone.childNodes.length === 0 && cloned.parentNode === root &&
       content.childNodes.length === 3, "inserting a fragment moves its children and spares the source");

     const fragment = document.createDocumentFragment();
     fragment.appendChild(document.createElement("i"));
     fragment.appendChild(document.createTextNode("tail"));
     const observer = new MutationObserver(() => {});
     observer.observe(root, { childList: true });
     root.appendChild(fragment);
     const records = observer.takeRecords();
     observer.disconnect();
     expect(records.length === 1 && records[0].addedNodes.length === 2,
       "a fragment insertion reports the nodes that actually moved");
     expect(root.lastChild.nodeValue === "tail", "nodeValue reads text data");
     root.lastChild.nodeValue = "changed";
     expect(root.textContent.endsWith("changed"), "nodeValue writes text data");
     const anchor = root.lastChild;
     anchor.before(document.createElement("u"));
     expect(anchor.previousSibling.tagName === "U", "before() inserts against a sibling");

     // Without this, Vite's module-preload polyfill installs itself and fetches
     // every chunk over an address that has no server behind it.
     const link = document.createElement("link");
     expect(link instanceof HTMLLinkElement && link.relList.supports("modulepreload") &&
       !link.relList.supports("not-a-relation"), "link.relList reports the keywords it knows");
     link.rel = "modulepreload";
     link.href = "assets/chunk.js";
     expect(link.relList.contains("modulepreload") && link.getAttribute("rel") === "modulepreload" &&
       link.href === "blitsen://app/assets/chunk.js", "rel and href reflect their attributes");
     let tokenError;
     try { root.classList.supports("x"); } catch (error) { tokenError = error.constructor.name; }
     expect(tokenError === "TypeError", "a token list with no keyword set refuses supports()");

     expect(localStorage.getItem("absent") === null, "an unset key reads as null, not undefined");
     localStorage.setItem("theme", "dark");
     localStorage.count = 2;
     expect(localStorage.getItem("count") === "2" && localStorage.theme === "dark" &&
       localStorage.length === 2 && Object.keys(localStorage).join() === "theme,count",
       "both access forms reach one store");
     localStorage.removeItem("theme");
     expect(localStorage.length === 1 && sessionStorage.getItem("count") === null,
       "the two storage areas are separate");
     expect(navigator.userAgent.startsWith("Blitsen/") && navigator.platform.length > 0 &&
       navigator.languages[0] === navigator.language, "navigator states this machine's identity");
     for (const capability of ["clipboard", "geolocation", "mediaDevices", "serviceWorker",
       "sendBeacon", "userAgentData", "onLine", "storage", "permissions", "cookieEnabled"])
       if (capability in navigator) throw new Error("navigator claims capability: " + capability);
     root.setAttribute("data-dom-surface", "ok"); }`,
  320,
  180,
));
const surfaceNodes = new Map(domSurface.nodes.map(node => [node.attributes.id, node]));
assert.equal(surfaceNodes.get("surface").attributes["data-dom-surface"], "ok");
assert(surfaceNodes.has("gradient"), "the namespaced element reached the Rust tree");
assert.equal(domSurface.nodes.filter(node => node.attributes.class === "cloned").length, 1,
  "the clone reached the Rust tree and its source stayed in the detached fragment");

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

// Traversal, class selection, the namespaced attribute half, the variadic
// insertion methods and the reads that go with them — the surface enumerated
// against the live runtime in issue #115. Behaviour, not presence: presence is
// what the manifest check below covers.
const surfaceGaps = JSON.parse(native.runBridgeHarness(
  `<style>#tree { display:block; width:200px; height:40px }</style>
   <div id="tree">head<span class="leaf tall" id="one">1</span>between<span class="leaf" id="two">2</span>tail</div>`,
  `{ const expect = (condition, message) => { if (!condition) throw new Error(message); };
     const ids = list => [...list].map(node => node.id).join();
     const tree = document.getElementById("tree");
     const one = document.getElementById("one");
     const two = document.getElementById("two");
     expect(tree.childNodes.length === 5 && tree.childElementCount === 2,
       "childElementCount counts elements, not nodes");
     expect(tree.firstElementChild === one && tree.lastElementChild === two &&
       tree.firstChild !== one && tree.lastChild !== two,
       "first and lastElementChild skip the text around them");
     expect(one.nextElementSibling === two && two.previousElementSibling === one &&
       one.previousElementSibling === null && two.nextElementSibling === null,
       "element siblings skip the text nodes between them");

     expect(ids(tree.getElementsByClassName("leaf")) === "one,two" &&
       ids(tree.getElementsByClassName("tall")) === "one" &&
       ids(tree.getElementsByClassName("leaf tall")) === "one",
       "one class among several, and only the elements carrying every class asked for");
     expect(ids(document.getElementsByClassName("leaf")) === "one,two" &&
       ids(one.getElementsByClassName("leaf")) === "",
       "document scope reaches the same elements, element scope excludes itself");
     two.className = "leaf tall";
     expect(ids(tree.getElementsByClassName("tall")) === "one,two",
       "a re-query sees the mutation");
     two.className = "leaf";

     const XLINK = "http://www.w3.org/1999/xlink";
     const use = document.createElementNS("http://www.w3.org/2000/svg", "use");
     use.id = "used";
     tree.appendChild(use);
     use.setAttributeNS(XLINK, "xlink:href", "#glyph");
     expect(use.getAttributeNS(XLINK, "href") === "#glyph" && use.getAttribute("href") === null &&
       use.getAttributeNS(null, "href") === null,
       "a namespaced attribute round-trips and is not the null-namespace one of that name");
     // Only the any-namespace form is asserted: Blitz matches an attribute
     // selector on the local name whatever namespace it is in, so a plain
     // [href] would match this too and would prove nothing about the namespace.
     expect(document.querySelector("[*|href]") === use,
       "the attribute reached the selector engine, not just the read back");
     use.setAttributeNS(null, "width", "10");
     expect(use.getAttribute("width") === "10" && use.getAttributeNS(null, "width") === "10",
       "the null namespace is the space the plain accessors already use");
     use.removeAttributeNS(XLINK, "href");
     expect(use.getAttributeNS(XLINK, "href") === null && use.getAttribute("width") === "10",
       "removeAttributeNS removes the namespaced attribute and only that one");

     const bare = document.createElement("i");
     expect(!bare.hasAttributes() && bare.getAttributeNames().length === 0,
       "an element with no attributes says so");
     bare.setAttribute("title", "t");
     bare.setAttribute("data-x", "1");
     expect(bare.hasAttributes() && bare.getAttributeNames().join() === "title,data-x",
       "attribute names come back in document order");
     expect(bare.toggleAttribute("hidden") === true && bare.getAttribute("hidden") === "" &&
       bare.toggleAttribute("hidden") === false && !bare.hasAttribute("hidden"),
       "toggleAttribute flips and reports the state it left");
     expect(bare.toggleAttribute("hidden", false) === false && !bare.hasAttribute("hidden") &&
       bare.toggleAttribute("hidden", true) === true &&
       bare.toggleAttribute("hidden", true) === true && bare.hasAttribute("hidden"),
       "force pins the state rather than flipping it");

     const box = document.createElement("div");
     box.id = "box";
     tree.appendChild(box);
     box.append("a", document.createElement("b"), "c");
     expect(box.childNodes.length === 3 && box.childElementCount === 1 &&
       box.firstChild.nodeType === 3 && box.textContent === "ac",
       "append takes strings as text nodes and elements as themselves");
     box.prepend(document.createElement("u"), "z");
     expect(box.childNodes.length === 5 && box.firstElementChild.tagName === "U" &&
       box.childNodes[1].textContent === "z", "prepend inserts at the front, in order");
     box.replaceChildren();
     expect(box.childNodes.length === 0, "replaceChildren with nothing empties");
     box.replaceChildren(document.createElement("i"), "tail");
     expect(box.childNodes.length === 2 && box.lastChild.textContent === "tail",
       "and then fills");
     expect(box.outerHTML === '<div id="box"><i></i>tail</div>' &&
       box.innerHTML === '<i></i>tail', "outerHTML serializes the element itself");

     box.insertAdjacentHTML("afterbegin", "<em>first</em>");
     box.insertAdjacentHTML("beforeend", "<s>last</s>");
     expect(box.outerHTML === '<div id="box"><em>first</em><i></i>tail<s>last</s></div>',
       "insertAdjacentHTML parses into the element at both ends");
     box.firstElementChild.insertAdjacentHTML("beforebegin", "<q>before</q>");
     box.lastElementChild.insertAdjacentHTML("afterend", "<q>after</q>");
     expect(box.firstElementChild.tagName === "Q" && box.lastElementChild.textContent === "after" &&
       box.childElementCount === 5, "and against a sibling on either side of one");
     const row = document.createElement("tr");
     row.insertAdjacentHTML("beforeend", "<td>cell</td>");
     expect(row.childElementCount === 1 && row.firstElementChild.tagName === "TD",
       "parsed in the element it lands in, which is what keeps a table cell");

     const map = box.attributes;
     expect(map.length === 1 && map[0].name === "id" && map[0].value === "box" &&
       map[0].ownerElement === box && map[0].namespaceURI === null &&
       map.item(1) === null && [...map].length === 1, "attributes is a NamedNodeMap over the element");
     expect(map.getNamedItem("ID") === map[0] && map.getNamedItem("class") === null,
       "getNamedItem folds case in the null namespace and answers null for an absent one");
     map[0].value = "renamed";
     expect(box.id === "renamed" && map[0].value === "renamed" && box.attributes.length === 1,
       "an attribute node writes through to the element and reads back through it");
     box.id = "box";
     expect(use.attributes.length === 2 &&
       use.attributes.getNamedItemNS(XLINK, "href") === null &&
       use.getAttributeNames().join() === "id,width",
       "a removed namespaced attribute is gone from the map as well");
     use.setAttributeNS(XLINK, "xlink:href", "#glyph");
     const namespaced = use.attributes.getNamedItemNS(XLINK, "href");
     expect(namespaced.namespaceURI === XLINK && namespaced.value === "#glyph" &&
       use.attributes.getNamedItem("href") === null,
       "the map discriminates by namespace exactly as the accessors do");

     expect(tree.getRootNode() === document && box.getRootNode() === document,
       "a connected node's root is the document");
     const detached = document.createElement("div");
     const nested = document.createElement("span");
     detached.appendChild(nested);
     expect(nested.getRootNode() === detached && detached.getRootNode() === detached,
       "a detached node's root is the top of its own tree");

     const paragraph = document.createElement("p");
     paragraph.id = "paragraph";
     tree.appendChild(paragraph);
     paragraph.append("a", "b");
     paragraph.appendChild(document.createComment("gap"));
     paragraph.append("c", "", "d");
     expect(paragraph.childNodes.length === 6, "adjacent text nodes start out separate");
     paragraph.normalize();
     expect(paragraph.childNodes.length === 3 && paragraph.childNodes[0].textContent === "ab" &&
       paragraph.childNodes[1].nodeType === 8 && paragraph.childNodes[2].textContent === "cd",
       "normalize merges adjacent text, drops the empty, and does not merge across a comment");

     __blitsenAnimationFrameTick(0);
     tree.style.width = "180px";
     const rects = tree.getClientRects();
     expect(rects.length === 1 && __blitsenForcedLayoutsThisFrame() === 1,
       "getClientRects is charged as the forced layout it is");
     const bounds = tree.getBoundingClientRect();
     expect(rects[0].x === bounds.x && rects[0].y === bounds.y && rects[0].width === 180 &&
       rects[0].height === bounds.height && __blitsenForcedLayoutsThisFrame() === 1,
       "the border box getBoundingClientRect reports, off one settled layout");
     tree.setAttribute("data-surface-gaps", "ok"); }`,
  320,
  180,
));
const gapNodes = new Map(surfaceGaps.nodes.map(node => [node.attributes.id, node]));
assert.equal(gapNodes.get("tree").attributes["data-surface-gaps"], "ok");
assert.equal(gapNodes.get("used").attributes.width, "10",
  "the namespaced element kept the null-namespace attribute written through setAttributeNS");
assert.equal(gapNodes.get("box").attributes.id, "box",
  "the element filled by replaceChildren reached the Rust tree");
assert.equal(gapNodes.get("paragraph").text_content, "abcd",
  "the normalized text is one run in the authoritative tree");

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

// Read-back style: the cascade, the device and element geometry, asked from
// JavaScript. Asserted by what each answers, not by whether it exists — the
// manifest check below already covers presence.
const readBack = JSON.parse(native.runBridgeHarness(
  `<style>
     :root { --brand: #123456 }
     #resolved { display:block; width:50%; height:20px; padding:4px; border:2px solid;
       color:rgb(1,2,3) }
     #resolved.hot { color:rgb(9,9,9); height:44px }
     #observed { display:block; width:60px; height:30px; padding:5px; border:1px solid }
   </style>
   <div id="resolved">t</div><div id="observed"></div>`,
  `{ const expect = (condition, message) => { if (!condition) throw new Error(message); };
     const element = document.getElementById("resolved");
     const style = getComputedStyle(element);
     expect(style instanceof CSSStyleDeclaration && getComputedStyle(element) === style,
       "a computed declaration is a CSSStyleDeclaration and is stable per element");
     // Nothing here was ever an inline declaration: this is the stylesheet
     // resolved by Blitz, which element.style cannot see.
     expect(element.style.color === "" && style.color === "rgb(1, 2, 3)" &&
       style.getPropertyValue("color") === "rgb(1, 2, 3)", "resolved value: " + style.color);
     expect(style.getPropertyValue("--brand") === "#123456" &&
       style.getPropertyValue("--unset") === "",
       "a custom property resolves through inheritance: " + style.getPropertyValue("--brand"));
     // 320px viewport less the body's 8px margins: a percentage becomes the
     // used value, which only layout knows.
     expect(style.width === "152px" && style.height === "20px", "used box size: " + style.width);
     expect(style.getPropertyValue("padding") === "4px" && style.margin === "0px",
       "shorthands serialize from their longhands: " + style.getPropertyValue("padding"));
     expect(style.getPropertyValue("not-a-property") === "" &&
       getComputedStyle(document.createElement("div")).color === "",
       "an unknown property and an element the cascade never reached read as absent");
     element.classList.add("hot");
     expect(style.color === "rgb(9, 9, 9)" && style.height === "44px",
       "a class mutation changes what the same declaration resolves to: " + style.color);
     expect(style.cssText === "", "a computed declaration block serializes as nothing");
     for (const [operation, message] of [
       [() => style.setProperty("color", "red"), "setProperty"],
       [() => { style.color = "red"; }, "assignment"],
     ]) {
       let refused;
       try { operation(); } catch (error) { refused = error.name; }
       if (refused !== "NoModificationAllowedError") throw new Error("read-only: " + message);
     }
     let notElement, pseudo;
     try { getComputedStyle(document.createTextNode("x")); } catch (error) { notElement = error.constructor.name; }
     try { getComputedStyle(element, "::before"); } catch (error) { pseudo = error.name; }
     expect(notElement === "TypeError" && pseudo === "NotSupportedError",
       "a non-element and a pseudo-element are refused rather than answered");

     expect(matchMedia("(prefers-color-scheme: light)").matches &&
       !matchMedia("(prefers-color-scheme: dark)").matches, "the window's colour scheme");
     const unknownFeature = matchMedia("(prefers-reduced-motion: reduce)");
     const invalid = matchMedia("!!!");
     expect(!unknownFeature.matches && !invalid.matches && invalid.media === "not all",
       "an unknown feature does not match and an invalid query serializes as not all");
     const query = matchMedia("(min-width: 500px)");
     expect(query instanceof MediaQueryList && query.media === "(min-width: 500px)" &&
       !query.matches, "the viewport is 320px wide");
     const changes = [];
     query.addEventListener("change", event => changes.push(["listener", event.matches, event.media]));
     query.onchange = event => changes.push(["onchange", event.matches]);
     query.addListener(event => changes.push(["legacy", event instanceof MediaQueryListEvent]));
     __blitsenWindowResize("640", "480");
     __blitsenAnimationFrameTick(0);
     expect(query.matches, "a resize re-evaluates the query");
     expect(JSON.stringify(changes) === JSON.stringify([["listener", true, "(min-width: 500px)"],
       ["onchange", true], ["legacy", true]]), "change delivery: " + JSON.stringify(changes));
     __blitsenAnimationFrameTick(16);
     expect(changes.length === 3, "a query that did not flip dispatches nothing");

     const observed = document.getElementById("observed");
     const sizes = [];
     const observer = new ResizeObserver(entries => sizes.push(entries.map(entry =>
       [entry.target === observed, entry.contentRect.x, entry.contentRect.width,
        entry.contentRect.height, entry.borderBoxSize[0].inlineSize,
        entry.contentBoxSize[0].blockSize])));
     let badTarget, badBox;
     try { observer.observe(document.createTextNode("x")); } catch (error) { badTarget = error.constructor.name; }
     try { observer.observe(observed, { box: "device-pixel-content-box" }); }
     catch (error) { badBox = error.constructor.name; }
     expect(badTarget === "TypeError" && badBox === "TypeError",
       "a non-element target and an unreportable box are refused");
     expect(!__blitsenAnimationFramesPending(), "nothing is owed before observing");
     observer.observe(observed);
     expect(__blitsenAnimationFramesPending(),
       "an unreported observation keeps the host turning until it is delivered");
     __blitsenAnimationFrameTick(32);
     expect(!__blitsenAnimationFramesPending(), "a delivered observation owes nothing");
     __blitsenAnimationFrameTick(48);
     observed.style.width = "100px";
     __blitsenAnimationFrameTick(64);
     observer.unobserve(observed);
     observed.style.width = "20px";
     __blitsenAnimationFrameTick(80);
     expect(JSON.stringify(sizes) === JSON.stringify([
       [[true, 6, 60, 30, 72, 30]], [[true, 6, 100, 30, 112, 30]],
     ]), "resize delivery: " + JSON.stringify(sizes));
     observer.disconnect();
     element.setAttribute("data-read-back", "ok"); }`,
  320,
  180,
));
assert.equal(readBack.nodes.find(node => node.attributes.id === "resolved").attributes["data-read-back"], "ok");

// The CSSOM stylesheet surface, driven the way Svelte drives it: an empty
// <style> appended to the head, a @keyframes block inserted into its sheet, and
// `animation` set on the element. What is asserted is the cascade's answer and
// the painted frame — a rule that parsed into a shadow list and never reached
// Stylo would pass every structural check here and fail both of those.
const stylesheets = JSON.parse(native.runBridgeHarness(
  `<style id="authored">#box { display:block; width:120px; height:60px; background:rgb(9,9,9) }</style>
   <div id="box"></div>`,
  `{ const expect = (condition, message) => { if (!condition) throw new Error(message); };
     const box = document.getElementById("box");
     const authored = document.getElementById("authored");
     const resolved = property => getComputedStyle(box).getPropertyValue(property);
     expect(authored instanceof HTMLStyleElement && authored.sheet instanceof CSSStyleSheet &&
       authored.sheet === authored.sheet && authored.sheet.ownerNode === authored,
       "a <style> element has one stable sheet that knows the element it came from");
     expect(authored.sheet.cssRules instanceof CSSRuleList &&
       authored.sheet.cssRules.length === 1 &&
       authored.sheet.cssRules[0] instanceof CSSRule &&
       authored.sheet.cssRules[0].cssText.includes("#box") &&
       authored.sheet.cssRules[0].parentStyleSheet === authored.sheet,
       "the sheet reports the rule the document parsed: " + authored.sheet.cssRules.length);
     expect(document.styleSheets instanceof StyleSheetList &&
       document.styleSheets.length === 1 && document.styleSheets[0] === authored.sheet,
       "the document lists the sheets the cascade is reading");
     let constructed;
     try { new CSSStyleSheet(); } catch (error) { constructed = error.constructor.name; }
     expect(constructed === "TypeError",
       "a constructible sheet cannot reach the cascade, so it is refused rather than ignored");

     const style = document.createElement("style");
     expect(style.sheet === null, "a disconnected <style> element has no sheet");
     document.head.appendChild(style);
     const sheet = style.sheet;
     expect(document.styleSheets.length === 2 && document.styleSheets[1] === sheet,
       "an appended <style> element joins the document's sheets");

     const index = sheet.insertRule("#box { background: rgb(4, 200, 8) }", sheet.cssRules.length);
     expect(index === 0 && sheet.cssRules.length === 1,
       "insertRule answers with the index it inserted at, and the next read sees the rule");
     expect(resolved("background-color") === "rgb(4, 200, 8)",
       "an inserted rule is in the cascade: " + resolved("background-color"));
     sheet.deleteRule(0);
     expect(sheet.cssRules.length === 0 && resolved("background-color") === "rgb(9, 9, 9)",
       "a deleted rule leaves the cascade: " + resolved("background-color"));

     let refused, ranged, external;
     try { sheet.insertRule("this is not a rule", 0); } catch (error) { refused = error.name; }
     try { sheet.insertRule("#box { color: red }", 4); } catch (error) { ranged = error.name; }
     try { document.styleSheets[0].deleteRule(9); } catch (error) { external = error.name; }
     expect(refused === "SyntaxError" && ranged === "IndexSizeError" &&
       external === "IndexSizeError",
       "refusals: " + [refused, ranged, external].join(","));
     expect(sheet.cssRules.length === 0, "nothing refused reached the sheet");

     // Svelte's own teardown: the sheet is dropped by detaching its ownerNode.
     const scratch = document.createElement("style");
     document.head.appendChild(scratch);
     scratch.sheet.insertRule("#box { outline: 1px solid red }", 0);
     scratch.sheet.ownerNode.parentNode.removeChild(scratch);
     expect(document.styleSheets.length === 2 && resolved("outline-color") !== "rgb(255, 0, 0)",
       "detaching a sheet's owner takes its rules out of the cascade");

     sheet.insertRule("@keyframes __blitsen_fade { 0% { background: rgb(200, 0, 0) }" +
       " 100% { background: rgb(0, 0, 200) } }", 0);
     box.style.animation = "__blitsen_fade 1000ms linear 0ms 1 both";
     // The clock the cascade samples animations at is the frame's timestamp, and
     // it only moves when a frame is delivered. The first laid-out frame is when
     // the animation starts, so it has to happen at the timestamp it starts from.
     __blitsenAnimationFrameTick(0);
     expect(resolved("background-color") === "rgb(200, 0, 0)",
       "the first frame is the animation's first keyframe: " + resolved("background-color"));
     expect(__blitsenAnimationFramesPending(),
       "a running animation keeps the host turning: the clock only moves on a frame");
     __blitsenAnimationFrameTick(500);
     expect(resolved("background-color") === "rgb(100, 0, 100)",
       "half way through, the cascade interpolates: " + resolved("background-color"));
     box.setAttribute("data-stylesheets", "ok"); }`,
  320,
  180,
));
assert.equal(stylesheets.nodes.find(node => node.attributes.id === "box").attributes["data-stylesheets"],
  "ok");
// The painted frame, not the resolved value: the harness renders after the
// script, with the clock left half way through the inserted animation.
const halfway = stylesheets.paint_colors.find(color => color.rgba === "#640064ff");
assert(halfway?.pixels > 5_000,
  `a rule inserted from JavaScript animates in the painted frame: ${
    JSON.stringify(stylesheets.paint_colors)}`);

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

// Images. The decoded size and the load/error pair are what an application
// polls and waits on, so this asserts the outcome of a real 8x4 PNG and of
// bytes that are not an image at all — both delivered at the frame boundary,
// neither delivered retroactively to a listener that arrived too late.
const SWATCH = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAgAAAAECAYAAACzzX7wAAAA"
  + "F0lEQVR42mO4IyLyHxmLaNxBwQy0VwAAw8RBoVkySsgAAAAASUVORK5CYII=";
const BROKEN = "data:image/png;base64,bm90IGFuIGltYWdl";
const images = JSON.parse(native.runBridgeHarness(
  `<img id="parsed" src="${SWATCH}">`,
  `{ const expect = (condition, message) => { if (!condition) throw new Error(message); };
     const seen = [];
     const parsed = document.getElementById("parsed");
     expect(parsed instanceof HTMLImageElement && parsed instanceof HTMLElement &&
       parsed instanceof Element, "an <img> wrapper is an HTMLImageElement");
     expect(parsed.src === ${JSON.stringify(SWATCH)}, "src reflects the resolved source: " + parsed.src);
     expect(parsed.complete && parsed.naturalWidth === 8 && parsed.naturalHeight === 4,
       "a decoded image reports its intrinsic size: " +
       [parsed.complete, parsed.naturalWidth, parsed.naturalHeight]);
     // The image settled before this script ran; nothing is owed to a listener
     // that arrives afterwards, which is the question complete answers.
     parsed.addEventListener("load", () => seen.push("retroactive"));
     parsed.onerror = () => seen.push("retroactive-error");
     expect(!__blitsenAnimationFramesPending(), "a settled image owes the host nothing");
     __blitsenAnimationFrameTick(0);
     expect(seen.length === 0, "a listener attached after completion receives nothing: " + seen);

     const bare = new Image();
     const sized = new Image(24, 12);
     expect(sized instanceof Image && sized instanceof HTMLImageElement && sized.tagName === "IMG",
       "new Image() constructs an img element");
     expect(sized.getAttribute("width") === "24" && sized.getAttribute("height") === "12" &&
       !bare.hasAttribute("width") && !bare.hasAttribute("height"),
       "the constructor arguments are the width and height attributes");
     expect(bare.complete && bare.naturalWidth === 0 && bare.naturalHeight === 0,
       "an image with no source has nothing to wait for");

     sized.onload = event => seen.push(["load", event.type, sized.naturalWidth, sized.naturalHeight]);
     sized.onerror = () => seen.push("wrong-error");
     sized.src = ${JSON.stringify(SWATCH)};
     document.body.appendChild(sized);
     expect(seen.length === 0, "load is not delivered synchronously");
     expect(__blitsenAnimationFramesPending(), "an image in flight keeps the host turning");
     __blitsenAnimationFrameTick(16);
     expect(JSON.stringify(seen) === JSON.stringify([["load", "load", 8, 4]]),
       "a loaded image fires load once, with its size readable: " + JSON.stringify(seen));
     expect(!__blitsenAnimationFramesPending(), "a settled image stops holding the host open");
     __blitsenAnimationFrameTick(32);
     expect(seen.length === 1, "load is delivered exactly once");

     const broken = new Image();
     broken.onload = () => seen.push("wrong-load");
     broken.addEventListener("error",
       event => seen.push([event.type, broken.complete, broken.naturalWidth, broken.naturalHeight]));
     document.body.appendChild(broken);
     broken.setAttribute("src", ${JSON.stringify(BROKEN)});
     __blitsenAnimationFrameTick(48);
     expect(JSON.stringify(seen[1]) === JSON.stringify(["error", true, 0, 0]),
       "bytes that do not decode fire error and report a complete, sizeless image: " +
       JSON.stringify(seen));

     // A read after a write is a forced synchronous layout, exactly as a
     // geometry read is; the frame-boundary poll is not one, since no script
     // asked for it.
     __blitsenAnimationFrameTick(64);
     expect(__blitsenForcedLayoutsThisFrame() === 0, "the settle poll charges no forced layout");
     parsed.setAttribute("width", "40");
     void parsed.naturalWidth;
     expect(__blitsenForcedLayoutsThisFrame() === 1, "an image read after a write is a forced layout");
     parsed.setAttribute("data-images", "ok"); }`,
  320,
  180,
));
assert.equal(images.nodes.find(node => node.attributes.id === "parsed").attributes["data-images"], "ok");
assert.deepEqual(
  images.nodes.filter(node => node.tag === "img").map(node => node.image),
  [{ natural_width: 8, natural_height: 4, complete: true, errored: false },
    { natural_width: 8, natural_height: 4, complete: true, errored: false },
    { natural_width: 0, natural_height: 0, complete: true, errored: true }],
  "the JavaScript surface and the backend read report the same three images");

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

  // `window.stop()`. Svelte's minified store reads the bare name for its truth
  // value alone, but what it names is a real abort of the document's load.
  const stopped = JSON.parse(native.runBridgeHarness(
    `<div id="stop"></div>`,
    `{ const results = globalThis.__blitsenStop = { settled: [], ticks: [] };
       if (typeof stop !== "function" || stop !== window.stop)
         throw new Error("stop must resolve on the bare name a bundle reads");
       // Nothing is in flight yet: the machinery runs and finds nothing.
       if (stop() !== undefined) throw new Error("stop returns nothing");

       setTimeout(() => results.ticks.push("timeout"), 0);
       requestAnimationFrame(() => results.ticks.push("frame"));
       Promise.all([
         (() => {
           const pending = fetch("${probeOrigin}/echo");
           stop();
           return pending.then(() => "resolved",
             error => ["stopped", error.name, error instanceof DOMException]);
         })(),
         // Started after the stop, so it is part of a new load rather than the
         // stopped one; the stop it makes on arrival has nothing left to abort.
         fetch("${probeOrigin}/echo").then(async response => {
           stop();
           return ["after", response.status, (await response.json()).method];
         }),
       ]).then(settled => { results.settled = settled; results.done = true; });
       document.getElementById("stop").setAttribute("data-stop", "ok"); }`,
    200,
    100,
  ));
  assert.equal(stopped.nodes.find(node => node.attributes.id === "stop").attributes["data-stop"], "ok");
  for (let turn = 0; turn < 200 && !globalThis.__blitsenStop.done; turn++) {
    globalThis.__blitsenAnimationFrameTick(0);
    await Bun.sleep(5);
  }
  const aborted = new Map(globalThis.__blitsenStop.settled.map(entry => [entry[0], entry]));
  assert.deepEqual(aborted.get("stopped"), ["stopped", "AbortError", true],
    "stop() rejects an in-flight request exactly as that request's own signal would");
  assert.deepEqual(aborted.get("after"), ["after", 200, "GET"],
    "a load started after a stop completes, and a stop with nothing in flight leaves it alone");
  assert.deepEqual(globalThis.__blitsenStop.ticks.sort(), ["frame", "timeout"],
    "stop() aborts loading; a browser does not cancel timers or animation frames and neither does this");
  delete globalThis.__blitsenStop;
} finally {
  probe.close();
}

// The `native:` modules. Everything below reaches them the way an application
// does — through the `blitsen/app` and `blitsen/clipboard` proxies — so what is
// asserted is the installed namespace, not a description of it.
const nativeManifest = await loadApiManifest();
const namespaces = { app, clipboard, dialog, window: windowModule };
// The members whose presence is a platform fact rather than a version fact: the
// single-instance lock is a Unix socket, and a dialog is the XDG portal.
const absentOn = new Map([["app.requestSingleInstanceLock", ["win32"]]]);
for (const entry of nativeManifest.native.filter(entry => entry.module === "dialog")) {
  absentOn.set(entry.api, ["win32", "darwin"]);
}
for (const entry of nativeManifest.native) {
  const namespace = namespaces[entry.module];
  assert(namespace, `the manifest names native:${entry.module}, which the harness does not import`);
  const installed = entry.status === "implemented"
    && !(absentOn.get(entry.api) ?? []).includes(process.platform);
  if (installed) {
    assert.equal(typeof namespace[entry.member], "function",
      `native:${entry.api} is implemented and must be installed`);
    assert.equal(entry.member in namespace, true, `native:${entry.api} must be enumerable`);
  } else {
    // Absent, not stubbed: the property does not exist, so `if (app.onSuspend)`
    // selects a fallback instead of calling something that throws.
    assert.equal(namespace[entry.member], undefined,
      `native:${entry.api} is absent and must not be installed`);
    assert.equal(entry.member in namespace, false,
      `native:${entry.api} must not answer an "in" check`);
  }
}
for (const [name, namespace] of Object.entries(namespaces)) {
  assert.deepEqual(Object.keys(namespace).sort(),
    nativeManifest.native
      .filter(entry => entry.module === name && entry.status === "implemented"
        && !(absentOn.get(entry.api) ?? []).includes(process.platform))
      .map(entry => entry.member).sort(),
    `the native:${name} namespace enumerates exactly what the runtime installed`);
}
assert.throws(() => { app.dataDir = () => "/tmp"; }, /read-only/);

// Application directories. The application names itself, because the runtime
// cannot: the executable here is Bun.
const applicationName = "Blitsen Harness";
const directories = [app.dataDir(applicationName), app.cacheDir(applicationName),
  app.configDir(applicationName)];
for (const directory of directories) {
  assert.equal(isAbsolute(directory), true, `${directory} must be absolute`);
  assert.equal(basename(directory), applicationName);
}
assert.notEqual(directories[0], directories[1], "cache is not where data lives");
if (process.platform === "linux") {
  const home = (variable, fallback) => process.env[variable] || join(homedir(), fallback);
  assert.deepEqual(directories, [
    join(home("XDG_DATA_HOME", ".local/share"), applicationName),
    join(home("XDG_CACHE_HOME", ".cache"), applicationName),
    join(home("XDG_CONFIG_HOME", ".config"), applicationName),
  ], "the XDG base directories, or their defaults");
}
for (const rejected of ["", ".", "..", "escape/../..", "escape\\out"]) {
  assert.throws(() => app.dataDir(rejected), /not a valid application name/,
    `${JSON.stringify(rejected)} must not reach out of the directory the platform chose`);
}
// The directory is named, not created: making it is `node:fs`.
assert.equal(existsSync(directories[0]), false);

// The clipboard. A read is a round-trip through the real X11/Wayland selection
// or the system pasteboard; there is no in-process shortcut behind it.
assert.throws(() => clipboard.writeImage({ width: 2, height: 2, data: new Uint8Array(8) }),
  /RGBA bytes/, "an image must carry its own pixels");
const displayed = process.platform !== "linux"
  || Boolean(process.env.DISPLAY || process.env.WAYLAND_DISPLAY);
if (displayed) {
  const text = `blitsen harness ${process.pid}`;
  clipboard.writeText(text);
  assert.equal(clipboard.readText(), text);
  clipboard.writeHtml("<b>bold</b>", "bold");
  assert.equal(clipboard.readHtml(), "<b>bold</b>");
  assert.equal(clipboard.readText(), "bold",
    "HTML carries the plain text a paste that cannot read it receives");
  const pixels = new Uint8Array([255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255]);
  clipboard.writeImage({ width: 2, height: 2, data: pixels });
  const image = clipboard.readImage();
  assert.equal(image.width, 2);
  assert.equal(image.height, 2);
  assert.deepEqual([...image.data], [...pixels], "RGBA survives the clipboard's own encoding");
  assert.equal(clipboard.readText(), null, "an image is not text, and says so rather than throwing");
  clipboard.clear();
  assert.equal(clipboard.readText(), null);
  assert.equal(clipboard.readImage(), null);
} else {
  console.log("clipboard round-trips skipped: no DISPLAY or WAYLAND_DISPLAY on this host");
}

// The window, and the dialogs that are modal to it.
//
// This harness loads the addon into Bun rather than into a `blitsen <directory>`
// run, so there is no window here and never will be. That is the honest half to
// assert: everything each call decides before it needs one — the vocabulary it
// accepts and the shape of its options — plus the fact that a call without a
// window says which it is instead of quietly doing nothing. Driving a real
// window, or dismissing a real dialog, needs a person; the M4 notes say what to
// run and what to look for.
for (const [call, refusal] of [
  [() => windowModule.setCursor("wiggly"), /not a CSS cursor keyword/],
  [() => windowModule.setCursorGrab("everything"), /not a cursor grab mode/],
  [() => windowModule.setSize(0, 100), /at least 1x1 CSS pixels/],
  [() => windowModule.setSize(800, Infinity), /at least 1x1 CSS pixels/],
  [() => windowModule.setSize("wide", 600), /invalid window width/],
]) assert.throws(call, refusal, "a mistyped argument is refused before the window is looked for");

for (const call of [
  () => windowModule.setSize(800, 600),
  () => windowModule.setFullscreen(true),
  () => windowModule.isFullscreen(),
  () => windowModule.setDecorations(false),
  () => windowModule.isDecorated(),
  () => windowModule.setAlwaysOnTop(true),
  () => windowModule.setCursor("pointer"),
  () => windowModule.setCursorVisible(false),
  () => windowModule.setCursorGrab("none"),
  () => windowModule.monitors(),
]) assert.throws(call, /no application window yet/,
  "a window operation with no window reports that, rather than being a no-op");

if (dialog.openFile) {
  for (const [call, refusal] of [
    [() => dialog.openFile(null), /options must be an object/],
    [() => dialog.openFile({ filters: "text" }), /filters must be an array/],
    [() => dialog.openFile({ filters: [{ name: "text" }] }), /name, extensions/],
    [() => dialog.message({ level: "shouting" }), /not a message level/],
    [() => dialog.message({ buttons: "maybe" }), /not a button set/],
  ]) assert.throws(call, refusal, "dialog options are checked where the call was made");
  // A dialog here is always modal to the application window, so without one
  // nothing opens — and nothing is left outstanding for a frame turn to deliver.
  const outstanding = globalThis.__blitsenAnimationFramesPending();
  for (const call of [
    () => dialog.openFile({ title: "Open", filters: [{ name: "Text", extensions: ["txt"] }] }),
    () => dialog.openFiles(),
    () => dialog.saveFile({ fileName: "untitled.txt" }),
    () => dialog.openFolder({ directory: tmpdir() }),
    () => dialog.openFolders(),
    () => dialog.message({ title: "Quit", message: "Really?", buttons: "yesNo" }),
  ]) assert.throws(call, /no application window yet/);
  assert.equal(globalThis.__blitsenAnimationFramesPending(), outstanding,
    "a dialog that never opened leaves nothing for a frame turn to settle");
}

// The single-instance lock, over the real socket: the second request finds the
// lock held, hands this invocation over, and the first instance is handed it
// back on a frame turn.
if (process.platform !== "win32") {
  const received = [];
  // A stable name, so a run after one that crashed also exercises taking over a
  // socket whose owner is gone.
  const lockName = "blitsen-native-harness";
  assert.equal(app.requestSingleInstanceLock(lockName, invocation => received.push(invocation)),
    true, "the first instance owns the lock");
  assert.throws(() => app.requestSingleInstanceLock(lockName, "not a function"), TypeError);
  assert.equal(app.requestSingleInstanceLock(lockName), false,
    "a second request finds the lock held and hands its invocation over");
  // The hand-off crosses a socket and a listener thread, so the wait is for the
  // host to report work; the delivery itself is one turn, not a poll.
  let waiting = false;
  for (let turn = 0; turn < 200 && !waiting; turn++) {
    waiting = globalThis.__blitsenAnimationFramesPending();
    if (!waiting) await Bun.sleep(5);
  }
  assert.equal(waiting, true, "an undelivered invocation keeps the host turning");
  assert.equal(received.length, 0, "nothing is delivered between frame turns");
  globalThis.__blitsenAnimationFrameTick(0);
  assert.equal(received.length, 1, "the invocation arrived on the next frame turn");
  assert.deepEqual(received[0].argv, process.argv.map(String),
    "the second instance's command line, as the OS gave it");
  assert.equal(received[0].cwd, process.cwd());
  assert.equal(globalThis.__blitsenAnimationFramesPending(), false,
    "a delivered invocation stops asking for frames");
}

// `relaunch`. The successor is this process's own command line run again, so it
// is tested with a script that counts its own generations and stops at two.
const relaunchDirectory = mkdtempSync(join(tmpdir(), "blitsen-relaunch-"));
try {
  const marker = join(relaunchDirectory, "generations");
  const script = join(relaunchDirectory, "relaunch.mjs");
  writeFileSync(marker, "");
  writeFileSync(script, `
    import { appendFileSync, readFileSync } from "node:fs";
    import { createRequire } from "node:module";
    const native = createRequire(import.meta.url)(process.env.BLITSEN_RELAUNCH_ADDON);
    native.runBridgeHarness("<div></div>", "", 32, 32);
    const { default: app } = await import(process.env.BLITSEN_RELAUNCH_MODULE);
    const marker = process.env.BLITSEN_RELAUNCH_MARKER;
    appendFileSync(marker, process.argv.join(" ") + "\\n");
    const generations = readFileSync(marker, "utf8").split("\\n").filter(Boolean);
    if (generations.length < 2) app.relaunch();
  `);
  const relaunched = Bun.spawnSync({
    cmd: [process.execPath, script],
    env: {
      ...process.env,
      BLITSEN_RELAUNCH_ADDON: addonPath,
      BLITSEN_RELAUNCH_MARKER: marker,
      BLITSEN_RELAUNCH_MODULE: new URL("../src/native/app.mjs", import.meta.url).href,
    },
    stdout: "inherit",
    stderr: "inherit",
  });
  assert.equal(relaunched.exitCode, 0, "the relaunching process exits cleanly");
  let generations = [];
  for (let wait = 0; wait < 200 && generations.length < 2; wait++) {
    await Bun.sleep(25);
    generations = readFileSync(marker, "utf8").split("\n").filter(Boolean);
  }
  assert.equal(generations.length, 2, "relaunch starts a successor that outlives this process");
  assert.equal(generations[0], generations[1],
    "the successor runs the same command line, argument for argument");
} finally {
  rmSync(relaunchDirectory, { recursive: true, force: true });
}

console.log("native modules passed", `clipboard=${displayed ? "round-tripped" : "skipped"}`);
console.log("bridge harness passed", process.platform, process.arch, `style=${styled.attributes["data-style-call-us"]}us/call`);
