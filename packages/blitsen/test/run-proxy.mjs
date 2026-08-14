// Proxy mode (issue #67): the application is served by the user's own dev
// server, and the native window replaces the browser tab.
//
//     bun run --cwd packages/blitsen test:proxy
//
// The dev server here is `node:http` rather than Vite: what is under test is
// Blitsen's half of the arrangement — reading a document and its module graph
// over HTTP, keeping a hot-reload channel open, surviving a restart, and saying
// something useful when nothing is serving yet. Standing up a real Vite is a
// slower way to exercise the same four things, and it would test Vite.
import { strict as assert } from "node:assert";
import { join } from "node:path";

import { buildAddon, buildRuntime, repository } from "./build-addon.mjs";
import { resolvePhase2Runtime } from "../src/runtime.mjs";

const CLI = join(repository, "packages/blitsen/bin/blitsen.mjs");

// Served, not written to disk: proxy mode never touches a directory, and a
// fixture on disk would let a filesystem read pass for a served one.
const DOCUMENT = `<!doctype html><html>
<head><link rel="stylesheet" href="/src/style.css"></head>
<body><main id="out">waiting</main>
<script type="module" src="/src/main.js"></script>
</body></html>`;

// A module graph with a query string in it, because a dev server answers
// `?v=1` and `?v=2` differently and Blitsen has to ask as written.
const ENTRY_MODULE = `import { greeting } from "./greeting.js?v=1";
globalThis.__proxy = { module: import.meta.url, greeting, hmr: "connecting" };
document.getElementById("out").textContent = greeting;
fetch(new URL("./served.json", import.meta.url).href)
  .then(response => response.json())
  .then(value => { globalThis.__proxy.read = value.served; },
    error => { globalThis.__proxy.read = "failed: " + error.message; });
// The hot-reload channel a dev server keeps open. Vite's client opens its own
// and is told where by constants the server substitutes as it transforms —
// __HMR_PORT__ — which is why this is written the same way: the document's own
// origin is Blitsen's, and the socket has to reach the server instead.
const socket = new WebSocket("ws://127.0.0.1:__HMR_PORT__/hmr");
socket.addEventListener("open", () => socket.send("hello"));
socket.addEventListener("message", event => {
  globalThis.__proxy.hmr = event.data;
});
socket.addEventListener("error", () => { globalThis.__proxy.hmr = "errored"; });
socket.addEventListener("close", event => {
  if (globalThis.__proxy.hmr === "connecting") globalThis.__proxy.hmr = "closed:" + event.code;
});
globalThis.__proxySocket = socket;
`;

const FILES = {
  "/index.html": ["text/html", DOCUMENT],
  "/src/main.js": ["text/javascript", ENTRY_MODULE],
  "/src/greeting.js": ["text/javascript", `export const greeting = "served over http";`],
  "/src/served.json": ["application/json", `{"served":"yes"}`],
  "/src/style.css": ["text/css", "#out { color: rgb(10, 20, 30); }"],
};

/** A dev server: static files, and a hot-reload socket that answers. */
function devServer(port = 0) {
  const server = Bun.serve({
    port,
    hostname: "127.0.0.1",
    fetch(request, self) {
      const path = new URL(request.url).pathname;
      // The channel a dev server keeps open to the document it served. Vite's
      // is `/@vite/client` talking back to `/`; the shape is what matters here.
      if (path === "/hmr") {
        return self.upgrade(request) ? undefined : new Response("no upgrade", { status: 400 });
      }
      const file = FILES[path];
      if (!file) return new Response("not found", { status: 404 });
      // Substituted as it is served, exactly as a dev server substitutes its
      // own HMR constants while transforming a module.
      return new Response(file[1].replaceAll("__HMR_PORT__", String(self.port)),
        { headers: { "content-type": file[0] } });
    },
    websocket: {
      message(socket) {
        socket.send("reload");
      },
    },
  });
  return {
    port: () => server.port,
    close: async () => { await server.stop(true); },
  };
}

buildRuntime();
const runtime = await resolvePhase2Runtime();
// The addon stands in for an installed platform package, exactly as every other
// acceptance script drives the CLI.
const addon = await buildAddon({ purpose: "proxy mode", release: true });

// Spawned asynchronously, never `spawnSync`: the dev server being driven is in
// *this* process, and a synchronous wait would stop it answering the requests
// the run is about to make.
async function spawn(command, { cwd = repository, environment = {} } = {}) {
  const child = Bun.spawn({
    cmd: command,
    cwd,
    env: { ...process.env, ...environment },
    stdout: "pipe",
    stderr: "pipe",
  });
  const [stdout, stderr, code] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ]);
  return { code, stdout, stderr };
}

function runtimeAgainst(url, { assert: assertion, environment = {} }) {
  return spawn([runtime.path, url], {
    environment: {
      BLITSEN_STANDALONE_CHECK: "1",
      BLITSEN_STANDALONE_CHECK_DELAY: "400",
      ...(assertion ? { BLITSEN_STANDALONE_CHECK_ASSERT: assertion } : {}),
      ...environment,
    },
  });
}

function cli(args) {
  return spawn([process.execPath, CLI, ...args], { environment: { BLITSEN_NATIVE_PATH: addon } });
}

const dev = devServer();
// Held rather than asked for again later: a stopped server no longer reports
// the port it was on, and the restart below has to come back on the same one.
const port = dev.port();
const origin = `http://127.0.0.1:${port}`;
try {
  // ① The document, its module graph, its stylesheet and a file it reads — all
  // over HTTP, with nothing on disk.
  const report = await runtimeAgainst(origin, {
    assert: `console.log("proxy " + JSON.stringify({
      ...globalThis.__proxy,
      text: document.getElementById("out").textContent,
      color: getComputedStyle(document.getElementById("out")).color,
      readyState: globalThis.__proxySocket.readyState,
    }))`,
  });
  assert.equal(report.code, 0, `the runtime refused a served application:\n${report.stderr}`);
  const line = report.stdout.split("\n").find(text => text.startsWith("proxy "));
  assert.ok(line, `no probe in the output:\n${report.stdout}\n${report.stderr}`);
  const probe = JSON.parse(line.slice("proxy ".length));
  assert.equal(probe.greeting, "served over http",
    "the module graph did not come from the server");
  assert.equal(probe.text, "served over http", "the document did not render what it imported");
  assert.equal(probe.color, "rgb(10, 20, 30)", "the served stylesheet was not applied");
  assert.equal(probe.read, "yes", "fetch did not read a file the server serves");
  assert.match(probe.module, /^blitsen:\/\/app\/src\/main\.js$/,
    `a served module is addressed as an application: ${probe.module}`);

  // ② The hot-reload channel. The document opened a socket back to the server
  // and the server answered on it — which is the whole of what Blitsen owes
  // HMR: the channel stays open and its messages land in the frame turn.
  assert.equal(probe.hmr, "reload",
    `the hot-reload channel did not deliver: ${probe.hmr}`);

  // ③ A dev server that goes away and comes back. Restarting on the same port
  // is what `vite` does on a config change, and what a stopped `npm run dev`
  // looks like from here.
  await dev.close();
  const refused = await runtimeAgainst(origin, { environment: { BLITSEN_DEV_SERVER_GRACE_MS: "300" } });
  assert.notEqual(refused.code, 0, "a URL with nothing behind it started a session anyway");
  assert.match(refused.stderr, /nothing is answering at/,
    `the refusal should name the server: ${refused.stderr}`);
  assert.match(refused.stderr, /npm run dev/,
    `the refusal should say what to do about it: ${refused.stderr}`);

  const again = devServer(port);
  try {
    // The grace period is what makes a restart survivable: this run starts
    // while nothing is listening and connects when the server comes back.
    const reconnected = await runtimeAgainst(origin, {
      assert: `console.log("again " + document.getElementById("out").textContent)`,
      environment: { BLITSEN_DEV_SERVER_GRACE_MS: "5000" },
    });
    assert.equal(reconnected.code, 0,
      `the runtime did not come back with the server:\n${reconnected.stderr}`);
    assert.match(reconnected.stdout, /again served over http/,
      "the restarted server's application did not run");
  } finally {
    await again.close();
  }

  // ④ The other two commands read files, and a dev server has none to read.
  const built = await cli(["build", origin]);
  assert.notEqual(built.code, 0, "build accepted a URL");
  assert.match(built.stderr, /needs a directory of built output, not a URL/,
    `build should say why: ${built.stderr}`);
  const doctored = await cli(["doctor", origin]);
  assert.notEqual(doctored.code, 0, "doctor accepted a URL");
  assert.match(doctored.stderr, /needs a directory of built output, not a URL/,
    `doctor should say why: ${doctored.stderr}`);

  console.log("Proxy mode passed: document, module graph (with a query), stylesheet and a "
    + "fetched file all read over HTTP.");
  console.log("  Hot-reload channel delivered, a refused URL is reported, and a restarted "
    + "server is waited for and connected to.");
  console.log(`  Runtime: ${runtime.path}`);
} finally {
  await dev.close().catch(() => {});
}
