import { strict as assert } from "node:assert";
import { native } from "./addon.mjs";

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
