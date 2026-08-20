// `EventSource` (issue #236), against a real server on a real socket.
//
// The server is `node:http` rather than `Bun.serve` for the reason the fetch
// probe uses it: `runBridgeHarness` installs the bridge into *this* context, so
// `Response` and the rest are Blitsen's by the time a handler would run.
//
// Two connections are scripted, because the half of SSE that is not a parser is
// the reconnection: the first ends mid-feed with no close of any kind, which is
// what a proxy timing out looks like, and the second has to arrive carrying the
// id the first one reached.
import { strict as assert } from "node:assert";
import { createServer } from "node:http";

import { native } from "./addon.mjs";

/** What each request asked for, so the resume can be asserted from outside. */
const requests = [];

const server = createServer((request, response) => {
  requests.push({ url: request.url, headers: request.headers });
  if (request.url === "/not-a-stream") {
    response.writeHead(200, { "content-type": "application/json" });
    response.end("{}");
    return;
  }
  response.writeHead(200, {
    "content-type": "text/event-stream; charset=utf-8",
    "cache-control": "no-cache",
    connection: "keep-alive",
  });
  if (request.headers["last-event-id"] === "7") {
    response.write("data: resumed\n\n");
    return;
  }
  // `retry` is lowered on the first connection so the reconnection this test is
  // about happens inside the turns it allows, rather than three seconds later.
  response.write("retry: 50\n\n");
  response.write(": a comment keeps the connection warm\n\n");
  response.write("data: first\n\n");
  response.write("event: quote\ndata: line one\ndata: line two\nid: 7\n\n");
  // Ends the body without ending the stream, which is the case an application
  // relies on reconnection for.
  setTimeout(() => response.end(), 50);
});
await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
const origin = `http://127.0.0.1:${server.address().port}`;

try {
  const opened = JSON.parse(native.runBridgeHarness(
    `<div id="feed"></div>`,
    `{ const results = globalThis.__blitsenFeed = { events: [], states: [] };
       if (!("EventSource" in globalThis)) throw new Error("EventSource must be installed");
       if (EventSource.CONNECTING !== 0 || EventSource.OPEN !== 1 || EventSource.CLOSED !== 2)
         throw new Error("readyState constants");

       for (const [address, expected] of [["ws://example.com/", "SyntaxError"],
         ["not a url", "SyntaxError"]]) {
         let refused = null;
         try { new EventSource(address); } catch (error) { refused = error.name; }
         if (refused !== expected) throw new Error("a non-http address must be refused: " + address);
       }

       const feed = globalThis.__blitsenFeedHandle = new EventSource("${origin}/stream");
       if (feed.readyState !== EventSource.CONNECTING) throw new Error("a new stream is CONNECTING");
       if (feed.url !== "${origin}/stream") throw new Error("url reads back what it was given");
       if (feed.withCredentials !== false) throw new Error("withCredentials defaults to false");

       feed.addEventListener("open", () => results.states.push(["open", feed.readyState]));
       // A named event is not delivered to onmessage, which is the whole point
       // of naming it.
       feed.addEventListener("quote", event => {
         results.events.push(["quote", event.data, event.lastEventId, event.origin]);
       });
       feed.onmessage = event => {
         results.events.push(["message", event.data, event.lastEventId]);
         if (event.data === "resumed") {
           feed.close();
           results.states.push(["closed", feed.readyState]);
           results.done = true;
         }
       };
       feed.addEventListener("error", () => results.states.push(["error", feed.readyState]));
       document.getElementById("feed").setAttribute("data-feed", "ok"); }`,
    320, 180));
  assert.equal(
    opened.nodes.find(node => node.attributes.id === "feed").attributes["data-feed"], "ok");

  // The same landing rule the socket and network results follow: an event that
  // arrived off the wire waits for the frame turn rather than interrupting
  // between turns.
  await Bun.sleep(250);
  assert.equal(globalThis.__blitsenFeed.events.length, 0,
    "stream events wait for the frame turn rather than arriving between them");
  assert.equal(globalThis.__blitsenAnimationFramesPending(), true,
    "a live stream keeps the host turning so its events can land");

  for (let turn = 0; turn < 400 && !globalThis.__blitsenFeed.done; turn++) {
    globalThis.__blitsenAnimationFrameTick(0);
    await Bun.sleep(5);
  }
  const { events, states, done } = globalThis.__blitsenFeed;
  assert.equal(done, true, "the stream resumed and closed within the turns allowed");

  assert.deepEqual(events[0], ["message", "first", ""],
    "an event with no id carries an empty lastEventId");
  assert.deepEqual(events[1], ["quote", "line one\nline two", "7", origin],
    "a named event is delivered under its own type, with its data lines joined");
  assert.deepEqual(events[2], ["message", "resumed", "7"],
    "the resumed connection's events carry the id the feed had reached");

  assert.deepEqual(states.filter(([kind]) => kind === "open"), [["open", 1], ["open", 1]],
    "both connections announce themselves as OPEN");
  assert.deepEqual(states.filter(([kind]) => kind === "error"), [["error", 0]],
    "a dropped connection is an error that leaves the stream CONNECTING, not CLOSED");
  assert.deepEqual(states.at(-1), ["closed", 2], "close() settles the stream at CLOSED");

  assert.equal(requests.length, 2, "the stream reconnected exactly once");
  assert.equal(requests[0].headers.accept, "text/event-stream");
  assert.equal(requests[0].headers["last-event-id"], undefined,
    "nothing has been received yet, so there is no id to resume from");
  assert.equal(requests[1].headers["last-event-id"], "7",
    "the reconnection resumes from the last id the server sent");

  // Long enough for the 50ms retry to have fired twice over.
  await Bun.sleep(200);
  for (let turn = 0; turn < 10; turn++) globalThis.__blitsenAnimationFrameTick(0);
  assert.equal(requests.length, 2, "a closed stream does not reconnect");
  assert.equal(globalThis.__blitsenAnimationFramesPending(), false,
    "a closed stream stops asking for frames");

  // A response that is not a stream fails for good. Checked in its own session
  // so the first one can assert the reconnecting path without branching.
  JSON.parse(native.runBridgeHarness(
    `<div id="broken"></div>`,
    `{ const results = globalThis.__blitsenBrokenFeed = { states: [] };
       const feed = new EventSource("${origin}/not-a-stream");
       feed.onerror = () => { results.states.push(feed.readyState); results.done = true; };
       feed.onopen = () => results.states.push("open"); }`,
    320, 180));
  for (let turn = 0; turn < 400 && !globalThis.__blitsenBrokenFeed.done; turn++) {
    globalThis.__blitsenAnimationFrameTick(0);
    await Bun.sleep(5);
  }
  assert.deepEqual(globalThis.__blitsenBrokenFeed.states, [2],
    "a response that is not text/event-stream errors and settles at CLOSED");
  const asked = requests.filter(entry => entry.url === "/not-a-stream").length;
  await Bun.sleep(200);
  for (let turn = 0; turn < 10; turn++) globalThis.__blitsenAnimationFrameTick(0);
  assert.equal(requests.filter(entry => entry.url === "/not-a-stream").length, asked,
    "a stream that failed for good is not retried");

  delete globalThis.__blitsenFeed;
  delete globalThis.__blitsenFeedHandle;
  delete globalThis.__blitsenBrokenFeed;
} finally {
  server.close();
}
