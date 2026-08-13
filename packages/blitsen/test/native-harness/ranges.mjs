// Ranges, carets and the selection, against the real layout.
//
// Nothing here asserts a pixel count: the harness shapes text in whatever font
// the host has, so a width is not a fact this can pin. What it can pin is every
// relation between the measurements — a substring sits inside the run it is
// part of, a range that crosses a `<br>` occupies two lines, the character a
// point resolves to is the one whose box contains that point — and those are
// what a caller measuring text actually depends on. The exact geometry is
// asserted against a committed font in `crates/blitsen-blitz/src/tests`.
import { strict as assert } from "node:assert";

import { native } from "./addon.mjs";

const geometry = JSON.parse(native.runBridgeHarness(
  `<style>
     body { margin: 0; font: 16px/20px sans-serif }
     #line { position: absolute; left: 40px; top: 30px; width: 400px }
     #tall { display: inline-block; width: 30px; height: 40px; vertical-align: top }
   </style>
   <p id="line">first<span id="mid">middle</span><br>second<span id="tall"></span></p>`,
  `{ const expect = (condition, message) => { if (!condition) throw new Error(message); };
     const line = document.getElementById("line");
     const mid = document.getElementById("mid");
     const first = line.firstChild;
     const middle = mid.firstChild;
     const second = line.childNodes[3];

     const fresh = new Range();
     expect(fresh.collapsed && fresh.startContainer === document && fresh.startOffset === 0,
       "a new range is collapsed at the start of the document");
     expect(fresh.getClientRects().length === 0 &&
       fresh.getBoundingClientRect().width === 0,
       "and a collapsed range measures nothing");
     expect(document.createRange() instanceof Range, "document.createRange makes one");

     const whole = document.createRange();
     whole.selectNodeContents(first);
     const rects = whole.getClientRects();
     expect(rects.length === 1, "one line box, one rectangle: " + rects.length);
     const bounds = whole.getBoundingClientRect();
     expect(bounds.x === rects[0].x && bounds.width === rects[0].width,
       "the bounding rectangle of one fragment is that fragment");
     expect(bounds.x >= 40 && bounds.y >= 30 && bounds.height > 0,
       "measured in the viewport, inside the box it sits in: " + JSON.stringify(bounds.toJSON()));

     const part = document.createRange();
     part.setStart(first, 1);
     part.setEnd(first, 3);
     const partial = part.getBoundingClientRect();
     expect(partial.width > 0 && partial.width < bounds.width &&
       partial.left > bounds.left && partial.right < bounds.right,
       "a substring is narrower than its run and inside it: " +
       JSON.stringify([partial.toJSON(), bounds.toJSON()]));
     expect(part.toString() === "ir", "and it is the characters it measured: " + part.toString());

     // A range across a line break covers a rectangle on each line, which is the
     // whole reason the answer is a list rather than a box.
     const across = document.createRange();
     across.setStart(middle, 2);
     across.setEnd(second, 3);
     const lines = across.getClientRects();
     expect(lines.length === 2, "a range across the break is measured per line: " + lines.length);
     expect(lines[1].top > lines[0].top && lines[1].left < lines[0].left,
       "the second line starts further down and back at the left margin");
     expect(across.toString() === "ddlesec", "the text it covers: " + across.toString());

     // An element the range covers whole contributes its border box, which is
     // how a replaced or inline-block child inside a run is measured at all.
     const covering = document.createRange();
     covering.selectNodeContents(line);
     const tall = document.getElementById("tall");
     const box = tall.getBoundingClientRect();
     expect(covering.getClientRects().some(rect =>
       rect.width === box.width && rect.height === box.height),
       "the box of an element the range covers is in the list");

     // Text nothing laid out has no geometry, rather than an empty box at the
     // origin that a caller would place something at.
     const hidden = document.createElement("p");
     hidden.style.display = "none";
     hidden.textContent = "unseen";
     document.body.appendChild(hidden);
     const unseen = document.createRange();
     unseen.selectNodeContents(hidden.firstChild);
     expect(unseen.getClientRects().length === 0 && unseen.toString() === "unseen",
       "a run in a display:none subtree has text but no geometry");

     line.setAttribute("data-geometry", "ok"); }`,
  480,
  240,
));
assert.equal(geometry.nodes.find(node => node.attributes.id === "line")
  .attributes["data-geometry"], "ok");

// Boundary points: what a range is before it is geometry. Every one of these is
// answered in JavaScript against the tree, so they are asserted on their own.
const boundaries = JSON.parse(native.runBridgeHarness(
  `<div id="host"><b id="one">one</b><i id="two">two</i></div>`,
  `{ const expect = (condition, message) => { if (!condition) throw new Error(message); };
     const host = document.getElementById("host");
     const one = document.getElementById("one");
     const two = document.getElementById("two");

     const range = document.createRange();
     range.setStart(one.firstChild, 1);
     range.setEnd(two.firstChild, 2);
     expect(range.commonAncestorContainer === host,
       "the deepest node containing both ends");
     expect(!range.collapsed && range.toString() === "netw", "the text between them");
     expect(range.intersectsNode(one) && range.intersectsNode(two) && range.intersectsNode(host),
       "every node it reaches");
     expect(range.comparePoint(one.firstChild, 0) === -1 &&
       range.comparePoint(one.firstChild, 2) === 0 &&
       range.comparePoint(two.firstChild, 3) === 1,
       "a point before, inside and after it");
     expect(range.isPointInRange(one.firstChild, 2) && !range.isPointInRange(two.firstChild, 3),
       "which is what isPointInRange reports");

     // Moving a boundary past the other collapses onto it rather than inverting.
     const collapsing = range.cloneRange();
     collapsing.setStart(two.firstChild, 3);
     expect(collapsing.collapsed && collapsing.startContainer === two.firstChild &&
       collapsing.startOffset === 3, "a start moved past the end collapses the range");
     expect(!range.collapsed, "and the clone it was made from is untouched");

     const around = document.createRange();
     around.selectNode(one);
     expect(around.startContainer === host && around.startOffset === 0 &&
       around.endContainer === host && around.endOffset === 1 && around.toString() === "one",
       "selectNode puts the boundaries either side of the node");
     expect(around.compareBoundaryPoints(Range.START_TO_START, range) === -1 &&
       around.compareBoundaryPoints(Range.END_TO_END, range) === -1 &&
       range.compareBoundaryPoints(Range.START_TO_START, range) === 0,
       "boundary points compare in tree order");

     let refused = false;
     try { range.setStart(one.firstChild, 9); } catch { refused = true; }
     expect(refused, "an offset outside the node is refused");
     expect(range.deleteContents === undefined && range.insertNode === undefined,
       "the mutating half of Range is absent rather than half-built");
     host.setAttribute("data-boundaries", "ok"); }`,
  320,
  180,
));
assert.equal(boundaries.nodes.find(node => node.attributes.id === "host")
  .attributes["data-boundaries"], "ok");

// The caret read, checked against the rectangles rather than against a
// coordinate written down here: the point asked about is the middle of a box a
// range just reported, so the answer is the character that box belongs to.
const carets = JSON.parse(native.runBridgeHarness(
  `<style>body { margin: 0; font: 16px/20px sans-serif }
     #para { position: absolute; left: 20px; top: 10px; width: 400px }</style>
   <p id="para">abcdef</p>`,
  `{ const expect = (condition, message) => { if (!condition) throw new Error(message); };
     const text = document.getElementById("para").firstChild;
     const boxOf = (from, to) => {
       const range = document.createRange();
       range.setStart(text, from);
       range.setEnd(text, to);
       return range.getBoundingClientRect();
     };
     const third = boxOf(2, 3);
     const middle = { x: third.left + third.width / 2, y: third.top + third.height / 2 };

     const caret = document.caretPositionFromPoint(middle.x, middle.y);
     expect(caret.offsetNode === text, "the point resolves to the text node under it");
     expect(caret.offset === 2 || caret.offset === 3,
       "and to one side of the character it landed on: " + caret.offset);
     const rect = caret.getClientRect();
     expect(rect.width === 0 && rect.left >= third.left - 1 && rect.left <= third.right + 1,
       "the caret box is zero-wide beside that character: " + JSON.stringify(rect.toJSON()));

     const range = document.caretRangeFromPoint(middle.x, middle.y);
     expect(range instanceof Range && range.collapsed && range.startContainer === text &&
       range.startOffset === caret.offset,
       "the same reading, spelled as a collapsed range");

     expect(document.caretPositionFromPoint(2000, 2000) === null &&
       document.caretRangeFromPoint(2000, 2000) === null,
       "a point over no text at all has no answer");
     document.getElementById("para").setAttribute("data-caret", "ok"); }`,
  480,
  240,
));
assert.equal(carets.nodes.find(node => node.attributes.id === "para")
  .attributes["data-caret"], "ok");

// The selection: one object for the life of the document, holding an anchor and
// a focus rather than a range, because the direction is what an editor reads.
const selection = JSON.parse(native.runBridgeHarness(
  `<div id="host"><b id="one">one</b><i id="two">two</i></div>`,
  `{ const expect = (condition, message) => { if (!condition) throw new Error(message); };
     const host = document.getElementById("host");
     const one = document.getElementById("one").firstChild;
     const two = document.getElementById("two").firstChild;
     const changes = globalThis.__blitsenSelectionChanges = [];
     document.addEventListener("selectionchange", () => changes.push(getSelection().toString()));

     const selection = getSelection();
     expect(selection === document.getSelection() && selection === getSelection(),
       "the selection is one persistent object");
     expect(selection.rangeCount === 0 && selection.anchorNode === null &&
       selection.type === "None" && selection.direction === "none" && selection.isCollapsed,
       "and starts with nothing selected");
     let refused = false;
     try { selection.getRangeAt(0); } catch { refused = true; }
     expect(refused, "so there is no range to get");

     selection.setBaseAndExtent(one, 1, two, 2);
     expect(selection.rangeCount === 1 && selection.anchorNode === one &&
       selection.anchorOffset === 1 && selection.focusNode === two &&
       selection.focusOffset === 2, "the anchor and focus it was given");
     expect(selection.type === "Range" && !selection.isCollapsed &&
       selection.direction === "forward", "a selection made forwards");
     expect(selection.toString() === "netw" && selection.getRangeAt(0).toString() === "netw",
       "and the text between them: " + selection.toString());
     expect(selection.containsNode(document.getElementById("two")) === false &&
       selection.containsNode(document.getElementById("two"), true),
       "a partly selected node is contained only when partial ones count");

     selection.setBaseAndExtent(two, 2, one, 1);
     expect(selection.direction === "backward" && selection.toString() === "netw",
       "the same selection made backwards keeps its direction and its text");
     const backwards = selection.getRangeAt(0);
     expect(backwards.startContainer === one && backwards.endContainer === two,
       "while the range it hands back is still in tree order");

     selection.collapse(one, 2);
     expect(selection.isCollapsed && selection.type === "Caret" && selection.toString() === "",
       "collapsed onto a caret");
     selection.extend(two, 1);
     expect(selection.focusNode === two && selection.toString() === "et",
       "extended from the anchor it kept");
     selection.selectAllChildren(host);
     expect(selection.toString() === "onetwo", "or told to take the lot");

     const added = document.createRange();
     added.selectNodeContents(one);
     selection.removeAllRanges();
     expect(selection.rangeCount === 0, "emptied");
     selection.addRange(added);
     expect(selection.rangeCount === 1 && selection.toString() === "one", "and given a range");
     expect(changes.length === 0, "nothing has been announced from inside the calls");
     host.setAttribute("data-selection", "ok"); }`,
  320,
  180,
));
assert.equal(selection.nodes.find(node => node.attributes.id === "host")
  .attributes["data-selection"], "ok");

// `selectionchange` is announced in a later task, so it lands after the script
// that changed the selection has finished — and a run of changes says so once.
await new Promise(resolve => setTimeout(resolve, 0));
assert.deepEqual(globalThis.__blitsenSelectionChanges, ["one"],
  "one selectionchange for the run of changes, carrying the settled selection");
