// The native harness: one linear pass over the runtime the addon installs.
//
// Each module below is a section of checks that runs on evaluation. They are
// imported dynamically, one awaited at a time, because the order is
// load-bearing: several sections lean on state an earlier one left in this
// realm, and some assert on real elapsed time. Static imports would not do —
// a section suspended on a top-level `await` lets its siblings evaluate, so the
// native work of a later section runs during an earlier section's timer window
// and starves it. The addon itself is loaded once, by `native-harness/addon.mjs`.

await import("./native-harness/document-scripts.mjs");
await import("./native-harness/bridge.mjs");
await import("./native-harness/events.mjs");
await import("./native-harness/pointer-events.mjs");
await import("./native-harness/forms.mjs");
await import("./native-harness/text-editing.mjs");
await import("./native-harness/dom.mjs");
await import("./native-harness/read-back-and-scrolling.mjs");
await import("./native-harness/ranges.mjs");
const { styled } = await import("./native-harness/style.mjs");
await import("./native-harness/layout-and-images.mjs");
await import("./native-harness/canvas.mjs");
await import("./native-harness/runtime-surface.mjs");
await import("./native-harness/network.mjs");
await import("./native-harness/web-socket.mjs");
await import("./native-harness/messaging.mjs");
await import("./native-harness/audio.mjs");
const { displayed } = await import("./native-harness/native-modules.mjs");

console.log("native modules passed", `clipboard=${displayed ? "round-tripped" : "skipped"}`);
console.log("bridge harness passed", process.platform, process.arch,
  `style=${styled.attributes["data-style-call-us"]}us/call`);
