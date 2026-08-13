// `WebSocket` (issue #80), against a real server on a real socket.
//
// The server is hand-rolled on `node:net` rather than taken from `Bun.serve`
// for the reason the fetch probe uses `node:http`: `runBridgeHarness` installs
// the bridge into *this* context, so `Response`, `Blob` and the rest are
// Blitsen's by the time a server handler would run. A raw socket has no such
// surface to be handed. RFC 6455 is small enough at this scale — a handshake,
// and frames a browser client always masks and a server never does.
import { strict as assert } from "node:assert";
import { createHash } from "node:crypto";
import { createServer } from "node:net";

import { native } from "./addon.mjs";

const GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const accept = key => createHash("sha1").update(key + GUID).digest("base64");

/** Frames a payload the way a server does: never masked. */
const frame = (opcode, payload) => {
  const body = Buffer.isBuffer(payload) ? payload : Buffer.from(payload, "utf8");
  const header = body.length < 126 ? Buffer.from([0x80 | opcode, body.length])
    : body.length < 65536
      ? Buffer.concat([Buffer.from([0x80 | opcode, 126]), (() => {
        const length = Buffer.alloc(2); length.writeUInt16BE(body.length); return length;
      })()])
      : Buffer.concat([Buffer.from([0x80 | opcode, 127]), (() => {
        const length = Buffer.alloc(8); length.writeBigUInt64BE(BigInt(body.length)); return length;
      })()]);
  return Buffer.concat([header, body]);
};

/**
 * Pulls one complete frame off the front of `buffer`.
 *
 * Returns null when the frame is not all there yet, which on a stream socket is
 * an ordinary state rather than an error.
 */
const readFrame = buffer => {
  if (buffer.length < 2) return null;
  const opcode = buffer[0] & 0x0f;
  const masked = (buffer[1] & 0x80) !== 0;
  let length = buffer[1] & 0x7f;
  let offset = 2;
  if (length === 126) {
    if (buffer.length < 4) return null;
    length = buffer.readUInt16BE(2);
    offset = 4;
  } else if (length === 127) {
    if (buffer.length < 10) return null;
    length = Number(buffer.readBigUInt64BE(2));
    offset = 10;
  }
  const mask = masked ? buffer.subarray(offset, offset + 4) : null;
  if (masked) offset += 4;
  if (buffer.length < offset + length) return null;
  const payload = Buffer.from(buffer.subarray(offset, offset + length));
  // A client must mask every frame it sends, so unmasking is not optional.
  if (mask) for (let index = 0; index < payload.length; index += 1) payload[index] ^= mask[index % 4];
  return { opcode, payload, rest: buffer.subarray(offset + length) };
};

// What the server does with what it is sent, so the client's half can be
// asserted from the outside as well as from inside the runtime.
const received = [];
let negotiated = null;

const server = createServer(connection => {
  let buffer = Buffer.alloc(0);
  let open = false;
  connection.on("data", chunk => {
    buffer = Buffer.concat([buffer, chunk]);
    if (!open) {
      const end = buffer.indexOf("\r\n\r\n");
      if (end < 0) return;
      const request = buffer.subarray(0, end).toString();
      buffer = buffer.subarray(end + 4);
      const key = /sec-websocket-key:\s*(\S+)/i.exec(request)?.[1];
      // The client asks for two and the server picks the second, so the test
      // can tell a negotiated protocol from an echoed one.
      const offered = /sec-websocket-protocol:\s*(.+)/i.exec(request)?.[1]
        ?.split(",").map(value => value.trim()) ?? [];
      negotiated = offered[1] ?? offered[0] ?? null;
      connection.write([
        "HTTP/1.1 101 Switching Protocols",
        "Upgrade: websocket",
        "Connection: Upgrade",
        `Sec-WebSocket-Accept: ${accept(key)}`,
        ...(negotiated ? [`Sec-WebSocket-Protocol: ${negotiated}`] : []),
        "", "",
      ].join("\r\n"));
      open = true;
      // Both frame kinds, unprompted, so the client's `binaryType` handling is
      // exercised on data it did not ask for at a moment it chose.
      connection.write(frame(0x1, "hello"));
      connection.write(frame(0x2, Buffer.from([1, 2, 3, 4])));
    }
    for (let parsed = readFrame(buffer); parsed; parsed = readFrame(buffer)) {
      buffer = parsed.rest;
      if (parsed.opcode === 0x8) {
        const code = parsed.payload.length >= 2 ? parsed.payload.readUInt16BE(0) : null;
        received.push(["close", code, parsed.payload.subarray(2).toString()]);
        connection.end(frame(0x8, parsed.payload));
      } else if (parsed.opcode === 0x1) {
        received.push(["text", parsed.payload.toString()]);
        // The echo is what proves a text frame made the round trip.
        connection.write(frame(0x1, `echo:${parsed.payload.toString()}`));
      } else if (parsed.opcode === 0x2) {
        received.push(["binary", [...parsed.payload]]);
      }
    }
  });
  connection.on("error", () => {});
});
await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
const address = `ws://127.0.0.1:${server.address().port}/socket`;

try {
  const opened = JSON.parse(native.runBridgeHarness(
    `<div id="socket"></div>`,
    `{ const results = globalThis.__blitsenSocket = { events: [], errors: [] };
       if (!("WebSocket" in globalThis)) throw new Error("WebSocket must be installed");
       // The readyState constants live on the constructor and on instances.
       if (WebSocket.CONNECTING !== 0 || WebSocket.OPEN !== 1 || WebSocket.CLOSING !== 2
           || WebSocket.CLOSED !== 3) throw new Error("readyState constants");

       for (const [address, expected] of [["http://example.com/", "SyntaxError"],
         ["not a url", "SyntaxError"]]) {
         let refused = null;
         try { new WebSocket(address); } catch (error) { refused = error.name; }
         if (refused !== expected) throw new Error("a non-ws address must be refused: " + address);
       }

       const socket = globalThis.__blitsenSocketHandle =
         new WebSocket("${address}", ["chat.v1", "chat.v2"]);
       if (socket.readyState !== WebSocket.CONNECTING) throw new Error("a new socket is CONNECTING");
       if (socket.url !== "${address}") throw new Error("url reads back what it was given");
       if (socket.bufferedAmount !== 0) throw new Error("nothing is buffered yet");
       if (socket.binaryType !== "blob") throw new Error("binaryType defaults to blob");
       let refusedType = null;
       try { socket.binaryType = "text"; } catch (error) { refusedType = error.constructor.name; }
       if (refusedType !== "TypeError") throw new Error("an unknown binaryType is refused");
       socket.binaryType = "arraybuffer";

       // Sending before the socket is open is the one thing that throws.
       let early = null;
       try { socket.send("too soon"); } catch (error) { early = error.name; }
       if (early !== "InvalidStateError") throw new Error("send before open must throw");

       socket.addEventListener("open", () => {
         results.events.push(["open", socket.readyState, socket.protocol]);
         socket.send("ping");
         socket.send(new Uint8Array([9, 8, 7]).buffer);
       });
       socket.addEventListener("message", event => {
         const data = event.data;
         if (typeof data === "string") results.events.push(["text", data]);
         else if (data instanceof ArrayBuffer) results.events.push(["binary", [...new Uint8Array(data)]]);
         else results.events.push(["other", String(data)]);
         if (results.events.filter(entry => entry[0] === "text").length === 2)
           socket.close(4000, "done here");
       });
       socket.addEventListener("close", event => {
         results.events.push(["close", event.code, event.reason, event.wasClean, socket.readyState]);
         results.done = true;
       });
       socket.addEventListener("error", () => results.errors.push("error"));
       document.getElementById("socket").setAttribute("data-socket", "ok"); }`,
    320, 180));
  assert.equal(
    opened.nodes.find(node => node.attributes.id === "socket").attributes["data-socket"], "ok");

  // The same landing rule the network results follow: a frame that arrived off
  // the socket waits for the frame turn rather than interrupting between turns.
  await Bun.sleep(250);
  assert.equal(globalThis.__blitsenSocket.events.length, 0,
    "socket events wait for the frame turn rather than arriving between them");
  assert.equal(globalThis.__blitsenAnimationFramesPending(), true,
    "a live socket keeps the host turning so its frames can land");

  for (let turn = 0; turn < 400 && !globalThis.__blitsenSocket.done; turn++) {
    globalThis.__blitsenAnimationFrameTick(0);
    await Bun.sleep(5);
  }
  const { events, errors, done } = globalThis.__blitsenSocket;
  assert.equal(done, true, "the socket closed within the turns allowed");
  assert.deepEqual(errors, [], "a clean session reports no error");

  const byKind = kind => events.filter(entry => entry[0] === kind);
  assert.deepEqual(byKind("open")[0], ["open", 1, "chat.v2"],
    "open reports OPEN and the protocol the server chose, not the first offered");
  assert.deepEqual(byKind("text").map(entry => entry[1]), ["hello", "echo:ping"],
    "text frames arrive as strings, in order");
  assert.deepEqual(byKind("binary")[0], ["binary", [1, 2, 3, 4]],
    "a binary frame arrives as an ArrayBuffer under binaryType arraybuffer");
  assert.deepEqual(byKind("close")[0], ["close", 4000, "done here", true, 3],
    "close carries its code and reason, is clean, and leaves the socket CLOSED");

  // The other half: what the server actually received off the wire.
  assert.deepEqual(received.find(entry => entry[0] === "text"), ["text", "ping"]);
  assert.deepEqual(received.find(entry => entry[0] === "binary"), ["binary", [9, 8, 7]]);
  assert.deepEqual(received.find(entry => entry[0] === "close"), ["close", 4000, "done here"]);
  assert.equal(negotiated, "chat.v2");
  assert.equal(globalThis.__blitsenAnimationFramesPending(), false,
    "a closed socket stops asking for frames");

  // Blob is the default, and is what an application that never sets
  // `binaryType` gets. Checked in its own session so the first one can assert
  // the arraybuffer path without branching.
  received.length = 0;
  JSON.parse(native.runBridgeHarness(
    `<div id="blob"></div>`,
    `{ const results = globalThis.__blitsenBlobSocket = { seen: [] };
       const socket = new WebSocket("${address}");
       socket.addEventListener("message", event => {
         if (event.data instanceof Blob) {
           results.seen.push(["blob", event.data.size, event.data.type]);
           socket.close();
         }
       });
       socket.addEventListener("close", () => { results.done = true; }); }`,
    320, 180));
  for (let turn = 0; turn < 400 && !globalThis.__blitsenBlobSocket.done; turn++) {
    globalThis.__blitsenAnimationFrameTick(0);
    await Bun.sleep(5);
  }
  assert.deepEqual(globalThis.__blitsenBlobSocket.seen[0], ["blob", 4, ""],
    "the default binaryType delivers a Blob of the frame's bytes");

  delete globalThis.__blitsenSocket;
  delete globalThis.__blitsenSocketHandle;
  delete globalThis.__blitsenBlobSocket;
} finally {
  server.close();
}
