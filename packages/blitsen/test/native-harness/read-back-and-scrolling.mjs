// The read-back and scrolling surface (issue #115).
//
// These are the APIs a component library reaches for to answer "where is this
// and what does it say", plus the scrolling that moves the answer. They are
// grouped because they share a failure mode: each one can be present and
// return a plausible number that no pixel agrees with. So the scrolling checks
// below assert on painted colours rather than on the offsets they set — a
// `scrollTop` that moved and a viewport that did not would pass any check that
// only read the property back.
import { strict as assert } from "node:assert";

import { native } from "./addon.mjs";

const DOCUMENT = `<style>
  body { margin: 0 }
  #box { display:block; width:200px; height:200px; overflow:auto;
         border-top:3px solid #000; border-left:5px solid #000; position:relative }
  #tall { display:block; height:1200px }
  #target { display:block; height:20px; margin-top:600px }
  .gone { display:none }
  .unpainted { visibility:hidden }
</style>
<div id="box"><div id="tall"><div id="target">T</div></div></div>
<p id="para">one<br>two <span class="gone">skipped</span><span class="unpainted">skipped</span><em>three</em></p>`;

const reads = JSON.parse(native.runBridgeHarness(DOCUMENT,
  `{ const expect = (actual, wanted, what) => {
       const seen = JSON.stringify(actual), meant = JSON.stringify(wanted);
       if (seen !== meant) throw new Error(what + ": " + seen + " is not " + meant);
     };
     const box = document.getElementById("box");
     const target = document.getElementById("target");
     const para = document.getElementById("para");

     // Border widths, not the padding-inclusive difference between the boxes.
     expect([box.clientTop, box.clientLeft], [3, 5], "clientTop and clientLeft are the border widths");

     // Rendered text: a <br> is a line break, and neither an unlaid-out nor an
     // unpainted subtree contributes. textContent would have carried both.
     expect(para.innerText, "one\\ntwo three", "innerText skips hidden subtrees and breaks on <br>");
     expect(para.textContent.includes("skipped"), true, "textContent still carries what innerText drops");
     para.innerText = "a\\nb";
     expect([para.childNodes.length, para.innerText], [3, "a\\nb"], "writing innerText inserts a <br>");

     // Reflected content attributes.
     expect([box.title, box.hidden], ["", false], "an unreflected attribute reads empty");
     box.title = "named"; box.hidden = true;
     expect([box.getAttribute("title"), box.getAttribute("hidden")], ["named", ""],
       "title and hidden write through to the attributes");
     box.hidden = false;
     expect(box.hasAttribute("hidden"), false, "clearing hidden removes the attribute");
     expect(box.tabIndex, -1, "an ordinary element is not in the tab order");
     expect(document.createElement("input").tabIndex, 0, "a control is");
     box.tabIndex = 3;
     expect([box.getAttribute("tabindex"), box.tabIndex], ["3", 3], "tabIndex reflects both ways");

     // The nearest positioned ancestor, which #box is because it is relative.
     expect(target.offsetParent === box, true, "offsetParent is the nearest positioned ancestor");
     const detached = document.createElement("div");
     detached.className = "gone";
     document.body.appendChild(detached);
     expect(detached.offsetParent, null, "an unlaid-out element has no offset parent");

     // Document position, as the bitmask rather than as a boolean.
     const first = document.createElement("i"), second = document.createElement("u");
     para.append(first, second);
     expect(para.compareDocumentPosition(first), 20, "a descendant is CONTAINED_BY | FOLLOWING");
     expect(first.compareDocumentPosition(para), 10, "and its ancestor is CONTAINS | PRECEDING");
     expect(first.compareDocumentPosition(second), 4, "a later sibling FOLLOWS");
     expect(second.compareDocumentPosition(first), 2, "an earlier one PRECEDES");
     expect(first.compareDocumentPosition(document.createElement("s")), 35,
       "another tree is DISCONNECTED | PRECEDING | IMPLEMENTATION_SPECIFIC");
     expect(first.compareDocumentPosition(first), 0, "a node is not positioned against itself");

     const inserted = document.createElement("hr");
     expect(para.insertAdjacentElement("afterbegin", inserted) === inserted
       && para.firstChild === inserted, true, "insertAdjacentElement returns what it inserted");
     expect(document.createElement("div").insertAdjacentElement("beforebegin", document.createElement("hr")),
       null, "a parentless element has no beforebegin to insert into");

     // Document reads.
     expect(document.title, "", "a document with no <title> has an empty one");
     document.title = "Named";
     expect([document.title, document.querySelector("title").textContent], ["Named", "Named"],
       "setting the title creates the element it reads back from");
     expect(document.characterSet, "UTF-8", "every document is decoded as UTF-8");
     expect(document.documentURI, location.href, "documentURI is the document's own address");
     expect(document.hasFocus(), true, "a window that was never told otherwise is focused");
     expect(document.scrollingElement === document.documentElement, true,
       "standards mode scrolls the root element");
     expect(document.adoptNode(first) === first, true, "there is one document to adopt into");
     const named = document.createElement("input");
     named.setAttribute("name", "q");
     document.body.appendChild(named);
     expect([...document.getElementsByName("q")].length, 1, "getElementsByName matches the attribute");

     // CSS.escape is string work; CSS.supports asks the cascade's own parser.
     expect(CSS.escape("w-1/2"), "w-1\\\\/2", "a bundler's class escapes for a selector");
     expect([CSS.supports("color", "red"), CSS.supports("color", "notacolour")], [true, false],
       "supports answers from what the parser accepts");
     expect(CSS.supports("(display: flex)"), true, "the one-argument condition form");
     expect(CSS.supports("display: flex and (color: red)"), false,
       "a compound condition is not decomposed, and says so");

     // DOMParser: a detached fragment, and explicit that it is not a document.
     const parsed = new DOMParser().parseFromString("<p class='k'>hello <b>world</b></p>");
     expect([parsed.body.textContent, parsed.querySelector(".k") !== null, parsed.head],
       ["hello world", true, null], "a parsed string is a fragment whose body is itself");
     let refused;
     try { new DOMParser().parseFromString("<a/>", "text/xml"); } catch (error) { refused = error.name; }
     expect(refused, "TypeError", "an XML type is refused rather than mis-parsed as HTML");

     // Event interfaces, as constructors with their own members.
     expect(new FocusEvent("focus", { relatedTarget: para }).relatedTarget === para, true, "FocusEvent");
     expect(new PointerEvent("pointerdown", { clientX: 3 }).pointerType, "mouse", "PointerEvent");
     expect(new WheelEvent("wheel", { deltaY: 4 }).deltaMode, 0, "WheelEvent delivers pixels");
     expect(new InputEvent("input", { inputType: "insertText" }).inputType, "insertText", "InputEvent");

     document.body.setAttribute("data-reads", "ok"); }`,
  320, 400));
assert.equal(reads.nodes.find(node => node.tag === "body").attributes["data-reads"], "ok");

// Hit testing, asked the other way round, in a document of its own so the
// coordinates stay readable.
//
// The trailing `<input>` is load-bearing rather than decoration: mixing an
// inline child in among block ones is what makes the renderer wrap the inline
// run in an anonymous block box, and hit testing used to walk DOM parents and
// miss that box's offset — so the input answered for points at the top of the
// document, in front of everything actually there. That mis-routed real clicks,
// not just `elementFromPoint`. Fixed in `hit_test.rs`; this keeps it fixed from
// the bridge's side, and `hit_testing_subtracts_the_offsets_of_anonymous_block_boxes`
// does from the renderer's.
const hits = JSON.parse(native.runBridgeHarness(
  `<style>body{margin:0}#outer{display:block;width:200px;height:100px;background:#eee}
     #inner{display:block;width:50px;height:50px;background:#ccc}</style>
   <div id="outer"><div id="inner"></div></div><input id="late">`,
  `{ const expect = (actual, wanted, what) => {
       if (JSON.stringify(actual) !== JSON.stringify(wanted))
         throw new Error(what + ": " + JSON.stringify(actual) + " is not " + JSON.stringify(wanted));
     };
     const outer = document.getElementById("outer");
     const inner = document.getElementById("inner");
     // The innermost box at the point, not the outermost: #inner covers the
     // top-left 50 square, and #outer the strip to the right of it.
     expect(document.elementFromPoint(10, 10) === inner, true, "elementFromPoint takes the innermost box");
     expect(document.elementFromPoint(150, 10) === outer, true, "and the box actually under the point");
     expect(document.elementsFromPoint(10, 10).includes(outer), true,
       "elementsFromPoint reports what the hit walked through");
     expect(document.elementFromPoint(-1, -1), null, "a point outside the viewport hits nothing");
     // The inline element in the anonymous block answers for its own box only.
     const late = document.getElementById("late");
     expect(document.elementFromPoint(10, 10) === late, false,
       "an inline element in an anonymous block does not answer for the document's origin");
     const box = late.getBoundingClientRect();
     expect(document.elementFromPoint(box.left + 5, box.top + 5) === late, true,
       "and is still hit-testable where it actually is");
     outer.setAttribute("data-hits", "ok"); }`,
  320, 180));
assert.equal(hits.nodes.find(node => node.attributes.id === "outer").attributes["data-hits"], "ok");

// Focus moves dispatch four events, not two: `focus` and `blur` do not bubble,
// so a framework delegating from the root sees nothing without `focusin` and
// `focusout`. Each carries the other end of the move as `relatedTarget`.
const focus = JSON.parse(native.runBridgeHarness(
  `<div id="wrap"><input id="one"><input id="two"></div>`,
  `{ const seen = [];
     const wrap = document.getElementById("wrap");
     const one = document.getElementById("one");
     const two = document.getElementById("two");
     for (const type of ["focusin", "focusout"])
       wrap.addEventListener(type, event => seen.push(
         type + ":" + event.target.id + ">" + (event.relatedTarget?.id || "body")));
     for (const type of ["focus", "blur"])
       one.addEventListener(type, event => seen.push(type + ":" + one.id));
     one.focus();
     two.focus();
     wrap.setAttribute("data-focus", seen.join(" ")); }`,
  320, 180));
assert.equal(
  focus.nodes.find(node => node.attributes.id === "wrap").attributes["data-focus"],
  "focus:one focusin:one>body blur:one focusout:one>two focusin:two>one",
  "focus and blur stay unbubbled while focusin and focusout bubble to the wrapper");

// Scrolling, asserted on pixels. The document is two full-viewport blocks of
// flat colour, so whichever one fills the frame says where the viewport is —
// a scroll offset that moved without the viewport following cannot pass this.
const SCROLLED = `<style>body{margin:0}
  #red{display:block;height:800px;background:#ff0000}
  #green{display:block;height:800px;background:#00ff00}</style>
<div id="red"></div><div id="green"></div>`;
const filling = script => {
  const snapshot = JSON.parse(native.runBridgeHarness(SCROLLED, script, 200, 200));
  return snapshot.paint_colors[0].rgba;
};
assert.equal(filling(`{}`), "#ff0000ff", "the document opens at the top");
assert.equal(filling(`{ scrollTo(0, 900); }`), "#00ff00ff", "window.scrollTo moves the viewport");
assert.equal(filling(`{ scrollTo({ top: 900 }); }`), "#00ff00ff", "and takes an option bag");
assert.equal(filling(`{ scrollBy(0, 500); scrollBy(0, 400); }`), "#00ff00ff", "scrollBy accumulates");
assert.equal(filling(`{ scrollTo(0, 900); scrollTo(0, 0); }`), "#ff0000ff", "and scrolls back");
assert.equal(filling(`{ document.getElementById("green").scrollIntoView(); }`), "#00ff00ff",
  "scrollIntoView brings an element into the viewport");
assert.equal(
  filling(`{ scrollTo(0, 900);
             if (scrollY !== 900 || pageYOffset !== 900 || scrollX !== 0 || pageXOffset !== 0)
               throw new Error("the scroll offsets do not read back what was set"); }`),
  "#00ff00ff", "scrollY and pageYOffset read the offset the viewport actually moved to");

// An element with its own scrollport, rather than the document's. #target sits
// 600px down a box only 200px tall, so nothing of it is in view until the box
// scrolls — and the box is the only thing that may scroll.
const inner = JSON.parse(native.runBridgeHarness(DOCUMENT,
  `{ const box = document.getElementById("box");
     const before = box.scrollTop;
     document.getElementById("target").scrollIntoView();
     box.setAttribute("data-scroll", before + ">" + Math.round(box.scrollTop)); }`,
  320, 400));
const scrolled = inner.nodes.find(node => node.attributes.id === "box");
const [before, after] = scrolled.attributes["data-scroll"].split(">").map(Number);
assert.equal(before, 0, "the box starts unscrolled");
assert.ok(after >= 600 && after <= 640,
  `scrollIntoView scrolls the ancestor scrollport to the element, not past it (got ${after})`);
