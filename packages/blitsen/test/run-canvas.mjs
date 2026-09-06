import { runExample } from "./build-addon.mjs";

await runExample({
  purpose: "canvas demo", example: "canvas", width: 700, height: 620, title: "canvas 2D",
  hints: [
    "canvas 2D: paths, gradients, patterns, text, images and compositing, drawn into",
    "the same frame as the DOM. Expect six orbiting discs inside a dashed ring, a",
    "patterned band clipped to a rounded rectangle, three labels at three text anchors,",
    "a DOM chip on top reporting a pixel read back with getImageData, and a thumbnail",
    "below the stage that is the canvas encoded through toDataURL.",
  ],
});
