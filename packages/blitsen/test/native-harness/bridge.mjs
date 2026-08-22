import { strict as assert } from "node:assert";
import { join } from "node:path";

import { native } from "./addon.mjs";

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
// Waited for rather than slept through: what is asserted below is the order
// these land in and that the interval stops itself, not how long a loaded
// runner takes to deliver two 2ms periods. A fixed sleep asserts the second
// thing by accident, and fails on the machine that was busy.
const timersSettled = performance.now() + 2000;
while (!globalThis.__blitsenTimerOrder.includes("interval:2")
  && performance.now() < timersSettled) await Bun.sleep(1);
assert.deepEqual(globalThis.__blitsenTimerOrder.slice(0, 2), ["timeout:a:2", "microtask"],
  "Bun timer arguments are forwarded and microtasks drain after the macrotask");
assert.deepEqual(globalThis.__blitsenTimerOrder.filter(entry => entry.startsWith("interval")),
  ["interval:1", "interval:2"], "intervals repeat and clearInterval stops them");
assert(!globalThis.__blitsenTimerOrder.includes("cancelled"), "clearTimeout cancels the callback");
delete globalThis.__blitsenTimerOrder;

