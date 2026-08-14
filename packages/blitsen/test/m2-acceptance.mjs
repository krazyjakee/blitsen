// M2 acceptance gate (issue #43). Everything here runs against examples/interactive
// through the real input path: coordinates are hit-tested against the laid-out tree
// and the resulting event is dispatched at whatever that resolves to, exactly as the
// native window does. Nothing picks a target by hand.
import { strict as assert } from "node:assert";
import { createRequire } from "node:module";
import { join } from "node:path";

const addonPath = process.argv[2];
if (!addonPath) throw new Error("usage: bun m2-acceptance.mjs <addon.node>");
const native = createRequire(import.meta.url)(addonPath);

const entrypoint = join(import.meta.dir, "../../../examples/interactive/index.html");
const WIDTH = 960;
const HEIGHT = 640;
const FRAMES = 60;

// Runs once after `load`, before the frame loop. Assertions that need the two sides
// of a single event live here; everything durable is stashed on #demo so the frame
// snapshots below can assert on it from Bun.
const setup = `{
  const demo = document.getElementById("demo");
  const control = document.getElementById("control");
  const order = document.getElementById("event-order");
  const record = (name, value) => demo.setAttribute("data-m2-" + name, String(value));

  const rect = control.getBoundingClientRect();
  if (!(rect.width > 0 && rect.height > 0)) throw new Error("control has no layout box");
  const widthBefore = demo.offsetWidth;

  // The press and then the click, the way the native window delivers them:
  // focus is the mousedown's default action and activation the click's.
  __blitsenInjectPointerAt("mousedown", rect.x + rect.width / 2, rect.y + rect.height / 2);
  const hit = __blitsenInjectPointerAt("click", rect.x + rect.width / 2, rect.y + rect.height / 2);
  if (!hit) throw new Error("hit test found no node under the control centre");
  record("hit-path", hit.path.map(node => node.id || node.nodeName).join(">"));
  record("click-order", order.textContent);
  record("width-before", widthBefore);
  record("width-after", demo.offsetWidth);
  record("focus", document.activeElement.id);

  const allowed = __blitsenDispatchKeyboardEvent("keydown", {
    bubbles: true, cancelable: true, key: "ArrowLeft", code: "ArrowLeft", repeat: false,
  });
  record("key-order", order.textContent);
  record("key-prevented", allowed === false);
}`;

const frames = JSON.parse(
  native.runDocumentAnimationHarness(entrypoint, setup, FRAMES, WIDTH, HEIGHT),
);
assert.equal(frames.length, FRAMES, "the harness advanced the full frame budget");

const node = (frame, id) => frame.nodes.find(entry => entry.attributes.id === id);
const first = frames[0];
const last = frames.at(-1);
const data = key => node(first, "demo").attributes[`data-m2-${key}`];

// --- Click and keyboard events dispatch to JS listeners with correct propagation ---
assert.match(data("hit-path"), /(^|>)control(>|$)/,
  "the hit test resolved the pointer coordinates to the control, or a child of it");
assert.equal(data("click-order"), "click: window↓  document↓  stage↓  stage↑  document↑  window↑",
  "click runs capture root-to-target then bubbles target-to-root");
assert.equal(data("key-order"), "keydown: window↓  document↓  stage↓  stage↑  document↑  window↑",
  "keydown propagates through the same three-level path as click");

// --- Default actions: the press moves focus, and preventDefault is honoured ---
assert.equal(data("focus"), "control", "pressing on the control made it the active element");
assert.equal(data("key-prevented"), "true",
  "the control's keydown listener cancelled the event through preventDefault");

// --- Style and class mutation from JS relayouts correctly ---
assert.equal(node(first, "demo").attributes.class.includes("expanded"), true,
  "the click listener's classList.toggle reached the Rust tree");
assert(Number(data("width-after")) > Number(data("width-before")),
  `restyle relaid out the demo: ${data("width-before")}px -> ${data("width-after")}px`);
assert.equal(node(first, "demo").layout.width, Number(data("width-after")),
  "the layout Blitz reports agrees with the width JavaScript read back synchronously");

// --- requestAnimationFrame drives a smooth animation ---
const orbLeft = frame => {
  const inline = node(frame, "orb").inline_style;
  const match = /left:\s*([\d.]+)px/.exec(inline);
  assert(match, `orb has no animated left offset: ${inline}`);
  return Number(match[1]);
};
const positions = frames.map(orbLeft);
assert(new Set(positions).size > FRAMES / 2,
  "the orb takes a distinct position on most frames rather than stepping in bursts");
// The orb rests against the left edge, so ArrowLeft is absorbed by the bounce clamp and
// travel stays rightward. What the key must prove is that the listener ran and restyled.
assert(node(first, "demo").attributes.class.includes("moving-left"),
  "the ArrowLeft listener mutated the class list");
assert.equal(node(first, "input-state").text_content, "ArrowLeft dispatched",
  "the ArrowLeft listener mutated text content");
const steps = positions.slice(1).map((value, index) => value - positions[index]);
const nominal = (1000 / 60) * 0.16;
for (const [index, step] of steps.entries()) {
  assert(Math.abs(step - nominal) < nominal * 0.01,
    `frame ${index + 2} advanced ${step.toFixed(4)}px, not the ${nominal.toFixed(4)}px a 60 Hz step implies`);
}
assert(node(last, "orb").layout.x > node(first, "orb").layout.x,
  "the animated orb's laid-out box tracks its animated inline style");

console.log(`M2 acceptance: ${FRAMES} frames, propagation, focus, preventDefault, and relayout all verified.`);
