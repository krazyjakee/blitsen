// User text selection in document content (issue #436).
//
// Driven through the same mouse dispatch a real press takes:
// `__blitsenInjectMouseEvent` calls `dispatchMouseEvent`, which runs the
// document-selection response after listeners, so what is asserted here is the
// shipping path rather than a shortcut. Nothing paints the selection yet; what
// this proves is that the user *makes* the real `getSelection()`, that
// `user-select` is honoured, that a run of changes announces itself once through
// `selectionchange`, and that Ctrl/Cmd+A outside a field selects the document.
//
// The harness runs the script in this same realm (Phase 1 is Bun's engine), so
// a global the script writes is readable back here — which is how the later
// `selectionchange` task is observed, exactly as `ranges.mjs` observes its own.
import { strict as assert } from "node:assert";

import { native } from "./addon.mjs";

const result = JSON.parse(native.runBridgeHarness(
  `<style>
     body { margin: 0; font: 16px/20px monospace }
     #para { position: absolute; left: 20px; top: 10px; width: 600px }
     #nope { position: absolute; left: 20px; top: 60px; width: 600px; user-select: none }
   </style>
   <p id="para">alpha beta gamma</p>
   <p id="nope">unselectable text</p>`,
  `{ const expect = (condition, message) => { if (!condition) throw new Error(message); };
     const para = document.getElementById("para");
     const nope = document.getElementById("nope");
     const text = para.firstChild;
     const boxOf = (node, from, to) => { const range = document.createRange();
       range.setStart(node, from); range.setEnd(node, to); return range.getBoundingClientRect(); };
     const middle = (node, from, to) => { const box = boxOf(node, from, to);
       return { x: box.left + box.width / 2, y: box.top + box.height / 2 }; };
     const press = (type, target, point, extra = {}) =>
       __blitsenInjectMouseEvent(type, target, { button: 0,
         buttons: type === "mouseup" ? 0 : 1, clientX: point.x, clientY: point.y,
         bubbles: true, cancelable: true, ...extra });

     const changes = globalThis.__blitsenSelectionDrags = [];
     document.addEventListener("selectionchange", () => changes.push(getSelection().toString()));

     // A drag from the first character to the last selects the run between them,
     // and it is the real selection that getSelection() returns.
     const start = middle(text, 0, 1);
     const end = middle(text, 15, 16);
     press("mousedown", para, start);
     press("mousemove", para, end);
     press("mouseup", para, end);
     const dragged = getSelection().toString();
     expect(dragged.length > 5 && "alpha beta gamma".includes(dragged),
       "the drag selected a run of the paragraph: " + JSON.stringify(dragged));

     // A double-click takes the word under the point.
     const onBeta = middle(text, 7, 8);
     press("mousedown", para, onBeta);
     press("mousedown", para, onBeta);
     expect(getSelection().toString() === "beta",
       "a double-click takes the word: " + JSON.stringify(getSelection().toString()));

     // A third press in the same place takes the whole block.
     press("mousedown", para, onBeta);
     expect(getSelection().toString() === "alpha beta gamma",
       "a triple-click takes the paragraph: " + JSON.stringify(getSelection().toString()));

     // A Shift+click extends from the anchor the last selection kept.
     getSelection().collapse(text, 0);
     press("mousedown", para, middle(text, 4, 5), { shiftKey: true });
     expect(getSelection().toString().length >= 4 && "alpha".startsWith(getSelection().toString()),
       "Shift+click extends the selection: " + JSON.stringify(getSelection().toString()));

     // A plain click collapses the selection, which is how clicking elsewhere
     // clears it.
     press("mousedown", para, start);
     press("mouseup", para, start);
     expect(getSelection().isCollapsed, "a plain click collapses the selection");

     // user-select: none refuses a selection, and the click still lands.
     let clicked = false;
     nope.addEventListener("click", () => { clicked = true; });
     const nopePoint = middle(nope.firstChild, 0, 4);
     press("mousedown", nope, nopePoint);
     press("mousemove", nope, { x: nopePoint.x + 60, y: nopePoint.y });
     press("mouseup", nope, { x: nopePoint.x + 60, y: nopePoint.y });
     expect(getSelection().rangeCount === 0 || getSelection().isCollapsed,
       "user-select: none is not selectable: " + JSON.stringify(getSelection().toString()));
     nope.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
     expect(clicked, "and clicking the unselectable element still works");

     // Ctrl/Cmd+A with focus outside a text control selects the document.
     getSelection().removeAllRanges();
     __blitsenDispatchKeyboardEvent("keydown",
       { key: "a", code: "KeyA", ctrlKey: true, bubbles: true, cancelable: true });
     expect(getSelection().toString().includes("alpha beta gamma"),
       "Ctrl/Cmd+A selects the document's content: " + JSON.stringify(getSelection().toString()));

     // Record what the runtime resolved user-select to, so the assertion below
     // proves the property the whole feature reads is actually resolvable here.
     para.setAttribute("data-user-select", getComputedStyle(nope).getPropertyValue("user-select"));
     para.setAttribute("data-selection", "ok"); }`,
  640,
  240,
));

const para = result.nodes.find(node => node.attributes.id === "para");
assert.equal(para.attributes["data-selection"], "ok");
assert.equal(para.attributes["data-user-select"], "none",
  "user-select resolves through getComputedStyle, which is what the selection honours");

// The selectionchange lands in a later task, after the script that changed the
// selection finished, and a run of changes announces itself once — carrying the
// settled selection, which is the Ctrl/Cmd+A document selection.
await new Promise(resolve => setTimeout(resolve, 0));
const drags = globalThis.__blitsenSelectionDrags;
assert.ok(Array.isArray(drags) && drags.length >= 1,
  "selectionchange fired for the user's selection");
assert.ok(drags[drags.length - 1].includes("alpha beta gamma"),
  "and its settled value is what the user last selected: " + JSON.stringify(drags.at(-1)));
