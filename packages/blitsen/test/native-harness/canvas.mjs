import { strict as assert } from "node:assert";
import { native } from "./addon.mjs";

// `<canvas>` 2D (issue #99). The renderer's own tests cover what the recorded
// scene composites to; what is asserted here is the half that only exists in
// JavaScript — the state machine, the value objects, and the readbacks that
// have to agree with what was drawn.
//
// Pixels are read back through `getImageData` rather than out of the rendered
// frame wherever the question is about the canvas rather than about the page,
// because that is the answer an application would act on.

// A 4-by-2 PNG: two red pixels, then transparent, then blue, over two rows.
const SWATCH = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAQAAAACCAYAAAB/qH1jAAAA"
  + "FUlEQVR4nGP4z8DwH4QZEOD/f2QMALFwC/Vw91JXAAAAAElFTkSuQmCC";

const canvas = JSON.parse(native.runBridgeHarness(
  `<canvas id="c" width="40" height="20"></canvas><img id="swatch" src="${SWATCH}"><img id="shot">`,
  `{ const expect = (condition, message) => { if (!condition) throw new Error(message); };
     const canvas = document.getElementById("c");
     expect(canvas instanceof HTMLCanvasElement && canvas instanceof Element,
       "a <canvas> wrapper is an HTMLCanvasElement");
     expect(canvas.width === 40 && canvas.height === 20,
       "width and height are the backing store: " + [canvas.width, canvas.height]);
     const ctx = canvas.getContext("2d");
     expect(ctx instanceof CanvasRenderingContext2D, "getContext('2d') is a 2D context");
     expect(ctx === canvas.getContext("2d"), "the same context comes back every time");
     expect(ctx.canvas === canvas, "the context names the canvas it draws into");
     expect(canvas.getContext("webgl") === null, "an unimplemented context type is null");

     // Defaults, as a browser reports them.
     expect(ctx.fillStyle === "#000000", "default fillStyle: " + ctx.fillStyle);
     expect(ctx.font === "10px sans-serif", "default font: " + ctx.font);
     expect(ctx.globalCompositeOperation === "source-over", "default composite");
     expect(ctx.lineWidth === 1 && ctx.lineCap === "butt" && ctx.lineJoin === "miter",
       "default line style");

     // Colour parsing, in the syntaxes a canvas is configured with.
     const colours = [["red", "#ff0000"], ["#0f0", "#00ff00"], ["#0000ff80", null],
       ["rgb(1, 2, 3)", "#010203"], ["rgba(0, 0, 0, 0.5)", null],
       ["hsl(120 100% 50%)", "#00ff00"], ["rgb(50% 0% 0%)", "#800000"]];
     for (const [written, read] of colours) {
       ctx.fillStyle = written;
       if (read !== null && ctx.fillStyle !== read)
         throw new Error("fillStyle " + written + " read back as " + ctx.fillStyle);
     }
     ctx.fillStyle = "#123456";
     ctx.fillStyle = "not a colour";
     expect(ctx.fillStyle === "#123456", "an unparseable colour is ignored, not applied");

     // The font shorthand, and the normalized form it reads back as.
     ctx.font = "bold italic 24px 'Some Family', sans-serif";
     expect(ctx.font === "italic 700 24px 'Some Family', sans-serif",
       "font round-trip keeps the family names as they were written: " + ctx.font);
     ctx.font = "nonsense";
     expect(ctx.font.startsWith("italic 700 24px"), "an unparseable font is ignored");
     ctx.font = "16px sans-serif";

     // Drawing, then reading the same pixels back.
     ctx.fillStyle = "rgb(0, 128, 255)";
     ctx.fillRect(0, 0, 10, 10);
     const patch = ctx.getImageData(2, 2, 1, 1);
     expect(patch instanceof ImageData && patch.width === 1 && patch.height === 1,
       "getImageData answers an ImageData of the size asked for");
     expect(Array.from(patch.data).join() === "0,128,255,255",
       "a filled rectangle reads back as what was filled: " + Array.from(patch.data));
     expect(Array.from(ctx.getImageData(30, 15, 1, 1).data).join() === "0,0,0,0",
       "and nothing was drawn where nothing was drawn");

     // The transform stack.
     ctx.save();
     ctx.translate(20, 0);
     ctx.scale(2, 1);
     const matrix = ctx.getTransform();
     expect(matrix instanceof DOMMatrix && matrix.a === 2 && matrix.e === 20,
       "getTransform reports the current transform: " + matrix);
     ctx.fillStyle = "#00ff00";
     ctx.fillRect(0, 0, 5, 5);
     ctx.restore();
     expect(ctx.getTransform().isIdentity, "restore puts the transform back");
     expect(Array.from(ctx.getImageData(25, 2, 1, 1).data).join() === "0,255,0,255",
       "the transform moved and scaled the fill: " + Array.from(ctx.getImageData(25, 2, 1, 1).data));

     // Clipping, which the renderer records as a layer the batch must balance.
     ctx.save();
     ctx.beginPath();
     ctx.rect(0, 12, 4, 4);
     ctx.clip();
     ctx.fillStyle = "#ff00ff";
     ctx.fillRect(0, 12, 40, 8);
     ctx.restore();
     expect(Array.from(ctx.getImageData(1, 13, 1, 1).data).join() === "255,0,255,255",
       "the clipped fill lands inside the clip");
     expect(Array.from(ctx.getImageData(10, 13, 1, 1).data).join() === "0,0,0,0",
       "and stops at it: " + Array.from(ctx.getImageData(10, 13, 1, 1).data));
     ctx.fillStyle = "#ffff00";
     ctx.fillRect(10, 13, 2, 2);
     expect(Array.from(ctx.getImageData(10, 13, 1, 1).data).join() === "255,255,0,255",
       "and the clip is gone once it is restored");

     // clearRect, both the whole-canvas form and a rectangle of it.
     ctx.clearRect(0, 0, 4, 4);
     expect(Array.from(ctx.getImageData(1, 1, 1, 1).data).join() === "0,0,0,0",
       "clearRect erases the rectangle");
     expect(Array.from(ctx.getImageData(6, 6, 1, 1).data).join() === "0,128,255,255",
       "and leaves the rest of the canvas");
     ctx.clearRect(0, 0, 40, 20);
     expect(Array.from(ctx.getImageData(6, 6, 1, 1).data).join() === "0,0,0,0",
       "and a full-canvas clear erases all of it");

     // Paths, strokes and hit testing.
     const path = new Path2D();
     path.rect(4, 4, 12, 12);
     expect(ctx.isPointInPath(path, 10, 10), "a point inside the path is inside it");
     expect(!ctx.isPointInPath(path, 30, 10), "and one outside it is not");
     ctx.lineWidth = 4;
     ctx.strokeStyle = "#ffffff";
     ctx.stroke(path);
     expect(Array.from(ctx.getImageData(4, 10, 1, 1).data).join() === "255,255,255,255",
       "a stroke paints along the path");
     expect(Array.from(ctx.getImageData(10, 10, 1, 1).data).join() === "0,0,0,0",
       "and not inside it");
     expect(ctx.isPointInStroke(path, 4, 10), "the pen's own region is what isPointInStroke tests");
     expect(!ctx.isPointInStroke(path, 10, 10), "not the region the path encloses");
     ctx.lineWidth = 1;

     // An SVG path string, which is how a Path2D is usually built.
     const arrow = new Path2D("M2 2 L8 2 L5 8 Z");
     expect(ctx.isPointInPath(arrow, 5, 4), "an SVG path describes the region it names");

     // Gradients.
     ctx.clearRect(0, 0, 40, 20);
     const gradient = ctx.createLinearGradient(0, 0, 40, 0);
     gradient.addColorStop(0, "#ff0000");
     gradient.addColorStop(1, "#0000ff");
     ctx.fillStyle = gradient;
     expect(ctx.fillStyle === gradient, "a gradient reads back as itself");
     ctx.fillRect(0, 0, 40, 20);
     const [left, right] = [ctx.getImageData(1, 10, 1, 1).data, ctx.getImageData(38, 10, 1, 1).data];
     expect(left[0] > 200 && left[2] < 60, "the gradient starts at its first stop: " + Array.from(left));
     expect(right[2] > 200 && right[0] < 60, "and ends at its last: " + Array.from(right));
     let refused = null;
     try { gradient.addColorStop(2, "#000"); } catch (error) { refused = error.name; }
     expect(refused === "IndexSizeError", "an out-of-range stop offset is refused: " + refused);

     // globalAlpha and a composite operation.
     ctx.clearRect(0, 0, 40, 20);
     ctx.globalAlpha = 0.5;
     ctx.fillStyle = "#ff0000";
     ctx.fillRect(0, 0, 10, 10);
     ctx.globalAlpha = 1;
     const faded = ctx.getImageData(5, 5, 1, 1).data;
     expect(faded[0] === 255 && faded[3] === 128,
       "globalAlpha applies to the paint, not to the colour: " + Array.from(faded));
     ctx.globalCompositeOperation = "destination-out";
     ctx.fillRect(0, 0, 5, 5);
     ctx.globalCompositeOperation = "source-over";
     expect(Array.from(ctx.getImageData(2, 2, 1, 1).data).join() === "0,0,0,0",
       "destination-out erases what it covers");
     expect(ctx.getImageData(7, 7, 1, 1).data[3] === 128, "and leaves what it does not");

     // putImageData writes pixels through, ignoring the transform and the clip.
     const pixels = new ImageData(2, 2);
     pixels.data.set([9, 8, 7, 255], 0);
     ctx.save();
     ctx.translate(100, 100);
     ctx.putImageData(pixels, 30, 15);
     ctx.restore();
     expect(Array.from(ctx.getImageData(30, 15, 1, 1).data).join() === "9,8,7,255",
       "putImageData lands where it was told, untransformed: "
       + Array.from(ctx.getImageData(30, 15, 1, 1).data));

     // Text.
     ctx.font = "16px sans-serif";
     const metrics = ctx.measureText("Blitsen");
     expect(metrics instanceof TextMetrics && metrics.width > 0,
       "measureText reports an advance: " + metrics.width);
     expect(metrics.actualBoundingBoxAscent > 0 && metrics.fontBoundingBoxAscent > 0,
       "and ink above the baseline: " + metrics.actualBoundingBoxAscent);
     expect(ctx.measureText("").width === 0, "an empty string measures zero");
     ctx.clearRect(0, 0, 40, 20);
     ctx.fillStyle = "#ffffff";
     ctx.fillText("I", 4, 16);
     const inked = ctx.getImageData(0, 0, 40, 20).data;
     let painted = 0;
     for (let index = 3; index < inked.length; index += 4) if (inked[index] > 0) painted++;
     expect(painted > 0, "fillText paints glyphs");

     // An image source, decoded by the same seam an <img> is.
     ctx.clearRect(0, 0, 40, 20);
     const image = document.getElementById("swatch");
     expect(image.complete && image.naturalWidth === 4, "the fixture image decoded");
     ctx.drawImage(image, 0, 0);
     expect(ctx.getImageData(0, 0, 1, 1).data[0] > 200,
       "drawImage draws the decoded image: " + Array.from(ctx.getImageData(0, 0, 1, 1).data));
     ctx.drawImage(image, 0, 0, 2, 1, 10, 10, 8, 4);
     expect(ctx.getImageData(12, 11, 1, 1).data[0] > 200,
       "and the nine-argument form scales the source rectangle it was given");

     // A pattern, which is the same source through a brush.
     ctx.clearRect(0, 0, 40, 20);
     const pattern = ctx.createPattern(image, "repeat");
     expect(pattern instanceof CanvasPattern, "createPattern makes a CanvasPattern");
     ctx.fillStyle = pattern;
     ctx.fillRect(0, 0, 40, 20);
     expect(ctx.getImageData(0, 0, 1, 1).data[0] > 200, "a pattern fills with its source");

     // An image paint at less than full opacity. The renderer refuses an
     // opacity on an image sampler outright rather than approximating it, so
     // this is a layer — and a canvas that got it wrong would take the process
     // down rather than draw the wrong thing.
     ctx.clearRect(0, 0, 40, 20);
     ctx.globalAlpha = 0.5;
     ctx.drawImage(image, 0, 0);
     ctx.fillStyle = pattern;
     ctx.fillRect(20, 0, 20, 20);
     ctx.globalAlpha = 1;
     expect(ctx.getImageData(0, 0, 1, 1).data[3] === 128,
       "a half-transparent drawImage lands at half alpha: "
       + Array.from(ctx.getImageData(0, 0, 1, 1).data));
     expect(ctx.getImageData(21, 1, 1, 1).data[3] === 128,
       "and so does a half-transparent pattern fill");

     // Encoding.
     ctx.clearRect(0, 0, 40, 20);
     ctx.fillStyle = "#ff8800";
     ctx.fillRect(0, 0, 40, 20);
     const url = canvas.toDataURL();
     expect(url.startsWith("data:image/png;base64,") && url.length > 100,
       "toDataURL encodes a PNG: " + url.slice(0, 40));
     expect(canvas.toDataURL("image/jpeg", 0.9).startsWith("data:image/jpeg;base64,"),
       "and a JPEG when asked for one");

     // Resizing clears the canvas and resets the context.
     ctx.fillStyle = "#ff0000";
     ctx.translate(5, 5);
     canvas.width = 24;
     expect(canvas.width === 24, "the width setter reflects the attribute");
     expect(ctx.fillStyle === "#000000" && ctx.getTransform().isIdentity,
       "resizing resets the context state");
     expect(Array.from(ctx.getImageData(0, 0, 1, 1).data).join() === "0,0,0,0",
       "and clears what was drawn");

     // A canvas that was never in the document still draws and still encodes.
     const detached = document.createElement("canvas");
     detached.width = 8;
     detached.height = 8;
     const offscreen = detached.getContext("2d");
     offscreen.fillStyle = "#00ff00";
     offscreen.fillRect(0, 0, 8, 8);
     expect(Array.from(offscreen.getImageData(1, 1, 1, 1).data).join() === "0,255,0,255",
       "a canvas made with createElement draws while it is detached");
     expect(detached.toDataURL().startsWith("data:image/png;base64,"),
       "and encodes without ever being connected");
     ctx.drawImage(detached, 0, 0);
     expect(Array.from(ctx.getImageData(1, 1, 1, 1).data).join() === "0,255,0,255",
       "and is itself a drawable source");

     // The readback path end to end: an encoded canvas is an ordinary image
     // source, decoded by the same seam that decodes an <img src>.
     const shot = document.getElementById("shot");
     shot.src = url;
     __blitsenAnimationFrameTick(0);
     expect(shot.complete && shot.naturalWidth === 40 && shot.naturalHeight === 20,
       "a canvas encoded with toDataURL decodes back as an image: "
       + [shot.complete, shot.naturalWidth, shot.naturalHeight]);

     canvas.setAttribute("data-canvas", "ok"); }`,
  200,
  120,
));

assert.equal(canvas.nodes.find(node => node.attributes.id === "c").attributes["data-canvas"], "ok");
