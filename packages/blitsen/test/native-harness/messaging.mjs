// Message ports and structured clone, in the document's own context.
//
// Workers are not exercised here and cannot be: `runBridgeHarness` installs the
// bridge over a document with no application behind it, so there is no file a
// worker script could be loaded from — which the constructor says, and this
// asserts. What a worker *does* with a message is covered end to end against the
// real runtime binary in `crates/blitsen-runtime/tests/workers.rs`, where there
// is an application and a second thread. What is checked here is the half both
// share: the codec, and the ports it travels over.
//
// Delivery is at the animation-frame tick, so every case below turns the frame
// the way the host would rather than awaiting a promise the bridge never makes.
import { strict as assert } from "node:assert";

import { native } from "./addon.mjs";

const probe = JSON.parse(native.runBridgeHarness(
  `<div id="messaging"></div>`,
  `{ const trail = globalThis.__blitsenMessaging = { received: [], errors: [] };
     const channel = new MessageChannel();
     const shared = new ArrayBuffer(8);
     const message = {
       text: "hello", when: new Date(1000), set: new Set([1, 2, 3]),
       map: new Map([["k", "v"]]), viewA: new Uint8Array(shared, 0, 4),
       viewB: new Uint8Array(shared, 4, 4), re: /ab+c/gi, err: new TypeError("bad"),
       big: 9007199254740993n, negZero: -0, nan: NaN, sparse: [0, , 2],
     };
     message.self = message;
     channel.port2.onmessage = event => {
       const data = event.data;
       trail.received.push({
         text: data.text, date: data.when.getTime(), set: data.set.size,
         map: data.map.get("k"), cyclic: data.self === data,
         sharedBuffer: data.viewA.buffer === data.viewB.buffer,
         offsets: [data.viewA.byteOffset, data.viewB.byteOffset],
         re: data.re.source + data.re.flags,
         error: data.err instanceof TypeError && data.err.message,
         big: String(data.big), negZero: Object.is(data.negZero, -0),
         nan: Number.isNaN(data.nan), hole: (1 in data.sparse), length: data.sparse.length,
         ports: event.ports.length,
       });
     };
     channel.port1.postMessage(message);

     // A clone is the same operation without the port, so the same value is
     // asserted about both ways round.
     const copy = structuredClone(message);
     trail.cloned = copy !== message && copy.self === copy
       && copy.when.getTime() === 1000 && copy.viewA.buffer === copy.viewB.buffer;

     // Transfer: the receiving side gets the bytes, and this side is left
     // holding a detached buffer rather than a second copy of them.
     const moved = new MessageChannel();
     const buffer = new Uint8Array([7, 8, 9]).buffer;
     moved.port2.onmessage = event =>
       trail.moved = { bytes: [...new Uint8Array(event.data)], here: buffer.byteLength };
     moved.port1.postMessage(buffer, [buffer]);

     // A port handed through a port: the third channel's end arrives on the
     // second and works from there.
     const passed = new MessageChannel();
     const carrier = new MessageChannel();
     carrier.port2.onmessage = event => {
       trail.carried = event.ports.length;
       event.ports[0].onmessage = inner => trail.received.push({ viaPort: inner.data });
     };
     carrier.port1.postMessage("take it", [passed.port2]);

     for (const [what, value] of [["function", () => 1], ["node", document.body]]) {
       try { channel.port1.postMessage({ value }); }
       catch (error) { trail.errors.push([what, error.name]); }
     }
     // A port that was transferred is no longer this context's to use.
     try { passed.port2.postMessage("after the handover"); trail.afterHandover = "sent"; }
     catch (error) { trail.afterHandover = error.name; }

     globalThis.__blitsenMessagingTick = () => {
       globalThis.__blitsenAnimationFrameTick(performance.now());
       // The carried port only starts once the message carrying it is delivered,
       // so the send onto it belongs to a later turn — the same shape as a
       // browser, where the receiving side sets onmessage before anything can
       // arrive on it.
       passed.port1.postMessage("through the carried port");
     };
     document.getElementById("messaging").setAttribute("data-messaging", "ok"); }`,
  200,
  100,
));
assert.equal(
  probe.nodes.find(node => node.attributes.id === "messaging").attributes["data-messaging"],
  "ok",
);

const trail = globalThis.__blitsenMessaging;
assert.equal(trail.cloned, true, "structuredClone rebuilds the graph, cycles and shared buffers");
assert.deepEqual(trail.errors, [["function", "DataCloneError"], ["node", "DataCloneError"]],
  "a function and a DOM node are refused rather than flattened");
assert.equal(trail.afterHandover, "sent",
  "posting on a transferred port is discarded, not thrown at the caller");

// Nothing is delivered until the frame turns: that is the whole contract.
assert.equal(trail.received.length, 0, "a message is not delivered before the frame it lands in");
globalThis.__blitsenMessagingTick();

const [first] = trail.received;
assert.ok(first, "the message was not delivered on the animation-frame tick");
assert.equal(first.text, "hello");
assert.equal(first.date, 1000);
assert.equal(first.set, 3);
assert.equal(first.map, "v");
assert.equal(first.cyclic, true, "a cycle survives the round trip");
assert.equal(first.sharedBuffer, true, "two views over one buffer stay two views over one buffer");
assert.deepEqual(first.offsets, [0, 4]);
assert.equal(first.re, "ab+cgi");
assert.equal(first.error, "bad");
assert.equal(first.big, "9007199254740993", "a BigInt past the double's integers arrives exact");
assert.equal(first.negZero, true, "-0 is not 0");
assert.equal(first.nan, true);
assert.equal(first.hole, false, "a hole is not an undefined element");
assert.equal(first.length, 3);
assert.equal(first.ports, 0);
assert.deepEqual(trail.moved, { bytes: [7, 8, 9], here: 0 },
  "a transferred buffer arrives whole and leaves a detached one behind");
assert.equal(trail.carried, 1, "a port sent through a port arrives as event.ports");

globalThis.__blitsenMessagingTick();
assert.deepEqual(trail.received.at(-1), { viaPort: "through the carried port" },
  "the carried port delivers on the far end");

delete globalThis.__blitsenMessaging;
delete globalThis.__blitsenMessagingTick;

// A worker needs an application to load its script out of, and the bare bridge
// harness is not one. Refused at the constructor, naming what is missing.
const refusal = JSON.parse(native.runBridgeHarness(
  `<div id="worker"></div>`,
  `{ let refused = "constructed";
     try { new Worker("./work.js"); } catch (error) { refused = error.message; }
     document.getElementById("worker").setAttribute("data-refused", refused); }`,
  200,
  100,
));
assert.match(
  refusal.nodes.find(node => node.attributes.id === "worker").attributes["data-refused"],
  /no application/,
  "a worker with no application behind it is refused rather than started",
);
