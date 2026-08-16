import { strict as assert } from "node:assert";
import { join } from "node:path";

import { native, testDir } from "./addon.mjs";

const scriptFixture = join(testDir, "fixtures/scripts");
const scriptSnapshot = JSON.parse(native.runDocumentScriptsHarness(
  join(scriptFixture, "index.html"),
  320,
  180,
));
const scriptTarget = scriptSnapshot.nodes.find((node) => node.attributes.id === "script-target");
assert(scriptTarget, "script fixture target reached the Rust tree");
assert.equal(scriptTarget.attributes["data-order"], "inline,async,defer,module,inline-module");
assert.match(scriptTarget.attributes["data-module-url"], /module\.js$/);
assert.equal(scriptTarget.attributes["data-dom-content-loaded"], "interactive");
assert.equal(scriptTarget.attributes["data-load"], "complete");
assert.equal(scriptTarget.attributes["data-module-load"], "fired",
  "an unqualified addEventListener from a module script binds to the window");
const interactiveSnapshot = JSON.parse(native.runDocumentScriptsHarness(
  join(testDir, "../../../examples/interactive/index.html"),
  720,
  520,
));
const interactiveDemo = interactiveSnapshot.nodes.find(node => node.attributes.id === "demo");
assert.equal(interactiveDemo.attributes["data-ready"], "true",
  "interactive acceptance example installs its event and animation script");
// The hardware example, which is the only one of these whose script depends on a
// `native:` module. Running it here is what catches an application that parses
// and then throws on evaluation — the marker is absent, rather than the document
// merely looking sparse.
const hardwareSnapshot = JSON.parse(native.runDocumentScriptsHarness(
  join(testDir, "../../../examples/hardware/index.html"),
  1180,
  820,
));
const hardwareHeader = hardwareSnapshot.nodes.find(node => node.attributes.id === "bar");
assert.equal(hardwareHeader.attributes["data-ready"], "true",
  "the hardware example reads blitsen/os and runs its script to the end");
// The counts come from the machine running this, so they are asserted as facts
// about any machine rather than as numbers: something has threads, and something
// is mounted.
assert(Number(hardwareHeader.attributes["data-threads"]) >= 1,
  `logical processors: ${hardwareHeader.attributes["data-threads"]}`);
assert(Number(hardwareHeader.attributes["data-volumes"]) >= 1,
  `volumes with capacity: ${hardwareHeader.attributes["data-volumes"]}`);
assert.equal(
  hardwareSnapshot.nodes.filter(node => node.attributes.class === "core").length,
  Number(hardwareHeader.attributes["data-threads"]),
  "one meter is built per logical processor",
);

const pongSnapshot = JSON.parse(native.runDocumentScriptsHarness(
  join(testDir, "../../../examples/pong/index.html"),
  720,
  520,
));
const pongGame = pongSnapshot.nodes.find(node => node.attributes.id === "game");
assert.equal(pongGame.attributes["data-ready"], "true",
  "Pong installs its input and animation loop from the three-file application");
assert.equal(pongGame.attributes["data-state"], "paused",
  "Pong starts in a playable serve state");
const pongFrames = JSON.parse(native.runDocumentAnimationHarness(
  join(testDir, "../../../examples/pong/index.html"),
  `__blitsenDispatchKeyboardEvent("keydown", { bubbles: true, cancelable: true,
     key: " ", code: "Space", repeat: false });
   __blitsenDispatchKeyboardEvent("keydown", { bubbles: true, cancelable: true,
     key: "w", code: "KeyW", repeat: false });`,
  60,
  960,
  640,
));
const pongNode = (snapshot, id) => snapshot.nodes.find(node => node.attributes.id === id);
assert.equal(pongNode(pongFrames[0], "game").attributes["data-state"], "playing",
  "Space serves the ball");
assert(pongNode(pongFrames.at(-1), "left-paddle").layout.y
  < pongNode(pongFrames[0], "left-paddle").layout.y, "W moves player one's paddle");
assert.notEqual(pongNode(pongFrames.at(-1), "ball").layout.x,
  pongNode(pongFrames[0], "ball").layout.x, "the ball advances through requestAnimationFrame");
// The game's own #fps readout is deliberately not asserted. The harness feeds
// JavaScript a fixed 1000/60 ms timestep and the game divides frames by those
// timestamps, so the readout reports ~60 however slow the renderer actually is.
// Real frame cost is measured against wall clock by `frames`; determinism of the
// rendered output is gated by `test:determinism`.
let scriptError;
try {
  native.runDocumentScriptsHarness(join(scriptFixture, "error.html"), 320, 180);
} catch (error) {
  scriptError = error;
}
assert(scriptError, "broken external script throws");
assert.match(String(scriptError.stack ?? scriptError), /intentional script fixture failure/);
assert.match(String(scriptError.stack ?? scriptError), /broken\.js/);
await Bun.sleep(15);
assert.equal(globalThis.__blitsenDisposedTimerRan, undefined,
  "document reload cancels timers owned by the previous context");

const harnessMode = JSON.parse(native.runBridgeHarness(
  `<p id="mode"></p>`,
  `{ if (typeof __blitsenInjectMouseEvent !== "function" ||
         typeof __blitsenInjectPointerAt !== "function" ||
         typeof __blitsenDomCallCount !== "function")
       throw new Error("test harness mode did not install its test-only globals");
     document.getElementById("mode").setAttribute("data-mode", "test-harness"); }`,
));
assert.equal(harnessMode.nodes.find(node => node.attributes.id === "mode")
  .attributes["data-mode"], "test-harness");
