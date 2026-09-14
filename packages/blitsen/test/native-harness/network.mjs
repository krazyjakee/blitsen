import { strict as assert } from "node:assert";
import { native } from "./addon.mjs";

const server = Bun.serve({ port: 0, hostname: "127.0.0.1", async fetch(request) {
  if (new URL(request.url).pathname === "/stream") return new Response(new ReadableStream({
    async start(controller) {
      controller.enqueue(new TextEncoder().encode("first"));
      await Bun.sleep(20);
      controller.enqueue(new TextEncoder().encode("second"));
      controller.close();
    },
  }));
  if (new URL(request.url).pathname === "/wait") await Bun.sleep(500);
  return Response.json({ method: request.method, body: await request.text() });
}});
const origin = `http://127.0.0.1:${server.port}`;
const settle = async predicate => {
  const deadline = performance.now() + 3000;
  while (!predicate() && performance.now() < deadline) {
    globalThis.__blitsenAnimationFrameTick(performance.now());
    await Bun.sleep(5);
  }
  assert(predicate(), "Bun network completion reached the document");
};
try {
  native.runBridgeHarness("<p></p>", `
    globalThis.networkResult = {};
    fetch(${JSON.stringify(origin)}, { method: "POST", body: "payload" })
      .then(response => response.json()).then(value => networkResult.echo = value);
    fetch(${JSON.stringify(origin + "/stream")}).then(async response => {
      networkResult.streaming = response.body instanceof ReadableStream;
      networkResult.body = await response.text();
    });
    const aborter = new AbortController();
    fetch(${JSON.stringify(origin + "/wait")}, { signal: aborter.signal })
      .catch(error => networkResult.abort = error.name);
    aborter.abort();
  `);
  await Bun.sleep(80);
  assert.equal(globalThis.networkResult.echo, undefined, "fetch handoff waits for a native frame");
  await settle(() => networkResult.echo && networkResult.body && networkResult.abort);
  assert.deepEqual(networkResult.echo, { method: "POST", body: "payload" });
  assert.equal(networkResult.streaming, true);
  assert.equal(networkResult.body, "firstsecond");
  assert.equal(networkResult.abort, "AbortError");

  native.runBridgeHarness("<p></p>", `fetch(${JSON.stringify(origin + "/wait")})
    .then(() => networkResult.late = true, error => networkResult.closed = error.name);`);
  globalThis.__blitsenDisposeContext();
  await Bun.sleep(20);
  assert.equal(networkResult.late, undefined);
  assert.equal(networkResult.closed, "AbortError", "document disposal cancels owned fetches");
} finally { await server.stop(true); }
