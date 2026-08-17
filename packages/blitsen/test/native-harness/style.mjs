import { strict as assert } from "node:assert";
import { join } from "node:path";

import { native } from "./addon.mjs";

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
export const styled = styleSnapshot.nodes.find((node) => node.attributes.id === "styled");
assert.equal(styled.attributes["data-style"], "ok");
assert.match(styled.inline_style, /left:\s*5px/);
assert.doesNotMatch(styled.inline_style, /definitely-invalid/);
assert.equal(styled.layout.width, 90);

const tokenizedInlineStyle = JSON.parse(native.runBridgeHarness(
  `<div id="tokens" style='background-image:url("data:image/svg+xml;charset=utf-8,%3Csvg%3E%3C/svg%3E");--quoted:"left;right:tail";--escaped:semi\\;colon\\:tail;--commented:left/* ; : */right;color:rgb(1,2,3)!important;color:blue'></div>`,
  `{ const element = document.getElementById("tokens");
     const style = element.style;
     const names = ["background-image", "--quoted", "--escaped", "--commented", "color"];
     const before = names.map(name => style.getPropertyValue(name));
     if (before.some(value => value === "") || !before[0].includes("data:image/svg+xml;charset=utf-8") ||
         before[1] !== '"left;right:tail"' || before[4] !== "rgb(1, 2, 3)")
       throw new Error("inline declaration parsing: " + JSON.stringify(before));
     style.width = "17px";
     if (JSON.stringify(names.map(name => style.getPropertyValue(name))) !== JSON.stringify(before))
       throw new Error("setting a property corrupted another declaration");
     if (!style.cssText.includes("!important") || style.removeProperty("width") !== "17px" ||
         JSON.stringify(names.map(name => style.getPropertyValue(name))) !== JSON.stringify(before))
       throw new Error("removal corrupted declaration order, importance, or tokens");
     style.color = "green";
     if (style.color !== "green" || style.cssText.includes("green !important"))
       throw new Error("assignment did not replace an important declaration");
     element.setAttribute("data-inline-tokens", "ok"); }`,
  320,
  180,
));
assert.equal(tokenizedInlineStyle.nodes.find(node => node.attributes.id === "tokens")
  .attributes["data-inline-tokens"], "ok");

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
     const animationChecks = __blitsenDomCallCount("isAnimating");
     const pendingFrame = requestAnimationFrame(() => {});
     expect(__blitsenAnimationFramesPending(), "a queued animation frame keeps the host turning");
     expect(__blitsenDomCallCount("isAnimating") === animationChecks,
       "pending work short-circuits before querying the renderer");
     cancelAnimationFrame(pendingFrame);
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

// `<link rel="stylesheet">` load/error, which is what a theme switcher and
// every deferred-CSS loader waits on. The assertion that matters is inside the
// handler: a `load` that fired before the sheet reached the cascade would still
// arrive here and still read the width the sheet was about to replace.
const THEME = "data:text/css;base64,I3RoZW1lZHt3aWR0aDoxNTBweH0=";
const SWAPPED = "data:text/css;base64,I3RoZW1lZHt3aWR0aDo2MHB4fQ==";
const linked = JSON.parse(native.runBridgeHarness(
  `<div id="themed"></div>`,
  `{ const expect = (condition, message) => { if (!condition) throw new Error(message); };
     const seen = [];
     const themed = document.getElementById("themed");
     const width = () => getComputedStyle(themed).width;

     const link = document.createElement("link");
     expect(link instanceof HTMLLinkElement, "a <link> wrapper is an HTMLLinkElement");
     link.rel = "stylesheet";
     link.href = ${JSON.stringify(THEME)};
     link.onload = event => seen.push([event.type, width()]);
     link.onerror = () => seen.push("wrong-error");
     document.head.appendChild(link);
     expect(seen.length === 0, "load is not delivered synchronously");
     expect(__blitsenAnimationFramesPending(), "a sheet in flight keeps the host turning");
     __blitsenAnimationFrameTick(0);
     expect(JSON.stringify(seen) === JSON.stringify([["load", "150px"]]),
       "a linked sheet fires load once, with the cascade already reading it: " +
       JSON.stringify(seen));
     expect(!__blitsenAnimationFramesPending(), "a settled sheet stops holding the host open");
     __blitsenAnimationFrameTick(16);
     expect(seen.length === 1, "load is delivered exactly once");

     // The theme swap: a new address on the same element is a new request, and
     // owes a second outcome to the handler that is still installed.
     link.href = ${JSON.stringify(SWAPPED)};
     __blitsenAnimationFrameTick(32);
     expect(JSON.stringify(seen[1]) === JSON.stringify(["load", "60px"]),
       "a rewritten href loads again: " + JSON.stringify(seen));

     const missing = document.createElement("link");
     missing.rel = "stylesheet";
     missing.href = "https://example.com/theme.css";
     missing.onload = () => seen.push("wrong-load");
     missing.addEventListener("error", event => seen.push(event.type));
     document.head.appendChild(missing);
     __blitsenAnimationFrameTick(48);
     expect(seen[2] === "error",
       "a sheet that cannot be fetched fires error: " + JSON.stringify(seen));

     // Nothing is fetched for a preload hint, so nothing is owed for one — and
     // an element owed nothing must not hold the host open waiting for it.
     const hint = document.createElement("link");
     hint.rel = "preload";
     hint.href = ${JSON.stringify(THEME)};
     hint.onload = () => seen.push("wrong-preload-load");
     hint.onerror = () => seen.push("wrong-preload-error");
     document.head.appendChild(hint);
     expect(!__blitsenAnimationFramesPending(), "a preload hint is waiting on nothing");
     __blitsenAnimationFrameTick(64);
     expect(seen.length === 3, "only the stylesheets reported: " + JSON.stringify(seen));

     expect(__blitsenForcedLayoutsThisFrame() === 0, "the settle poll charges no forced layout");
     themed.setAttribute("data-linked", "ok"); }`,
  320,
  180,
));
assert.equal(linked.nodes.find(node => node.attributes.id === "themed").attributes["data-linked"],
  "ok");
assert.equal(linked.nodes.find(node => node.attributes.id === "themed").layout.width, 60,
  "the swapped-in sheet is the one the frame was laid out with");

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
