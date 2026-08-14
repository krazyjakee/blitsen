import { strict as assert } from "node:assert";
import { join } from "node:path";

import { native } from "./addon.mjs";

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
     const swapped = document.createElement("strong"); swapped.id = "swapped";
     expect(a.replaceChild(swapped, replacement) === replacement && a.firstChild === swapped
       && replacement.parentNode === null, "replaceChild");
     let refused = false;
     try { b.replaceChild(document.createElement("i"), swapped); } catch { refused = true; }
     expect(refused, "replaceChild refuses a node that is not its child");
     a.setAttribute("data-tree", "ok"); }`,
  320,
  180,
));
const treeById = new Map(treeSnapshot.nodes.map((node) => [node.attributes.id, node]));
assert.equal(treeById.get("a").attributes["data-tree"], "ok");
assert.equal(treeById.get("swapped").parent, treeById.get("a").handle);
assert.equal(treeById.get("two").parent, treeById.get("b").handle);
for (const removedId of ["zero", "one", "three", "replacement"])
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

     // Read off the instance as often as off the interface: Monaco writes
     // \`child.nodeType === child.ELEMENT_NODE\`, and an undefined constant there
     // is a comparison that quietly never holds rather than an error.
     const text = document.createTextNode("x");
     expect(Node.ELEMENT_NODE === 1 && Node.ATTRIBUTE_NODE === 2 && Node.TEXT_NODE === 3 &&
       Node.CDATA_SECTION_NODE === 4 && Node.ENTITY_REFERENCE_NODE === 5 && Node.ENTITY_NODE === 6 &&
       Node.PROCESSING_INSTRUCTION_NODE === 7 && Node.COMMENT_NODE === 8 && Node.DOCUMENT_NODE === 9 &&
       Node.DOCUMENT_TYPE_NODE === 10 && Node.DOCUMENT_FRAGMENT_NODE === 11 && Node.NOTATION_NODE === 12,
       "the node-type constants are on the Node interface");
     expect(span.nodeType === span.ELEMENT_NODE && text.nodeType === text.TEXT_NODE &&
       comment.nodeType === comment.COMMENT_NODE && document.nodeType === document.DOCUMENT_NODE &&
       Element.ELEMENT_NODE === 1 && Node.prototype.ELEMENT_NODE === 1,
       "and reachable through every node, the document and the subclasses");
     const constant = Object.getOwnPropertyDescriptor(Node, "ELEMENT_NODE");
     expect(constant.value === 1 && !constant.writable && constant.enumerable && !constant.configurable,
       "declared read-only the way a browser declares them");
     try { Node.ELEMENT_NODE = 99; } catch { /* strict mode throws, sloppy mode ignores */ }
     expect(Node.ELEMENT_NODE === 1 && span.ELEMENT_NODE === 1, "and not overwritable");
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

