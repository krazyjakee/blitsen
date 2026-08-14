import { strict as assert } from "node:assert";
import { createServer } from "node:http";

import { native } from "./addon.mjs";

// Blitsen's own fetch, not the Phase 1 host's. `runBridgeHarness` installs the
// bridge into this very context, so these are the classes an exported
// application sees — which is also why the probe server is `node:http`:
// `Bun.serve` would be handed the replaced `Response`.
const probe = createServer((request, response) => {
  let body = "";
  request.on("data", chunk => { body += chunk; });
  request.on("end", () => {
    if (request.url === "/missing") {
      response.writeHead(404, { "content-type": "text/plain" });
      response.end("gone");
      return;
    }
    response.writeHead(200, { "content-type": "application/json", "x-probe": "kept" });
    response.end(JSON.stringify({
      method: request.method, sent: body, probe: request.headers["x-probe"] ?? null,
    }));
  });
});
await new Promise(resolve => probe.listen(0, "127.0.0.1", resolve));
const probeOrigin = `http://127.0.0.1:${probe.address().port}`;

try {
  const network = JSON.parse(native.runBridgeHarness(
    `<div id="network"></div>`,
    `{ const results = globalThis.__blitsenNetwork = { settled: [] };
       const headers = new Headers([["X-One", "1"]]);
       headers.append("x-one", "2");
       if (headers.get("X-ONE") !== "1, 2" || !headers.has("x-one") || [...headers].length !== 1)
         throw new Error("Headers case-folding or combination");
       headers.delete("X-One");
       if (headers.get("x-one") !== null || headers.has("x-one")) throw new Error("Headers delete");
       const request = new Request("/reports", { method: "post", headers: { "x-probe": "yes" }, body: "payload" });
       if (request.method !== "POST" || request.url !== "blitsen://app/reports" ||
           request.headers.get("content-type") !== "text/plain;charset=UTF-8" || request.bodyUsed)
         throw new Error("Request normalization: " + request.url);
       let bodylessGet;
       try { new Request("/x", { body: "no" }); } catch (error) { bodylessGet = error.constructor.name; }
       if (bodylessGet !== "TypeError") throw new Error("a GET request must refuse a body");

       const response = new Response("hi", { status: 202, statusText: "Accepted",
         headers: { "content-type": "text/plain" } });
       if (response.status !== 202 || !response.ok || response.statusText !== "Accepted" || response.bodyUsed)
         throw new Error("Response construction");
       // Streaming bodies are not in this tier, so the property is absent and
       // a feature test can branch on it rather than on a null.
       if ("body" in response || "clone" in response || "getSetCookie" in Headers.prototype)
         throw new Error("unimplemented body/cookie surface must be absent");

       const blob = new Blob(["chunk-", "one"], { type: "TEXT/plain" });
       if (blob.size !== 9 || blob.type !== "text/plain") throw new Error("Blob assembly");

       const controller = new AbortController();
       if (controller.signal.aborted || !(controller.signal instanceof AbortSignal))
         throw new Error("AbortController signal");

       Promise.all([
         response.text().then(text => ["response-text", text, response.bodyUsed]),
         response.text().then(() => "re-read", error => ["re-read", error.constructor.name]),
         blob.text().then(text => ["blob-text", text]),
         new Response(new Uint8Array([104, 105])).arrayBuffer().then(buffer => ["bytes", buffer.byteLength]),
         Response.json({ n: 7 }).json().then(value => ["json", value.n]),
         fetch("/local.json").then(() => "resolved", error => ["no-server", String(error.message).includes("is none of them")]),
         fetch("${probeOrigin}/missing").then(async result => ["missing", result.status, result.ok, await result.text()]),
         fetch("${probeOrigin}/echo", { method: "PUT", headers: { "x-probe": "yes" }, body: "payload" })
           .then(async result => ["echo", result.status, result.headers.get("x-probe"), result.url,
             result.redirected, await result.json()]),
         (() => {
           const aborter = new AbortController();
           const pending = fetch("${probeOrigin}/echo", { signal: aborter.signal });
           aborter.abort();
           return pending.then(() => "resolved", error => ["aborted", error.name, controller.signal.aborted]);
         })(),
         fetch("http://127.0.0.1:1/refused").then(() => "resolved", error => ["refused", error.constructor.name]),
       ]).then(settled => { results.settled = settled; results.done = true; });
       document.getElementById("network").setAttribute("data-network", "ok"); }`,
    200,
    100,
  ));
  assert.equal(network.nodes.find(node => node.attributes.id === "network").attributes["data-network"], "ok");

  // The frame turn is the landing point: nothing settles between turns, however
  // long the worker pool has been finished.
  await Bun.sleep(250);
  assert.equal(globalThis.__blitsenNetwork.done, undefined,
    "network results wait for the frame turn rather than arriving between them");
  assert.equal(globalThis.__blitsenAnimationFramesPending(), true,
    "an in-flight request keeps the host turning so its results can land");
  for (let turn = 0; turn < 200 && !globalThis.__blitsenNetwork.done; turn++) {
    globalThis.__blitsenAnimationFrameTick(0);
    await Bun.sleep(5);
  }
  assert.equal(globalThis.__blitsenAnimationFramesPending(), false,
    "a settled queue stops asking for frames");
  const settled = new Map(globalThis.__blitsenNetwork.settled.map(entry => [entry[0], entry]));
  assert.deepEqual(settled.get("response-text"), ["response-text", "hi", true]);
  assert.deepEqual(settled.get("re-read"), ["re-read", "TypeError"], "a body is readable once");
  assert.deepEqual(settled.get("blob-text"), ["blob-text", "chunk-one"]);
  assert.deepEqual(settled.get("bytes"), ["bytes", 2]);
  assert.deepEqual(settled.get("json"), ["json", 7]);
  // The bare harness has no application behind it, so nothing addresses one:
  // reading a shipped file is the window session's behaviour (issue #125), and
  // what is left here is the refusal that names what fetch does reach.
  assert.deepEqual(settled.get("no-server"), ["no-server", true],
    "with no application to read from, a document-relative URL says what fetch does reach");
  assert.deepEqual(settled.get("missing"), ["missing", 404, false, "gone"]);
  assert.deepEqual(settled.get("echo"),
    ["echo", 200, "kept", `${probeOrigin}/echo`, false, { method: "PUT", sent: "payload", probe: "yes" }]);
  assert.deepEqual(settled.get("aborted"), ["aborted", "AbortError", false],
    "AbortController rejects its own request and no other");
  assert.deepEqual(settled.get("refused"), ["refused", "TypeError"]);
  delete globalThis.__blitsenNetwork;

  // `URL` and `URLSearchParams`, which are Blitsen's rather than the host's on
  // both hosts (#125, #126). The one an application reaches for first is
  // `new URL("./blip.wav", import.meta.url)`: the identifier a module script
  // runs under is an application URL, and resolving an asset against it has to
  // land on the file the application shipped.
  const urls = JSON.parse(native.runBridgeHarness(
    `<div id="url"></div>`,
    `{ const results = globalThis.__blitsenUrl = {};
       const asset = new URL("./blip.wav", "blitsen://app/assets/index-a1b2c3.js");
       results.asset = asset.href;
       results.parts = [asset.protocol, asset.host, asset.pathname, asset.search, asset.hash];
       const absolute = new URL("https://example.com:8443/a/b?q=1&q=2#frag");
       results.absolute = [absolute.origin, absolute.port, absolute.pathname,
         absolute.search, absolute.hash, String(absolute), absolute.toJSON()];
       results.params = [absolute.searchParams.getAll("q"), absolute.searchParams.get("missing"),
         absolute.searchParams.size];
       absolute.searchParams.set("q", "3");
       absolute.searchParams.append("r", "a b&c");
       // A live searchParams writes through to the URL it came from, which is
       // the whole reason to hand one out rather than a copy.
       results.written = [absolute.search, absolute.href];
       absolute.pathname = "moved";
       absolute.hash = "top";
       absolute.port = "";
       results.mutated = [absolute.href, absolute.host, absolute.origin];
       const invalid = absolute.href;
       absolute.port = "not-a-port";
       results.refused = absolute.href === invalid;
       results.canParse = [URL.canParse("blitsen://app/x"), URL.canParse("nonsense"),
         URL.parse("nonsense"), typeof URL.parse("blitsen://app/x")];
       let threw;
       try { new URL("./relative-with-no-base"); } catch (error) { threw = error.constructor.name; }
       results.threw = threw;
       // Object URLs are absent, not stubbed: there is no origin to hang one on.
       results.objectUrls = ["createObjectURL" in URL, "revokeObjectURL" in URL];
       const query = new URLSearchParams([["b", "2"], ["a", "1"], ["a", "0"]]);
       query.sort();
       results.query = [query.toString(), [...query.keys()], new URLSearchParams("x=1&y=a+b%26c").get("y")];
       document.getElementById("url").setAttribute("data-url", "ok"); }`,
    200,
    100,
  ));
  assert.equal(urls.nodes.find(node => node.attributes.id === "url").attributes["data-url"], "ok");
  const url = globalThis.__blitsenUrl;
  assert.equal(url.asset, "blitsen://app/assets/blip.wav",
    "an asset resolves against the module that named it, on the application origin");
  assert.deepEqual(url.parts, ["blitsen:", "app", "/assets/blip.wav", "", ""]);
  assert.deepEqual(url.absolute, ["https://example.com:8443", "8443", "/a/b", "?q=1&q=2", "#frag",
    "https://example.com:8443/a/b?q=1&q=2#frag", "https://example.com:8443/a/b?q=1&q=2#frag"]);
  assert.deepEqual(url.params, [["1", "2"], null, 2]);
  assert.deepEqual(url.written, ["?q=3&r=a+b%26c",
    "https://example.com:8443/a/b?q=3&r=a+b%26c#frag"]);
  assert.deepEqual(url.mutated, ["https://example.com/moved?q=3&r=a+b%26c#top",
    "example.com", "https://example.com"]);
  assert.equal(url.refused, true, "a setter given something unparseable leaves the URL alone");
  assert.deepEqual(url.canParse, [true, false, null, "object"]);
  assert.equal(url.threw, "TypeError", "a relative URL with no base is a TypeError");
  assert.deepEqual(url.objectUrls, [false, false]);
  assert.deepEqual(url.query, ["a=1&a=0&b=2", ["a", "a", "b"], "a b&c"]);
  delete globalThis.__blitsenUrl;

  // `window.stop()`. Svelte's minified store reads the bare name for its truth
  // value alone, but what it names is a real abort of the document's load.
  const stopped = JSON.parse(native.runBridgeHarness(
    `<div id="stop"></div>`,
    `{ const results = globalThis.__blitsenStop = { settled: [], ticks: [] };
       if (typeof stop !== "function" || stop !== window.stop)
         throw new Error("stop must resolve on the bare name a bundle reads");
       // Nothing is in flight yet: the machinery runs and finds nothing.
       if (stop() !== undefined) throw new Error("stop returns nothing");

       setTimeout(() => results.ticks.push("timeout"), 0);
       requestAnimationFrame(() => results.ticks.push("frame"));
       Promise.all([
         (() => {
           const pending = fetch("${probeOrigin}/echo");
           stop();
           return pending.then(() => "resolved",
             error => ["stopped", error.name, error instanceof DOMException]);
         })(),
         // Started after the stop, so it is part of a new load rather than the
         // stopped one; the stop it makes on arrival has nothing left to abort.
         fetch("${probeOrigin}/echo").then(async response => {
           stop();
           return ["after", response.status, (await response.json()).method];
         }),
       ]).then(settled => { results.settled = settled; results.done = true; });
       document.getElementById("stop").setAttribute("data-stop", "ok"); }`,
    200,
    100,
  ));
  assert.equal(stopped.nodes.find(node => node.attributes.id === "stop").attributes["data-stop"], "ok");
  for (let turn = 0; turn < 200 && !globalThis.__blitsenStop.done; turn++) {
    globalThis.__blitsenAnimationFrameTick(0);
    await Bun.sleep(5);
  }
  const aborted = new Map(globalThis.__blitsenStop.settled.map(entry => [entry[0], entry]));
  assert.deepEqual(aborted.get("stopped"), ["stopped", "AbortError", true],
    "stop() rejects an in-flight request exactly as that request's own signal would");
  assert.deepEqual(aborted.get("after"), ["after", 200, "GET"],
    "a load started after a stop completes, and a stop with nothing in flight leaves it alone");
  assert.deepEqual(globalThis.__blitsenStop.ticks.sort(), ["frame", "timeout"],
    "stop() aborts loading; a browser does not cancel timers or animation frames and neither does this");
  delete globalThis.__blitsenStop;
} finally {
  probe.close();
}

