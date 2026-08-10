# S7 — HTTP document and module graph

This spike tests proxy-mode loading against a real Vite dev server using the
Phase 1 Bun host.

## Run

```sh
spikes/s7/run.sh
```

The script installs the locked Vite dependency, starts Vite on an ephemeral
loopback port, loads the fixture, prints a trace, and exits nonzero if any
required assertion fails.

## What passes

On Bun 1.3.14 with Vite 8.2.1:

- `fetch` follows a document redirect, and `HTMLRewriter` parses stylesheet,
  image, Vite client, and app-module references against the final document URL;
- stylesheet and SVG responses load with the expected content and MIME type;
- a loader prefetches and scans Vite's transformed ES module graph;
- static relative imports, root-relative imports, dynamic `import()`, Vite asset
  imports, and a redirected module all execute correctly;
- redirected modules resolve their own imports against the final response URL;
- repeated imports use Bun's module cache, and each JS graph URL is fetched only
  once; and
- Bun's `WebSocket` completes the real Vite `vite-hmr` handshake and receives
  the `connected` message.

The fixture result is `42`, its dynamic import returns
`dynamic-import-ok`, and the redirected module returns `7`.

## Host gaps found

A direct `import("http://…")` does not work in Bun; it is treated as a local
path. A runtime plugin can load the graph, but Bun 1.3.14 does not allow an
asynchronous `onResolve` callback. Correct redirects therefore require a
two-phase loader: asynchronously fetch and scan the graph first, then resolve
synchronously from a canonical-URL cache while `onLoad` serves the responses.

Bun also exposes remote plugin modules under a synthetic file identity. For
example:

```text
requested: http://127.0.0.1:36111/src/main.js
observed:  file:///http://127.0.0.1:36111/src/main.js
```

That breaks faithful `import.meta.url`, stack/source-map URL identity, and any
code that expects a browser HTTP module origin unless the loader rewrites or
maps it. Vite did not expose a valid external `main.js.map` response in this
test. Bun provides `fetch` and `WebSocket`, but not `EventSource`.

Finally, connecting the HMR transport is not the same as running Vite's HMR
client. `/@vite/client` is browser-facing and still depends on Blitsen's DOM,
location, error overlay, and hot-module lifecycle support.

## Decision

Do not ship proxy/dev-server mode in v0. Keep v0's directory watcher and full
JS-context reload. Revisit proxy mode as part of v1's `fetch`, WebSocket, URL,
and module-loader work, with these acceptance requirements:

- preserve canonical HTTP identity for `import.meta.url`, errors, and maps;
- support static and dynamic graphs without executing a second bundler;
- follow redirects before resolving child specifiers;
- execute the actual Vite HMR client against the Blitsen DOM bridge; and
- provide or deliberately omit-detect `EventSource`.

The transport and graph mechanics are feasible; the deferral is about browser-
faithful semantics and avoiding a Bun-specific loader that is replaced in
Phase 2.
