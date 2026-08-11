// Measures what one DOM property read or write costs through the bridge, which
// is the evidence behind open technical question 4 (do hot properties need a
// cache on the JavaScript wrapper?).
//
// Every operation runs against the real Pong document, so the tree, the cascade
// and the wrapper table are the ones the acceptance build uses. A plain
// JavaScript object property is measured alongside as the floor: the difference
// between the two is what crossing into Rust costs.
//
// usage: bun property-cost.mjs <addon.node>
import { createRequire } from "node:module";
import { join, resolve } from "node:path";

const addonPath = process.argv[2];
if (!addonPath) throw new Error("usage: bun property-cost.mjs <addon.node>");
const native = createRequire(import.meta.url)(resolve(addonPath));
const repository = resolve(import.meta.dir, "../../..");

native.runDocumentScriptsHarness(join(repository, "examples/pong/index.html"), 960, 640);
native.evaluateDocumentHarness(`(() => {
  const ball = document.getElementById("ball");
  const paddle = document.getElementById("left-paddle");
  const fps = document.getElementById("fps");
  const plain = { top: "0px" };
  const cached = { top: null };
  let sink = null;

  // Median of repeated batches: one batch is long enough to swamp timer
  // resolution, and the median discards the batch that was descheduled.
  const measure = (iterations, batches, operation) => {
    const samples = [];
    for (let batch = 0; batch < batches; batch++) {
      const started = performance.now();
      for (let index = 0; index < iterations; index++) operation(index);
      samples.push((performance.now() - started) * 1000 / iterations);
    }
    samples.sort((left, right) => left - right);
    return Math.round(samples[batches >> 1] * 1000) / 1000;
  };

  const iterations = 20000;
  const batches = 7;
  globalThis.__blitsenPropertyCost = {
    iterations,
    batches,
    plainWrite: measure(iterations, batches, index => { plain.top = index + "px"; }),
    plainRead: measure(iterations, batches, () => { sink = plain.top; }),
    styleWrite: measure(iterations, batches, index => { ball.style.top = index + "px"; }),
    styleWriteUnchanged: measure(iterations, batches, () => { ball.style.top = "17px"; }),
    styleWriteCached: measure(iterations, batches, () => {
      if (cached.top !== "17px") { cached.top = "17px"; ball.style.top = "17px"; }
    }),
    styleRead: measure(iterations, batches, () => { sink = ball.style.top; }),
    attributeWrite: measure(iterations, batches, index => paddle.setAttribute("data-cost", String(index))),
    attributeRead: measure(iterations, batches, () => { sink = paddle.getAttribute("data-cost"); }),
    textWrite: measure(iterations, batches, index => { fps.textContent = String(index & 63); }),
    textRead: measure(iterations, batches, () => { sink = fps.textContent; }),
    lookup: measure(iterations, batches, () => { sink = document.getElementById("ball"); }),
    // The layout-dependent read, both ways round. Interleaved with a write it
    // forces a flush and is charged the whole style and layout pass; on a clean
    // tree it is only a bridge crossing.
    boundingRectForced: measure(200, batches, index => {
      ball.style.left = (index & 31) + "px";
      sink = ball.getBoundingClientRect().x;
    }),
    boundingRectClean: measure(2000, batches, () => { sink = ball.getBoundingClientRect().x; }),
    // What one Pong frame's render() actually writes.
    pongRender: measure(iterations, batches, index => {
      paddle.style.top = index + "px";
      ball.style.left = index + "px";
      ball.style.top = index + "px";
    }),
  };
  if (sink === undefined) throw new Error("unreachable");
})()`);

console.log(JSON.stringify(globalThis.__blitsenPropertyCost));
