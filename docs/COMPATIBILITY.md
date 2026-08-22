# v1 compatibility profile

Blitsen v1 accepts built static applications that stay within the surface below. The profile is
deliberately narrower than “works in a browser”: it describes what the current runtime and Blitz
renderer can support consistently enough to make an adoption claim.

The tier this profile publishes is [PRODUCT.md §7](PRODUCT.md#7-scope-by-tier)'s v1 — the v0
architecture surface plus `fetch`, `WebSocket`, images, web fonts, audio playback and the
`blitsen/{app,window,dialog,clipboard}` modules. Three of its members are partial by design and
say so where they are documented: `dialog.*` is Linux/BSD only, `app.requestSingleInstanceLock`
is Unix-only, and `window.create` is absent. What is *not* v1 is stated as plainly: WebGL and
WebGPU are absent, accessibility is absent, and text controls provide basic editing and selection
but not IME or the advanced editing surface — see [What v1 is not](#what-v1-is-not).

## Window renderer by platform

Blitsen uses the GPU Vello renderer on Windows, Linux, Android and Apple Silicon macOS. Intel
macOS uses Vello's CPU rasterizer and presents its finished pixel buffer through a software
window backend. This is an automatic safety fallback: Vello/Metal compute work can wedge the
display GPU on Intel/Radeon Macs, reset WindowServer and terminate the whole desktop session
([#229](https://github.com/krazyjakee/blitsen/issues/229)). Adapter or device-loss recovery cannot
make that path safe because the system can stop responding before wgpu reports an error.

The selected renderer is written to stderr when a window opens. The CPU fallback needs no app
configuration and has no GPU override on Intel macOS; rendering there may use more CPU than on
the GPU-backed targets. It can be substantially slower at HiDPI resolutions, during resize, and
on pages that repaint frequently.

Run the check against build output, not source:

```sh
npx blitsen doctor dist
npx blitsen doctor dist --json
```

`doctor` exits non-zero for profile errors. A build repeats every diagnostic and **fails on any
error** — an export that cannot run is not worth producing — while warnings let feature-detected
fallback paths through. The scan is static and conservative: it finds references, not only
executed paths, which is why [severity](#diagnostic-severity) is narrow. Every rule it applies to
JavaScript comes from the [generated manifest](#capability-tiers) below.

## Strict v1 surface

| Area | In profile |
| --- | --- |
| Application shape | One built `index.html` plus the local files reachable from it; root-relative HTML/CSS asset URLs are normalized while ingesting without changing `dist` |
| JavaScript | ES modules already emitted by the application's bundler. **Source is refused**, not transpiled — see [Built output, not source](#built-output-not-source) |
| Framework DOM | Stable node identity, standard node type/name/value/owner fields, `MutationObserver`, creation/insertion/removal, text and attributes, elements, comments, namespaced elements, fragments and `<template>` |
| Selection and collections | `querySelector`, `querySelectorAll`, `getElementsByTagName` and `getElementsByClassName` on the document and on an element, `getElementById`, `closest`, `matches`, `children` and the element-traversal properties, `dataset`, `attributes`, static `NodeList`, `classList`, `link.relList` |
| Events | Capture/target/bubble listeners, click, mouse, wheel, keyboard, focus, resize and lifecycle events, plus `beforeinput`/`input` from typing into a control |
| Pointer input | `pointerdown`/`pointermove`/`pointerup`/`pointercancel` with `pointerType`, `pointerId`, `pressure` and `isPrimary`, for mouse, touch and pen; multi-touch with one pointer per contact; `setPointerCapture`/`releasePointerCapture`; the mouse events synthesised behind them — see [Pointer events](#pointer-events) |
| Style read-back | `getComputedStyle`, `matchMedia`/`MediaQueryList`, `ResizeObserver`, `CSS.escape`/`CSS.supports` |
| Geometry and text | `getBoundingClientRect`, `getClientRects`, the offset/client/scroll box properties, `clientTop`/`clientLeft`, `offsetParent`, `innerText`, `compareDocumentPosition`, `elementFromPoint` |
| Ranges and selection | `Range` and `document.createRange` for boundary points, text and geometry — `getClientRects` over a run of characters — `caretRangeFromPoint`/`caretPositionFromPoint`, and a `Selection` a script sets and reads; supported text controls also expose a user-placeable caret and drag selection, but generic document selection remains script-driven and the tree-editing range methods are absent |
| Scrolling | `window.scrollTo`/`scrollBy`/`scroll`, `scrollX`/`scrollY`/`pageXOffset`/`pageYOffset`, `element.scrollTop`/`scrollLeft`, `scrollIntoView` |
| Parsing | `innerHTML`, `outerHTML`, `insertAdjacentHTML`, `insertAdjacentElement`, and `DOMParser` for `text/html` into a fragment |
| Scheduling | `requestAnimationFrame`, timers and microtasks |
| Networking | `fetch`, `Headers`, `Request`, `Response`, `Blob`, `AbortController` over `http`/`https`, with buffered bodies |
| Audio | Web Audio — a context, gain, stereo panning and buffer sources over decoded files — and `<audio>`/`new Audio()` for whole-file playback |
| Routing | In-memory `history` and `location`, `popstate` and `hashchange` |
| CSS | Static block, flex and grid layout; bounded absolute positioning; spacing, borders, backgrounds, colors and system typography |
| Subresources | `<img>` and CSS `background-image` (PNG, JPEG, GIF, WebP and SVG), and `@font-face` web fonts (WOFF2, WOFF, TTF, OTF), loaded from local files; `<video>` is not. Audio is loaded and decoded by Web Audio rather than as a renderer subresource — see [Audio](#audio). A subresource the export cannot serve — a remote URL, or a local file that is missing — is answered with an empty body, so the document paints without it rather than waiting on it |

The M3b acceptance app intentionally uses the normal Vite default output, including
root-relative `/assets/...` references and Vite's module-preload bootstrap. It contains no
Blitsen imports or runtime branches.

## Development: your own dev server

```sh
blitsen http://localhost:5173      # while `vite` is running in another terminal
```

The window replaces the browser tab and nothing else about the inner loop
changes. The document, its module graph, its stylesheets and anything it
`fetch`es come from the server; your bundler goes on transforming, watching and
hot-reloading, and **source is fine here** — a dev server is what compiles it.

What holds:

| | Proxy mode |
| --- | --- |
| Modules | Loaded over HTTP as served, query strings and all: `/src/main.jsx?t=1738` is asked for as written, because that is a different response from `/src/main.jsx` |
| `import.meta.url` | The application origin, as everywhere else — `blitsen://app/src/main.jsx` — so an asset resolved against it is a sibling and `fetch` reads it back through the server |
| Hot reload | The channel is an ordinary `WebSocket` back to the dev server, and it stays open: messages land on the frame turn like any other socket's |
| A server that is not up yet | Waited for, then reported: `blitsen http://localhost:5173` before `npm run dev` waits ten seconds and then says nothing is answering, and what to do |
| A server that restarts | Reads fail while it is down, are named once on stderr, and succeed again when it comes back |
| `build` and `doctor` | Refused with a URL. Both read files; a dev server has no output directory to ingest or scan |

Two things to know:

- **Vite logs one connection error before its HMR socket connects.** Its client
  derives a socket URL from `location`, which is the application origin here and
  not a host it can reach; it then falls back to the host and port the server
  injected, which works. Setting `server.hmr.host`/`clientPort` in your Vite
  config removes the message.
- **Source maps are not consumed.** A stack frame names the served module URL —
  `blitsen://app/src/main.jsx:12:5` — which is the file you wrote when the server
  serves modules one-to-one, and the transformed line when it does not. Mapping
  frames back through `//# sourceMappingURL` is not implemented.

## Built output, not source

Blitsen loads the module graph a bundler already produced. It transpiles nothing and resolves no
bare specifier, by decision — so pointing it at a source tree is refused rather than half-supported:

```sh
cd my-app && blitsen                    # index.html loads /src/main.jsx
blitsen: /src/main.jsx is JSX source, not built output — a browser could not run it either.
Blitsen loads the graph a bundler already produced: build the application (Vite: `vite build`)
and point Blitsen at the output directory.
```

`.ts`, `.mts`, `.cts`, `.tsx`, `.jsx`, `.vue` and `.svelte` at a `<script src>` are the refusal;
`doctor` grades the same entrypoint `HTML_SOURCE_ENTRY`, an error, so a build stops before an
export exists. A bare specifier inside a module — `import React from "react"` — is refused where it
is resolved, naming the same fix.

This is stricter than it was. The Phase 1 host is Bun, which transpiles JSX and resolves
`node_modules` itself, so a source tree used to render; an author could develop against something
that would stop working under the shipped runtime, which is exactly the surprise
[#90](https://github.com/krazyjakee/blitsen/issues/90) exists to prevent.

**The refusal is about reading source off disk, not about developing.** Point Blitsen at your dev
server — `blitsen http://localhost:5173` — and the same `/src/main.jsx` runs, because the server
transforms it and Blitsen reads what it serves. That is a deliberate mode with its own behaviour
([above](#development-your-own-dev-server)) rather than an accident of which engine is hosting.

## Asset URLs

There is no web server behind an exported application, so a URL that assumes a server root has to
be resolved against the application instead. **Blitsen rewrites server-root URLs while ingesting,
in its own staging copy — your `dist` directory is never modified.**

Running a directory resolves them the same way rather than rewriting anything, so `blitsen dist`
and `blitsen build dist` accept the same output. They used to disagree, and the directory the
export accepted was the default `vite build` one.

A subresource the directory does not carry — a missing file, a remote URL — is named on stderr and
the document renders without it, which is what the export already does with it. Only a reference
that leaves the application directory is refused, because an export can serve nothing outside what
it collected and a directory being run is held to the same files.

| You wrote | Blitsen does |
| --- | --- |
| `href="./assets/app.css"` | Nothing; relative URLs already work. |
| `src="/assets/app.js"` (default `base`) | Rewrites to the equivalent document-relative path. |
| `src="/app/assets/app.js"` (custom `base: "/app/"`) | Drops the base prefix that does not exist in the output, then rewrites. |
| `url("/assets/hero.png")` in CSS, and `@import` | Same rewrite, applied transitively. |
| `<a href="/settings">` | Nothing; anchors are navigation, not subresources. |
| `href="https://cdn…"` or `//cdn…` on a subresource | Warns. The request is answered with nothing, so the page renders without that stylesheet, font or image. |
| `<script src="https://cdn…">` | Warns. The loader skips that one script and says so on stderr; every other script on the page still runs. |

**Only HTML and CSS are rewritten.** JavaScript is left byte-identical, because a path assembled
at runtime cannot be safely edited by a regular expression. In practice:

- `new URL('./x.png', import.meta.url)` and a relative `import()` **work** — the export preserves
  your directory layout, so relative resolution lands on the same file it did on a server.
- `fetch('./data.json')` and `fetch('/data.json')` **work**, and read the file the export ships —
  see [Networking](#networking). A literal path that names nothing in the output is diagnosed as
  an error at build time.
- `new URL('/assets/x.png', import.meta.url)` and any specifier built from a variable or template
  literal are **not diagnosed**. The server-root form resolves against whatever origin the module
  is on, so it finds the file inside an export and inside a directory run by the shipped runtime,
  and lands outside the application on the Phase 1 host, whose modules are on `file:` (see
  [Module identifiers](#module-identifiers)). Configure your bundler with a relative base (Vite:
  `base: './'`) if your application computes asset URLs from a server root, and the question does
  not arise.

### Module identifiers

A module script is named by an absolute URL, and `import.meta.url` is that URL. Which origin it is
on depends on the host, and nothing else does:

| | Shipped runtime | Phase 1 (Bun host) |
| --- | --- | --- |
| Inline `<script type="module">` | `blitsen://app/index.html#script-2` | `file:///…/index.html#script-2` |
| `<script type="module" src>` | `blitsen://app/assets/app.js` | `file:///…/assets/app.js` |

The fragment on an inline module is what makes one distinct from the next; it does not affect
resolution. The Phase 1 host is on `file:` because its module loader is the filesystem's — that is
also what makes `createRequire(import.meta.url)` reach a `.node` addon there — and the shipped
runtime is on the application origin because there is no filesystem inside an executable.

What holds on both, and is what an application depends on: the identifier is an absolute URL, a
relative asset resolved against it is a sibling of the module, and `fetch` reads that URL out of
the application. `test:hosts` asserts all three on both hosts, and a directory being run answers
the same way the export does.

## Networking

`fetch`, `Headers`, `Request`, `Response`, `Blob`, `AbortController` and `AbortSignal` are
Blitsen's own, backed by `reqwest`. They are not the host's: the Phase 1 Bun globals are replaced
so that the Phase 2 engine swap changes nothing an application can observe.

**There is no same-origin policy and no CORS, and this is deliberate.** An exported application
is trusted native software that happens to be written in HTML, not a document downloaded from a
site, so there is no origin to protect it from and no server to ask for permission. A request
goes where the application sends it. `mode`, `credentials`, `integrity` and `referrerPolicy`
describe a policy Blitsen does not have; they are not exposed on `Request` and passing them to
`fetch` changes nothing.

Requests run on a worker pool, never on the thread that owns the DOM. **Results land at one
defined point in the frame turn** — the start of the animation-frame stage, before any
`requestAnimationFrame` callback of that turn — so a response can never arrive in the middle of
one. The promise reactions themselves run at the microtask checkpoint that ends the turn, which
means a handler that mutates the DOM is painted by the following frame.

**`fetch` reads the files the application shipped.** A URL that resolves to a file inside the
application is answered from that file, out of the same source the renderer already reads images
and fonts from and the module resolver already reads scripts from — the export directory while one
is being run, and the section appended to the executable once it is exported. It is what makes the
idiomatic spelling work:

```js
const response = await fetch(new URL("./sounds/blip.wav", import.meta.url).href);
const buffer = await context.decodeAudioData(await response.arrayBuffer());
```

Without it an application could not read a file it shipped **at all**: `fetch` was http(s) only,
the shipped runtime implements no `node:fs`, and `blitsen/app` answers with directories rather than
contents. `decodeAudioData` therefore had no reachable source, and neither did a bundled `.json`
or `.wasm`.

Three consequences worth stating:

- **A path the application does not ship is a 404**, with a readable empty body — the web's own
  answer, so a caller that checks `response.ok` and falls back keeps working. Catching a typo is
  `doctor`'s job, and it does it at build time: a literal path at a `fetch` call site is resolved
  against the output, and reported as an error only when nothing there answers it. Two spellings
  are read — `fetch("./data.json")` and `fetch(new URL("./data.json", import.meta.url))`, the
  second resolved against the file it was written in, because that is what `import.meta.url`
  means. **The rule is literal-only**: a URL assembled from a variable or a template has nothing
  to resolve, and `doctor`'s silence about one is not a statement that it will arrive.
- **A URL outside the application is refused**, `file:` included. An application reading its own
  files is a different thing from one reading the disk; the second is what the `blitsen/*` modules
  and a native addon are for, and it is deliberately not what a web API does.
- **Both spellings of the same file agree.** `file:///…/blip.wav` while a directory is being run
  and `blitsen://app/blip.wav` inside an export name the same bytes, which is the property that
  keeps the two shapes from diverging.

**Streaming bodies are not implemented, and will not be in v1.** `fetch` buffers a whole
response. `Response.prototype.body`, `ReadableStream` and `Response.clone` are *absent*, not
null-valued, so `if (response.body)` selects a buffered fallback correctly. The reason is
coherence rather than difficulty: WHATWG streams are a large surface Blitsen does not otherwise
provide, and exposing the host's would reintroduce exactly the Phase 1/Phase 2 divergence this
API exists to remove. A per-chunk delivery path also has no defined place in the frame turn
above, which is the contract the rest of the runtime is built on. Revisit when an application
needs a download progress bar or a long-lived response, not before.

| You wrote | What happens |
| --- | --- |
| `fetch("https://api…")` | Runs off-thread; resolves at the next frame turn. |
| `fetch("./data.json")`, `fetch("/data.json")` | Reads the file the application shipped. See below. |
| `fetch("/api/data")` | Fails: the export ships no `api/data` and there is no server behind the document address. `doctor` reports it as an error at build time. |
| `new Request(…)`, `new Headers(…)`, `new Response(…)` | Full subset above, including `AbortSignal`. |
| `response.text/json/arrayBuffer/blob()` | Supported; a body is readable once. |
| `response.body`, `response.clone()`, `FormData` bodies | Absent. |
| `window.stop()` | Aborts the load in progress; see below. |

**`window.stop()` aborts loading, and only loading.** Every outstanding `fetch` rejects with an
`AbortError` — the rejection its own `AbortSignal` would have produced — and every subresource the
renderer is still waiting on is cancelled *and settled*, never merely abandoned: a request left
pending would block painting for the life of the document, which is the opposite of what a caller
asking to stop loading wants. Timers and animation frames keep running, as they do in a browser;
they are the application's own work, not the document's load. There is no parser half either — a
Blitsen document is parsed whole before any of its scripts run. A request made afterwards loads
normally, because `stop()` ends the load in progress rather than the document's ability to load.
With nothing in flight it does nothing observable, which is not the same as being a function that
does nothing: both halves run and find nothing to abort.

### WebSocket

`WebSocket` is Blitsen's own, backed by `tokio-tungstenite`, and is the streaming path this
runtime does have. The constructor, `url`, `readyState` and its four constants, `protocol`,
`extensions`, `bufferedAmount`, `binaryType`, `send` and `close` are all present, as are the
`open`, `message`, `error` and `close` events; a close carries its code, reason and `wasClean`.

Text and binary frames both work. `binaryType` is `"blob"` by default and `"arraybuffer"` when
asked, and the choice is made once at the boundary rather than by converting afterwards. `send`
accepts a string, a `Blob`, an `ArrayBuffer` or a typed array, and throws `InvalidStateError`
before the socket is open — which is the one thing about a socket that is not a queued no-op.

`wss://` uses the platform certificate store, through the same `native-tls` backend `fetch`
resolves to, so a certificate the desktop trusts is one the socket trusts.

The connection runs off the thread that owns the DOM, and **frames land at the same defined point
in the frame turn that `fetch` results do**. An open socket keeps the host turning, for the same
reason an in-flight request does: its landing point is that turn, so a loop that idled would never
deliver. A non-`ws:`/`wss:` address is refused with a `SyntaxError` at construction.

### Server-sent events

`EventSource` is implemented (#236), over the same worker pool `fetch` and `WebSocket` run on and
delivered at the same point in the frame turn. The whole of it is there: named events through
`addEventListener`, `MessageEvent` with `data`, `lastEventId` and `origin`, `readyState` and the
three constants, `close()`, and comment lines that keep an idle connection warm.

**Reconnection is the transport's, not the application's.** A stream whose body ends — a proxy
timing out, a server restarting — fires `error` with `readyState` back at `CONNECTING`, waits the
interval a `retry:` field asked for (three seconds if none did), and reconnects carrying
`Last-Event-ID`, so a feed resumes where it stopped rather than from the top. A response that is
not a `200 text/event-stream` is a different thing: that fires `error`, settles at `CLOSED` and is
not retried, because retrying a 404 forever is not a reconnection.

`withCredentials` is reflected and withholds nothing: there is no cookie store and no per-origin
credential in this runtime, so there is nothing for `false` to keep back. Only `http:` and `https:`
addresses are accepted; anything else is a `SyntaxError` at construction.

## Workers and messaging

A `Worker` is a whole JavaScript engine on a thread of its own, and **nothing is shared with the
document but messages**. That is not a restriction Blitsen adds: the DOM is not thread-safe and no
host's values are shareable across threads, which is the same reason the web specifies workers
this way.

**A worker loads its script out of the application**, through the same resolver the document's
modules go through, so `new Worker(new URL("./work.js", import.meta.url), { type: "module" })`
names the same file whether the application is a directory being run or a section inside an
exported executable. `import` works inside a worker and resolves against the worker's own URL. A
script the application does not ship is refused at the constructor, naming it, rather than
becoming an `error` event a turn later.

**What a worker's global scope has:** `self`, `name`, `location`, `postMessage`, `close`,
`onmessage`/`onmessageerror`, `addEventListener`, the timers, `queueMicrotask`, `console`,
`performance`, `fetch` and the request/response classes, `MessageChannel`, `MessagePort`,
`structuredClone`, `DOMException`, and `Worker` itself — a worker may start a worker.

**What it does not have:** any DOM at all — no `document`, no `window`, no `localStorage`, no
`requestAnimationFrame` — and no `navigator`, no `WebSocket`, and no `importScripts`. A classic
worker (`type: "classic"`, the default) therefore has no way to load a second file; use a module
worker, which is what every bundler emits anyway.

### Delivery lands in the frame turn

A message from a worker is delivered at the **start of the animation-frame stage**, the same
point `fetch` completions and socket frames land at, so it can never arrive part-way through a
callback. The cost is stated plainly: a reply's latency is bounded by the frame, not by the thread
that sent it — around 16 ms at 60 Hz. A worker that answers a thousand messages a second will be
paced by the document's frame rate, so batch them. Inside a worker the same messages are delivered
at the top of its own turn, which is not frame-paced. A live worker keeps the host turning, for
the same reason an open socket does.

### What survives a message

Structured clone, not JSON. Cycles and shared references are preserved, so an object that refers
to itself arrives referring to itself, and two `Uint8Array`s over one `ArrayBuffer` arrive as two
views over one buffer. `Map`, `Set`, `Date`, `RegExp`, `Error` and its subclasses, `BigInt`,
typed arrays and `DataView`, boxed primitives, array holes and `-0` all cross unchanged.

Refused with a `DataCloneError`, rather than silently flattened: functions, symbols, DOM nodes,
and anything else whose prototype the other side could not rebuild. **`SharedArrayBuffer` is
refused too** — the engine defines it, because it is an ECMAScript global, but each worker has a
heap of its own and there is no shared memory between them. `Atomics` on a buffer that cannot
cross is not useful, and a copy pretending to be shared memory would be worse than a refusal.

### Transfer moves rather than copies

`postMessage(value, [buffer])` detaches the `ArrayBuffer` here and delivers it whole there;
reading the buffer afterwards finds it zero-length, as the specification requires. A transfer list
is emptied by a *successful* send, so a message that could not be serialized leaves the sender
still holding everything it was about to give away.

A `MessagePort` may be transferred the same way, including to a worker, and **its queued messages
travel with it**: a message sent just before the port was handed over arrives where the port went.
A transferred port arrives stopped, so the receiving side's `onmessage` — or `start()` — decides
when its queue begins moving. Ports named in the transfer list arrive as `event.ports` whether or
not they also appear in the message body.

### Ending a worker

`terminate()` stops the worker even if it is inside a loop that never yields: the engine's
interrupt handler sees the same flag the worker's event loop does. Whatever the worker had queued
for this side is dropped. Reloading a document ends every worker it started.

An exception nothing in a worker caught — including one thrown while its module was evaluating —
is reported to the `Worker` object as an `error` event carrying the message, as well as to this
process's stderr. The worker keeps running, as a browser's does.

`window.postMessage(message)` also works, and means the same-window one: the message is serialized
at the call, delivered as a later task, and `targetOrigin` is accepted and ignored, because there
is one origin behind an application. `SharedWorker`, `ServiceWorker` and `BroadcastChannel` are
absent — all three are about sharing something between documents, and there is one document.

## Audio

Backed by [`web-audio-api`](https://crates.io/crates/web-audio-api), which is a Rust
implementation of the Web Audio API itself rather than a playback library with a graph rebuilt on
top of it — so what an application schedules is what the specification says it scheduled. Decoding
is Symphonia; output is `cpal` (WASAPI, CoreAudio, ALSA).

**Nothing opens the sound card until an application asks it to.** The context is created on the
first `new AudioContext()`, so an application that never plays a sound never touches the device.

**A machine with no output device still runs.** If the device cannot be opened the context falls
back to a silent sink and says so once on stderr. An application then behaves exactly as it would
for a user who has muted their speakers, which is the same thing as far as its own code can tell —
rather than throwing from a constructor and taking down every page that plays a click.

### What is implemented

| | |
| --- | --- |
| Context | `AudioContext`, `sampleRate`, `currentTime`, `state`, `destination`, `resume`, `suspend`, `close` |
| Nodes | `GainNode`, `StereoPannerNode`, `AudioBufferSourceNode`, `AudioDestinationNode` |
| Parameters | `AudioParam` — `value`, `setValueAtTime`, `linearRampToValueAtTime`, `exponentialRampToValueAtTime`, `setTargetAtTime`, `cancelScheduledValues` |
| Buffers | `decodeAudioData` (promise and callback forms), `AudioBuffer`, `getChannelData` |
| Element | `Audio`, `<audio>`, `HTMLAudioElement` — `play`, `pause`, `currentTime`, `duration`, `volume`, `muted`, `loop`, `paused`, `ended`, `canPlayType` |

**Formats are whatever Symphonia decodes**, which is not selectable: AAC, ADPCM, ALAC, FLAC,
MP1/MP2/MP3, PCM and Vorbis, in AIFF, CAF, ISO/MP4, MKV/WebM, Ogg, WAV and raw containers.
`canPlayType` answers `"probably"` or `""` and never `"maybe"`, because a maybe tells a caller
nothing.

A media element's source is read the same way, so `<audio src="blip.wav">` reads the shipped file
whether the application is a directory being run or an exported executable.

**Decoding runs on the worker pool** and lands at the same point in the frame turn `fetch` results
do. A decode in flight keeps the host turning, so its result cannot be stranded.

A source plays **once** — the specification says so, and starting one twice throws
`InvalidStateError`. Overlapping playback of one sound is several sources over one decoded buffer,
which is also what makes it cheap: the decode is paid for once.

### What is absent, and why

**The rest of the Web Audio graph.** `BiquadFilterNode`, `OscillatorNode`, `AnalyserNode`,
`ConvolverNode`, `DelayNode`, `DynamicsCompressorNode`, `WaveShaperNode`, `PannerNode` and its HRTF
spatialisation, `ChannelSplitterNode`/`ChannelMergerNode`, `AudioWorklet`, `OfflineAudioContext`
and `AudioListener` are all things the backing crate implements and this bridge does not name.
That is deliberate: every API named here is a published claim that `doctor` and the capability
tiers make on Blitsen's behalf, and the surface above is what an application asking for sound
effects, cues and a background loop actually uses. They are cheap to add when something measured
asks for one.

**`<audio>` is not a streaming media element.** The source is fetched whole and decoded whole
before playback starts, which is right for the sounds a desktop application has and wrong for an
hour of audio. `buffered`, `seekable`, `readyState`, `networkState`, `preload`, `played` and
`HTMLMediaElement` itself are absent rather than answered with a fiction. `<video>` is not
implemented at all.

**`webkitAudioContext`** is absent: it is a prefix for a browser this is not.

A source announces when it finishes: `ended` fires on the node, and on an `<audio>` element that
leaves it `paused`, `ended` and rewound, so the same element can be played again. The announcement
comes off the render thread and is delivered at the frame turn like everything else, and a sound
that is still playing keeps the host turning — so a loop that never ends is a host that never
idles, which is correct but worth knowing.

### Testing audio

`BLITSEN_AUDIO_OFFLINE=1` makes the context an offline one that renders to sample buffers with no
device at all. That is how Blitsen's own harness asserts on audio — reading the samples that came
out, the same way the renderer's tests read painted pixels — rather than on the calls that were
made. A graph built correctly that rendered silence would pass any check that only read properties
back.

An offline context has **no clock**: it renders when it is asked to and not before, so nothing in
it can be observed to *finish*. Anything about the end of a sound is therefore tested against a
real context with a real clock and no output device, which the harness selects for itself. The
three modes answer different questions, and only the first is what an application gets:

| Mode | Clock | Output | Answers |
| --- | --- | --- | --- |
| device | real time | the sound card | what an application does |
| silent | real time | none | when a sound started and finished |
| offline | on demand | sample buffers | what the samples actually are |

## Routing

`history` and `location` exist and are **in memory only**. There is no navigation, no network and
no back-forward cache — the document is never left, so nothing to navigate to and nothing to
restore. This is the surface a client-side router needs (React Router, Vue Router and
equivalents), and it is deliberately not more than that.

The document address is `blitsen://app/`. It is synthetic because an exported application has no
server and therefore no origin, and it is path-rooted because that is what a router reads. The
scheme makes the absence of an HTTP origin visible rather than pretending to be `localhost`.

| Supported | Absent |
| --- | --- |
| `location.href/protocol/host/hostname/port/origin/pathname/search/hash` | `location.assign/replace/reload`, `ancestorOrigins` |
| `location.hash = …` (pushes an entry, fires `hashchange`) | Assigning `href`, `pathname` or `search` — refused with a `NotSupportedError` naming `pushState`, never silently |
| `history.pushState/replaceState/go/back/forward`, `length`, `state`, `scrollRestoration` | Cross-origin entries — refused with a `SecurityError`, as in a browser |
| `popstate` and `hashchange` on `window`, `PopStateEvent`, `HashChangeEvent`, `document.location` | `navigation` (the Navigation API) |

Two differences from a browser worth knowing. `history.state` holds the value you pushed rather
than a structured clone of it, so mutating that object mutates the entry. And `scrollRestoration`
is recorded and reported but restores nothing, because a traversal never reloads a document.

An anchor still does nothing: `<a href="/settings">` is navigation, and a router that calls
`preventDefault` and `pushState` is what makes it work — which is what every client-side router
already does.

Checked against the real libraries rather than a reading of their source: React Router 7
(`createBrowserRouter` and `createHashRouter`, including `navigate(-1)` traversal through
`popstate`) and Vue Router 4 (`createWebHistory` and `createWebHashHistory`, including
`router.back()`) resolve, match and traverse routes unmodified on this surface.

## Nodes, fragments and templates

The HTML parser makes node kinds `createElement` cannot, and framework runtimes need every one of
them: `createComment` for Vue's `v-if` and fragment anchors, `createElementNS` for inline SVG,
`createDocumentFragment` and `<template>.content` for Svelte 5's cloned templates. These are real
nodes in the renderer's tree, not JavaScript stand-ins.

Three differences from a browser are worth knowing:

- **Collections are static.** `children`, `querySelectorAll`, `getElementsByTagName`,
  `getElementsByClassName` and `attributes` return a snapshot rather than a live collection. A
  re-query sees a mutation; the collection handed out before it does not. The `Attr` objects in
  `attributes` are the exception — each still reads and writes through its element.
- **A fragment is a detached `<template>` element underneath**, which is what gives its children a
  real parent to be parsed, serialized and cloned against — including table rows, which any other
  parsing context would discard. `cloneNode(true)` copies by serializing and reparsing, so a clone
  carries the tree and its attributes and nothing else: no listeners and no JavaScript state, which
  is what the DOM specifies anyway.
- **`template.content` takes the parsed children off the element** the first time it is read,
  because Blitz has no separate template-contents document. The element is empty afterwards, which
  is what the specification says it was all along.

A comment's data is fixed when it is created, and data that would close the comment early
(`-->`) is refused rather than silently truncated. `attachShadow` remains absent, as does
`document.currentScript`: nothing in the bridge is told which script element is executing.

`setAttributeNS`, `getAttributeNS` and `removeAttributeNS` key an attribute by namespace and
local name, which is the pair they ask for — so `xlink:href` round-trips and `getAttribute`
correctly does not see it. The prefix itself is not stored: `getAttributeNames()` reports `href`
and serialization writes `href="…"`, which is already true of markup the parser read.
`getClientRects` returns one rectangle per line box the element was broken across, off the same
layout flush. Anything with a box of its own has exactly one, and it is the border box
`getBoundingClientRect` returns; an inline element that wraps has one per line, and their union is
all a single rectangle could have said.

`link.relList` exists chiefly so that `relList.supports("modulepreload")` can answer truthfully.
Without it Vite's own module-preload polyfill installs itself and `fetch`es every chunk over an
address with no server behind it, which takes down any code-split build. The preload keywords are
honoured by doing nothing: an exported application's chunks are local files with no cache to warm.

**`link.onload` and `link.onerror` fire for a `<link rel="stylesheet">`**, including one script
inserted after the document loaded — the path a theme switcher and every deferred-CSS loader takes.
The event is delivered at the frame boundary, where image completions and `fetch` answers land, and
it is delivered *after* the sheet is in the cascade: a handler that calls `getComputedStyle` reads
the values the sheet resolved to, not the ones it replaced. Rewriting `href` on a link that has
already loaded is a new request and fires again. Three things to know:

- **Only `rel="stylesheet"` fires either event.** A `preload`, `prefetch` or `icon` link is never
  fetched — see `relList` above — so it is owed no outcome, and reporting one would mean announcing
  a request that was never made. Nothing fires for those, in either direction.
- **An empty stylesheet file reports `error`.** The renderer answers a subresource it cannot serve
  with zero bytes rather than dropping the request, because a dropped one leaves Blitz waiting on a
  critical resource for the life of the document. That makes "the file is missing" and "the file is
  genuinely empty" the same signal. An empty sheet contributes nothing to the cascade either way.
- **Nothing is delivered retroactively.** A listener attached after the sheet settled receives
  nothing, exactly as in a browser. Attach the handler before connecting the element.

## Reading style back

Blitz has already resolved the cascade, evaluated `@media` and laid the document out. These three
APIs ask it those answers from JavaScript rather than keeping a second idea of what an element's
style is; none of them is a shadow implementation.

**`getComputedStyle(element)`** returns a read-only `CSSStyleDeclaration` over the resolved
style — the stylesheet, not the inline declaration `element.style` reads. It is live: the same
object reflects a class or attribute mutation on the next read. Custom properties resolve through
inheritance, so a `--brand` declared on `:root` reads on any descendant.

Every read is layout-dependent, because CSSOM resolves `width` and `height` to the **used** value:
`width: 50%` reads as the pixel width layout produced. So a read goes through the same layout flush
`getBoundingClientRect` does, and a read *after* a write is a forced synchronous layout — the
expensive kind, counted by `BLITSEN_DEV_LAYOUT_WARNINGS` alongside the geometry reads. Batch reads
before writes as you would in a browser.

Where it differs from a browser:

- **An element the cascade has never reached reads empty.** A detached element is not in the
  document, so this renderer has no resolved style for it at all, and every property reads `""`
  rather than the initial value a browser would invent. Everything connected — including
  `display: none` subtrees — resolves normally.
- **Shorthands serialize from their longhands**, and read `""` when those longhands do not compose
  into one — which is also what `all` does in a browser. `margin`, `padding`, `border-width` and
  the rest of the ordinary shorthands compose.
- **The declaration is not enumerable.** `length`, `item()` and index access are absent; ask for
  the properties you want by name. `cssText` is `""`, which is what CSSOM specifies for a computed
  block anyway, and every mutator throws `NoModificationAllowedError` rather than silently
  ignoring the write.
- **Pseudo-elements are refused**, with a `NotSupportedError` naming the selector. A pseudo-element
  box is not addressable here, and answering with the originating element's style would be a wrong
  answer rather than a missing one.
- Only `width` and `height` are used values. The inset and box properties report their computed
  value, which is the declaration resolved to absolute units rather than the used geometry.
- **`pointer-events` reads `auto` where a browser reads `all`.** The cascade parses only `auto` and
  `none`; the nine values that mean "this element takes hits" — `all`, `visible`, `painted`, `fill`,
  `stroke`, the `visible*` trio and `bounding-box` — are rewritten to `auto` as the CSS enters the
  document, because the alternative is the cascade dropping them and the element inheriting the
  `none` of the container it sits in. The element behaves as declared; only the readback is the
  other word for it. See G15 in [`BLITZ-GAPS.md`](BLITZ-GAPS.md).

**`matchMedia(query)`** runs the query through the same parser and the same evaluator the cascade
uses for `@media`, so what matches in a stylesheet matches here. `MediaQueryList` carries `media`,
`matches`, `addEventListener("change")`, `onchange`, and the pre-2019 `addListener`/`removeListener`
a library still installs; the event is a `MediaQueryListEvent` with `media` and `matches`.

The features the style engine implements are `width`, `height`, `device-width`, `device-height`,
`orientation`, `aspect-ratio`, `resolution`, `device-pixel-ratio`, `scan`, `pointer`, `any-pointer`,
`hover`, `any-hover` and `prefers-color-scheme`. Anything else — `prefers-reduced-motion`,
`prefers-contrast`, `forced-colors` — is an unknown feature to the engine, and an unknown feature
does not match, which is the CSS answer rather than a Blitsen one. An unparsable query serializes
as `not all` and does not match, as it does in a browser.

`prefers-color-scheme` is **`light` for the life of the process**: the window is created with a
light colour scheme and nothing changes it yet, so a dark-mode toggle driven by the system
preference stays light while one driven by a class or `localStorage` works normally. The only
device state that can change is the viewport, so a `change` event is dispatched when — and only
when — a window resize flips a query, at the start of the frame turn.

**`ResizeObserver`** observes elements, with `observe`, `unobserve` and `disconnect`. An entry
carries `target`, `contentRect`, `borderBoxSize` and `contentBoxSize`; `contentRect` is the content
box positioned from the border box's own origin, exactly as the specification defines it.

Observations are delivered **at the start of the frame turn**, beside the `<blitsen-view>` surface
resizes and before any `requestAnimationFrame` callback — the same defined point in the turn that
network results land at. The first observation for an element is guaranteed: an undelivered
observation keeps the host turning the way an in-flight request does. A browser delivers after
layout and before paint instead, so an entry here describes the layout the previous frame settled
on. `box: "device-pixel-content-box"` is refused with a `TypeError`, because the device-pixel
snapping it reports is not exposed; `inlineSize` is width and `blockSize` is height, which holds
for every writing mode this renderer lays out.

`IntersectionObserver` and `PerformanceObserver` remain absent.

## Rendered text, box reads and scrolling

The read-back surface a component library reaches for once it starts measuring rather than only
rendering. Each of these asks Blitz the question rather than keeping a second answer beside it.

**`innerText`** is rendered text, which is the whole of what separates it from `textContent`: a
`display: none` or `visibility: hidden` subtree contributes nothing, a `<br>` is a line break, and
a block-level child starts a new line. What it does *not* do is re-derive line wrapping — it reads
the tree and its computed display rather than Blitz's line boxes, so a paragraph that wrapped over
three lines is one line here. Writing it is the inverse: newlines become `<br>` elements.

**`clientTop` and `clientLeft`** are the resolved border widths, read from the computed style
rather than differenced out of the border and content boxes — which would fold the padding in too.
**`offsetParent`** is the nearest positioned ancestor, or the body once the walk runs out, and is
`null` for an element the cascade is not laying out.

**`compareDocumentPosition`** returns the DOM's bitmask, computed by walking to the common
ancestor. Two nodes in different trees report `DISCONNECTED | PRECEDING |
IMPLEMENTATION_SPECIFIC`, as a browser does.

**`document.elementFromPoint`/`elementsFromPoint`** are the hit test the native window already runs
for every click, asked the other way round.

**Scrolling.** `window.scrollTo`, `scrollBy` and `scroll` move the document — `document.scrollingElement`,
the root element — and `scrollX`, `scrollY`, `pageXOffset` and `pageYOffset` read the offset back
live. `element.scrollIntoView` scrolls each scrolling ancestor and then the document until the
element's border box is inside each one, honouring `block` and `inline` including `nearest`.
`behavior` is accepted and ignored on both: there is no animation to run, so the scroll lands.

**`hidden`** reflects the content attribute, and the user-agent rule mapping `[hidden]` to
`display: none` applies as it does in a browser — including being overridden by an author rule that
sets `display` on the same element, which is ordinary cascade order rather than a Blitsen quirk.

`CSS.escape` and `CSS.supports` are present. `supports` answers from the cascade's own parser, by
round-tripping the declaration through an inline style, so it reports what *this* runtime accepts;
its one-argument form understands a plain `(property: value)` condition and answers `false` for a
compound one rather than guessing at it.

**`DOMParser` parses `text/html` into a detached fragment**, not into a second document — there is
one document in this runtime. `body` and `documentElement` are that fragment, and `head` is `null`,
because the fragment parser drops `<html>`, `<head>` and `<body>` tags and a parsed string is
therefore not split into head and body. An XML type is refused with a `TypeError` rather than run
through the HTML parser, which would mis-parse it silently.

### What is absent here, and why

- **`customElements` and `ShadowRoot`** — a decision, not an omission. Upgrading elements after
  parsing, running the lifecycle callbacks and ordering reactions is real machinery, and
  `<blitsen-view>` is registered natively rather than through a registry, so a user-defined element
  would need either its own registry beside that one or a merge of the two. Absent is also the
  shape a polyfill installs itself into.
- **`document.currentScript`** — the script runner does not carry the script element through to
  evaluation, and a module script's `currentScript` is `null` in a browser anyway, which is what
  every bundler in the profile emits.
- **`document.doctype`** — the backend's tree has no doctype node to report.
- **`outerWidth`, `outerHeight`, `screenX`, `screenY`** — the platform layer exposes no window
  frame or position, and `innerWidth`/`innerHeight` already answer for the viewport. A second
  answer that could disagree with those is worse than no answer.
- **`visibilityState`, `execCommand`, `createTreeWalker`, `createNodeIterator`, and the
  `document.forms`/`images`/`links`/`scripts` collections** — each needs a reason to exist rather
  than a reason not to, and none has one yet.

## Ranges, carets and the selection

**A range is how text is measured.** Every other geometry read in this runtime answers for an
element, and an element is the wrong unit for text: an editor laying out a line needs to know where
characters 4 to 11 of a text node are, and only the range API can ask that. `range.getClientRects()`
is the answer, and it is a real measurement of the laid-out text rather than an estimate from a
font metric — the same Parley layout the renderer paints from, read at the same flush and charged
as the same forced synchronous layout `getBoundingClientRect` is.

The list has **one rectangle per line box** a run was broken across, in line order, plus the border
boxes of the elements the range covers whole. `getBoundingClientRect()` is their union, and an
empty box when a range measured nothing — text in a `display: none` subtree, or a collapsed range,
which covers no characters and so returns no rectangles at all.

**Offsets are the DOM's, not the layout's.** The text Blitz lays out is not the text in the tree:
whitespace has been collapsed across node boundaries, `text-transform` has rewritten letters, a
`<br>` has contributed a newline no node owns and a list marker text that no node owns either. A
range counts UTF-16 code units in a text node's own data, the way a JavaScript string does, so
`node.textContent.slice(start, end)` and the rectangles for `start`–`end` always describe the same
characters. Rebuilding that correspondence is what the backend does before it measures.

**`caretRangeFromPoint(x, y)` and `caretPositionFromPoint(x, y)` are the same reading asked the
other way round**: which character is under this point. Both are here because a bundle has one or
the other spelling compiled into it — the first answers with a collapsed range, the second with a
`CaretPosition` carrying `offsetNode`, `offset` and a zero-width `getClientRect()`. A point over a
box that holds no text has no answer rather than a nearest one, so both return `null`. The
character the point is *inside* decides the node: a click on the right-hand half of `AB` in
`AB<span>CD</span>` is offset 2 of `AB`, not offset 0 of `CD`, even though those name the same
place in the text.

**`getSelection()` returns one object for the life of the document**, holding an anchor and a focus
rather than a range — that is what carries `direction`, which is `forward`, `backward` or `none`
and is what an editor reads to know which end is being dragged. `getRangeAt(0)` hands back a copy
in tree order rather than the live range a browser gives, and `selectionchange` is dispatched on
`document` in a later task, so a run of changes announces itself once, settled.

Two things the selection is not. **Nothing paints it**: this is the selection a script sets and
reads, and the renderer draws no highlight behind it. And **the user cannot make one**: dragging
across text does not move it, because text selection is a shell behaviour and the shell here has
none. A widget that maintains its own visible selection — which is what every editor does — works;
one that expects the platform to select text for it does not.

**Nothing edits through a range.** `deleteContents`, `extractContents`, `cloneContents`,
`insertNode` and `surroundContents` are absent, and absent rather than half-built: each one splits
a text node at a boundary point, this runtime has no `splitText` and no character-data interface to
split one with, and a range that cut in the wrong place would be worse than one that does not cut.
Edit the tree with the node methods, and use a range to measure and compare.

## Stylesheets

**A stylesheet is the element that owns it.** `document.styleSheets` lists the `<style>` and
`<link rel="stylesheet">` elements the cascade is reading, in the order it applies them;
`styleElement.sheet` is the same object, one per element for the element's whole life, and
`sheet.ownerNode` is the element it came from. A disconnected element has no sheet, because
nothing it says has reached the cascade.

That identity is the whole design. A `<style>` element's text *is* its sheet's source: Blitz parses
it and hands it to Stylo, and reparses it whenever the text changes. So `insertRule` and
`deleteRule` rewrite that text, and the rule they insert is in the same stylesheet set the
document's own rules are in. There is no second rule list that could parse successfully and then
cascade nothing, which is the failure this API is easiest to build.

Two consequences worth knowing:

- **`cssRules` is derived from the source on every read.** The list object handed out is a frozen
  snapshot, like every collection here, but the next read sees what the last mutation did — so an
  index taken from `cssRules.length` and passed straight to `insertRule`, which is how a framework
  appends a rule, means what it means in a browser. A rule's `cssText` is its source text, not a
  reserialization, so rewriting a sheet to insert one rule cannot quietly rewrite the others.
- **A rule is inserted whole or refused.** Text that is not exactly one rule — to the cascade's own
  parser, not just structurally — throws a `SyntaxError`, and an out-of-range index throws an
  `IndexSizeError`. Nothing is written that the cascade would then ignore.
- **The element's text follows its rules.** A browser keeps `styleElement.textContent` at whatever
  was authored and holds the rule list separately; here they are the same thing, so a sheet mutated
  through `insertRule` reads back its rules as the element's text. Comments between rules are not
  rules and do not survive a mutation, which is also what a browser's rule list reports.

What is absent: the `CSSRule` subclasses and everything read off a rule other than its text
(`style`, `selectorText`, `type`), `disabled`, `replace`/`replaceSync`, constructible sheets
(`new CSSStyleSheet()` throws, so a feature test selects its fallback) and `adoptedStyleSheets`.
The rules of a sheet loaded from a URL are refused rather than reported as empty: that sheet's
source is a file this process fetched, not text in the tree. It is still listed in
`document.styleSheets` and still answers `ownerNode` and `href`.

### The animation clock

CSS animations and transitions are sampled at **the frame's own timestamp**, set from the same
value `requestAnimationFrame` callbacks receive, once per frame turn. Nothing below that reads a
clock of its own, which is what keeps a replayed or recorded frame sequence identical to the one
that was captured, and a running animation keeps the host turning the way an in-flight request
does.

The consequence is that **animation only advances on delivered frames**. A harness that loads a
document and never turns the loop sees every animation pinned to its first keyframe — which is
correct, not stalled: no frame has been asked for. This is also why a `@keyframes` rule inserted
from JavaScript is worth having at all; until the clock was wired to the frame it would have
parsed, cascaded, and never moved.

## Form controls

The whole of this surface rests on one distinction: **the content attribute is the control's
default, and the property is its current state.** `value` is not `getAttribute("value")`. Typing
into a field, or assigning to `value`, moves the state and leaves the attribute where it was —
HTML calls that the dirty value flag — and from then on the attribute is only the default. So
`defaultValue` and `defaultChecked` are the attribute reflections, `value` and `checked` are the
state, and each pair moves without the other. Getting this backwards would look like it worked,
which is why it is the thing the tests assert first.

There is one copy of that state and it is the renderer's. Blitz already keeps a text editor for
`<input>` and `<textarea>` and a checkedness flag for a checkbox, and those are what it paints
from; `value` and `checked` read and write exactly those, rather than a second store beside them
that could disagree with the pixels. Two consequences follow. A value assigned before the control
has ever been laid out is held until Blitz builds its editor and then pushed into it, so nothing is
lost by writing early. And a `<textarea>`'s child text — its default value, where an input has an
attribute — is given to the editor too, so an untouched textarea paints what it reads and tracks
its children the way HTML says a textarea with no dirty flag does.

`<select>` and `<option>` are the exception, because Blitz renders a `<select>` as its options
rather than as a control and has no notion of selectedness. An option's selectedness is stored as
the same flag a checkbox uses, which is the flag `:checked` matches against, so `select
:checked` finds the selected option the way a browser does — Svelte 3 reads a bound select exactly
that way. `select.value`, `selectedIndex`, `selectedOptions` and `option.index` are all derived
from the options, so there is nothing to keep in step.

`options` and `form.elements` are **snapshots**, like every other collection this runtime hands
out: a re-read sees an option added since, the collection handed out before it does not.

Two divergences worth knowing:

- **A drop-down with nothing selected reports its first enabled option** rather than `-1`. That is
  the selectedness HTML resets a drop-down to, and it keeps `select.value` meaningful; what it does
  not do is stay at `-1` after `selectedIndex = -1` or after assigning a value no option carries.
- **`:checked` does not restyle.** A selector query evaluates it against current state and finds
  the right element, but changing checkedness does not invalidate the cascade, so a `:checked` CSS
  rule will not repaint. This is Blitz's behaviour for its own checkboxes too.

What a control *looks* like is a separate question from what it does, and it is the weaker half.
Blitz ships no equivalent of a browser's `forms.css`, so Blitsen appends the part of that baseline
the engine can honour — control cursors, unselectable labels, a visibly disabled control, a
`<fieldset>` that is a bordered block, and an `<a>` with no `href` that is not painted as a link.
The controls with no widget behind them are not covered and cannot be by a stylesheet: `<select>`,
`<meter>`, `<progress>` and `input[type=range|color|number]` paint nothing usable, `placeholder`
text is not drawn, and only `<input>` and `<textarea>` can show a focus ring. See G4 in
[`BLITZ-GAPS.md`](BLITZ-GAPS.md). An application that styles its own controls — as most component
libraries do — is unaffected by all of it.

### Submission

**`form.submit()` is absent, and `requestSubmit()` is not.** Submitting a form is defined as
navigating, and navigation is deliberately absent — there is no page to leave. `submit()` is
defined to skip the `submit` event and navigate, so an implementation could only be a silent
no-op or a throw; absent lets feature detection see it.

`requestSubmit([submitter])` is the half that means something without navigation: it fires a
bubbling, cancelable `SubmitEvent` at the form, carrying `submitter`, and does nothing further.
Clicking a submit button does the same, after the click and only if the click was not cancelled.
That is what a single-page application uses — `onsubmit` plus `preventDefault` — and it behaves
exactly as it would in a browser. An application that relied on the navigation gets nothing
instead of the wrong page.

A checkbox or radio clicked without the click being cancelled toggles and fires `input` and
`change`, and a checked radio clears the rest of its group. `form.reset()`, `action` and `method`
stay absent for the same reason `submit()` does: they describe a document navigation.

### Typing, and the caret

A key that reaches a focused `<input>` or `<textarea>` **edits it**. The keyboard events are
dispatched first and in full, because the edit is their default action: `preventDefault` on a
`keydown` stops the character from being typed, which is how a field that accepts only digits is
written. What happens next is announced and then reported — a cancelable `beforeinput` naming the
`inputType` about to be applied, the mutation, then a non-cancelable `input` saying it was. Both
are `InputEvent`s carrying `inputType` and `data`, and `input` is fired only when the value
actually moved, so backspacing at the start of a field is silent rather than a stream of empty
edits.

The operations behind the keys are `insertText`, `insertLineBreak` (Enter, in a `<textarea>` only —
a single-line field has no line to break, so Enter there is left to the application),
`deleteContentBackward`/`deleteContentForward` and their `deleteWord` pair under Ctrl. Arrow keys,
Home and End move the caret; Shift extends the selection and Ctrl widens each motion to a word or
to the whole value; Ctrl+A selects all. Clicking into a field puts the caret where the click
landed, shift-clicking extends to it, and dragging selects. A key a field took is not also a
scroll — a space typed into one does not page the document down behind it.

**Focus moves on `mousedown`, not on `click`**, because that is the event it is the default action
of and the only one an application can still refuse. A component that focuses something of its own
from a `mousedown` handler and then cancels the event keeps it — which is how every editor that
paints its own text and funnels keys through one off-screen `<textarea>` works, Monaco included.
Taking focus at `click` instead handed it back to the nearest focusable ancestor one event later,
so those keystrokes went to the body. Activation — a checkbox toggling, a submit button submitting
— stays on `click`, where HTML puts it. A press that lands on nothing focusable still blurs what
was focused, as it does in a browser.

`selectionStart`, `selectionEnd`, `selectionDirection`, `setSelectionRange()` and `select()` are
implemented on `<textarea>` and on the single-line-text input types, and are `null` on the rest:
HTML gives a date or a colour no caret to report, and a component reads that null before it tries
to restore one after a re-render. There is still one copy of the state and it is the renderer's —
the same editor `value` reads and writes, and the same one the caret and the selection highlight
are painted from — so a range set from script is a range the user can see, and a caret the user
moved is one script reads back. Which node has focus is mirrored into the renderer for the same
reason: nothing paints a caret, a highlight or a `:focus` rule until it is told.

One divergence, and it is HTML's own bit rather than the editor's: an anchor and a focus can say
forward or backward and have no third answer, so `"none"` — the direction a range set from script
has until something says otherwise — is kept beside the control and dropped the moment anything
moves the caret.

What is **not** here: the clipboard (`cut`/`copy`/`paste` and their `inputType`s), undo and redo,
IME composition (`compositionstart` and the rest — there is no IME path into this runtime, which is
why `InputEvent.isComposing` is always false), `getTargetRanges()` on a `beforeinput`, the
`selectionchange` event, implicit form submission on Enter, and the `change` event a text control
fires when its value is committed on blur. A framework that listens for `input` — React's
`onChange` is `input` — is unaffected by that last one.

### What is absent

Constraint validation (`validity`, `checkValidity`, `setCustomValidity`), `labels` and `files` are
absent rather than stubbed. Each is a surface of its own and each would be a wrong answer if
guessed at: there is no file picker behind an input in this runtime, and no validity model either.

## Pointer events

Input arrives as `pointerdown`, `pointermove`, `pointerup` and `pointercancel`, carrying
`pointerType` — `"mouse"`, `"touch"` or `"pen"` — a `pointerId`, `isPrimary`, and the `pressure`
the device measured. A touchscreen, a stylus and a precision touchpad are all pointing devices to
the platform underneath, so this is not a mobile feature: it is what a drawing surface reads to
vary a stroke's width, and it works on all six shipping desktop targets.

**A `MouseEvent` is still synthesised behind every pointer event**, in that order:
`pointerdown` then `mousedown`, `pointerup` then `mouseup` then `click`. That is what browsers do
and it is done here for the same reason — the installed base listens for mouse events. Every
component already running on Blitsen was written against `mousedown`/`click`, and it has to keep
working when the press came from a finger. Two rules keep the pair from becoming noise:

- **Only the primary pointer synthesises them.** A second finger does not fire a second `mousedown`
  at whatever it landed on.
- **A cancelled `pointerdown` suppresses them for the rest of that contact**, `click` included.
  This is how an application takes a gesture over: refuse the press, and the compatibility events —
  along with the focus change and the activation that are their default actions — do not happen.

**Every contact is its own pointer.** `pointerId` is stable for the life of one contact and is
never reused: the platform renumbers a finger once it has lifted, and a new contact has to be a new
pointer. Which buttons are held, and which node each of them went down on, is tracked per pointer,
so two fingers pressing two controls are two independent presses and lifting one does not cancel
the other's `click`.

**`setPointerCapture`/`releasePointerCapture`/`hasPointerCapture`** are implemented on `Element`.
Capture is *pending* until the next pointer event, exactly as the spec says: an element that
captures from its own `pointerdown` handler is not retroactively that event's target, and
`gotpointercapture` has not fired by the time the handler returns. From the next event on, every
event from that pointer is retargeted at the capturing element — including the synthesised mouse
events and the `click`, so a drag that ends outside its handle is still a click on the handle.
Capture is released implicitly when the contact ends, and immediately if the capturing element
leaves the document, which stops a re-render from swallowing the rest of a gesture.

`width` and `height` are 1, and `tiltX`/`tiltY`/`twist`/`tangentialPressure` are 0 unless a tablet
reported them: no platform underneath reports a touch ellipse, and a guessed one would be a
measurement this runtime never made.

### What is absent here, and why

- **`pointerover`/`pointerout`/`pointerenter`/`pointerleave`, and their mouse twins.** Crossing
  boundaries is a second piece of state — which element each pointer was last over — and neither
  the mouse nor the pointer half of it exists today. `:hover` is resolved by the renderer and is
  unaffected.
- **`TouchEvent`, `TouchList` and `event.touches`.** The older, touch-only interface. Pointer
  events supersede it and a library that wants it can build one from these; shipping both would be
  two sources of truth for one gesture.
- **`touch-action`.** The CSS property that tells the platform which gestures it may take over.
  There are no platform-taken gestures to declare (below), so the property would describe nothing.
- **Scroll and momentum from a touch drag.** A touchscreen has no wheel, so dragging a finger
  scrolls nothing: `wheel` and the keyboard are still the only things that scroll. This is a
  deliberate omission rather than an oversight — panning has to decide when a drag stops being a
  tap, whether that decision belongs here or in the renderer, and whether momentum is worth a
  per-frame animator — and none of that is settled. An application that wants a finger to scroll
  can do it today from `pointermove` and `element.scrollTop`. Tracked in #145.

## Canvas

`getContext("2d")` returns a real 2D context: paths, fills, strokes, gradients, patterns, images,
text, transforms, clipping and all 27 composite operations. Its contents are composited into the
same frame as the DOM, at the element's own paint position, so z-order, ancestor `overflow` and
`border-radius` apply to a canvas exactly as they apply to an image.

What is drawn is recorded as a display list rather than rasterised into a bitmap, and that is the
one structural difference from a browser worth knowing about. Painting a canvas costs no
rasterisation and no upload — the recorded commands are replayed into the frame the renderer was
already drawing — and a canvas scaled by CSS is drawn at the scaled size rather than sampled from
a smaller bitmap. Rasterisation happens only where the specification demands a readback:
`getImageData`, `toDataURL`, `toBlob`, and using one canvas as another's image source.

A canvas that is not in the document draws, reads back and encodes:
`document.createElement("canvas")`, draw, `toDataURL()` works without it ever being connected.
That is also what stands in for `OffscreenCanvas`, which is absent.

Canvas text is shaped from the same font collection the document is laid out with, so a family the
document registered with `@font-face` is available to `ctx.font` under its own name. The family
list is passed on as CSS — quoted names, fallback lists and the generic families are all
understood — and the default is the specification's `10px sans-serif`. `measureText` reports the
box that same shaping produced, ink extents included, so a measurement cannot disagree with what
`fillText` then draws.

### Where the canvas surface is narrower than its name

- **Shadows and `filter` are absent**, and absent means the property does not exist:
  `"shadowBlur" in ctx` is false. Both need a blur, and nothing in the paint pipeline under this
  runtime has one — the same reason `doctor` reports CSS `filter` as ignored rather than applied.
- **`clip("evenodd")` clips as if it were non-zero.** A clip is a layer in the recorded scene and
  the renderer's layer takes a path, not a fill rule. Everything else honours the rule it is
  given: `fill("evenodd")`, `isPointInPath(…, "evenodd")` and a `Path2D` with a hole in it are all
  correct — it is only the clip that cannot express it.
- **`ctx.font` resolves relative sizes against 16px**, not against the canvas element's own
  computed font. `16px`, `1.5rem` and `120%` all parse; the last two are 24px and 19.2px here
  rather than whatever the element inherited. Absolute units — `px`, `pt`, `pc`, `in`, `cm`, `mm`,
  `q` — and the `small`/`large` keywords are exact. Reading the element's computed font would be a
  forced style resolution on every assignment to `ctx.font`, which is a line inside draw loops.
- **`fillStyle` and `strokeStyle` parse hex, `rgb()`, `rgba()`, `hsl()`, `hwb()` and the CSS colour
  keywords.** The CSS Color 4 spaces — `lab()`, `oklch()`, `color()` — do not parse, and an
  unparseable colour is *ignored*, which is what the canvas specification says to do with one. The
  previous colour stays in effect; nothing throws.
- **A destructive composite operation inside a `clip()` erases within the clip, not across the
  canvas.** `copy`, `source-in`, `source-out`, `destination-in` and `destination-atop` clear the
  canvas wherever their source is absent, which is correct, and a canvas that uses one is
  composited as a group so it erases itself rather than the page behind it. Under an active clip
  the erasure is scoped to the clip's own layer, where a browser would clear the whole canvas.
- **`letterSpacing`, `wordSpacing`, `fontKerning`, `fontStretch`, `fontVariantCaps` and
  `textRendering` are absent**, and `direction: "inherit"` resolves to `ltr` rather than reading
  the element's own direction.
- **`toDataURL` and `toBlob` encode PNG and JPEG.** Any other type — `image/webp` among them —
  encodes PNG, which is what the specification says to do with a type an implementation does not
  support; the data URL's prefix and the blob's `type` say which format came back. The `quality`
  argument applies to JPEG, and anything outside 0–1 means the encoder's own default.
- **`ImageBitmap`, `createImageBitmap`, `OffscreenCanvas`, `captureStream` and
  `transferControlToOffscreen` are absent.** `drawImage` and `createPattern` take an `<img>` or
  another `<canvas>`, which is what a bitmap source is here.

## SVG

An `<svg>` subtree paints, and so does an SVG named by `<img src>` or by a CSS `background-image`
(issue #238). The element is parsed with usvg and painted through the same Vello scene the rest of
the frame is painted into, so a shape is a filled or stroked path rather than a rasterised image:
it stays sharp at any window scale, and a resize costs nothing but a repaint.

What that gets you, precisely:

| Paints | Does not |
| --- | --- |
| `path`, `rect`, `circle`, `ellipse`, `line`, `polygon`, `polyline`, `g`, `use`, `symbol`, `defs` | `foreignObject` |
| `viewBox`, `preserveAspectRatio`, `transform` on any element | SMIL animation — `animate`, `animateTransform`, `set` |
| `fill`, `stroke`, `stroke-width`, `stroke-linecap`/`linejoin`/`dasharray`, `fill-rule`, `paint-order` | `filter` and `mask` |
| `currentColor`, resolved from the CSS `color` the element inherits | `<pattern>` fills |
| `linearGradient` and `radialGradient`, including `stop-opacity` | A `clipPath` holding more than one path |
| `opacity`, `mix-blend-mode` and a single-path `clipPath` | |
| `<text>`, outlined through the host's fonts — with the caveat below | |

The element is sized like the replaced element it is: its `width`/`height` attributes, or author
CSS, which wins. A `viewBox`-only `<svg>` with no width, height or CSS box has nothing to size
itself from and lays out at zero — give it a box.

**`<text>` inside an SVG finds its fonts differently from HTML text, and can find none.** The two
go through different font discovery — usvg is given a database built by scanning well-known
directories, HTML text goes through the platform's own — and on a host where they disagree the SVG
text lays out, paints nothing, and takes nothing else with it. It happens on GitHub's Linux runner.
If a chart's axis labels matter, draw them as HTML beside the SVG rather than inside it, or check
them on the machines you ship to; gap G17 in [BLITZ-GAPS.md](BLITZ-GAPS.md) has the detail.

**One unsupported case is worse than a no-op and worth knowing about.** A `<pattern>` fill does not
merely fail to paint: the SVG renderer marks unsupported paints with a half-transparent red box
drawn at the *frame's* top-left corner rather than over the element, so a patterned shape anywhere
in the document leaves a red mark over whatever is in the corner. `doctor` reports it
(`HTML_SVG`), and gap G16 in [BLITZ-GAPS.md](BLITZ-GAPS.md) has the detail.

Mutating an SVG subtree from script works — set an attribute on a child and the frame follows,
which is what a charting library does — but the subtree is re-parsed rather than patched, so a
chart that rewrites its paths every frame pays a parse every frame. For per-frame drawing, use
`<canvas>` or `<blitsen-view>`.

## Intl

`Intl` is implemented (issue #237), natively, over CLDR through ICU4X and the platform's own
time-zone database. It is not the engine's: QuickJS-ng ships no ICU, and the formatters are the
bridge's, which is why they are in this document rather than in a note about the engine.

| Implemented | Absent |
| --- | --- |
| `Intl.NumberFormat` — decimal, percent, currency and compact notation | `formatToParts` and `formatRange`, on every formatter |
| `Intl.DateTimeFormat` — `dateStyle`/`timeStyle`, the component options, `hour12`/`hourCycle`, and named IANA `timeZone` values | `Intl.Segmenter` |
| `Intl.RelativeTimeFormat`, `Intl.PluralRules`, `Intl.Collator`, `Intl.ListFormat` | `Intl.DisplayNames`, `Intl.DurationFormat`, `Intl.supportedValuesOf` |
| `Number.prototype.toLocaleString`, `Date.prototype.toLocale*String`, `String.prototype.localeCompare`, all three over the formatters above | — |
| `Intl.getCanonicalLocales`, and `supportedLocalesOf` on each formatter | — |

Every CLDR locale is carried; there is no locale list to declare and nothing to configure. That is
a measured decision rather than a generous one — the whole of the data these formatters use is
about 3 MB of the export, which is less than a per-application slice of it would be worth in
build machinery. See [PRODUCT.md](PRODUCT.md) for what it did to the size budget.

Three things are worth knowing before you rely on the details:

- **A currency's fraction digits are the currency's.** `style: "currency"` formats to the minor
  units CLDR gives the code — two for `USD`, none for `JPY`, three for `KWD` — and
  `minimumFractionDigits`/`maximumFractionDigits` do not override that. `resolvedOptions()` reports
  the digits that were actually used, so the disagreement is detectable rather than silent.
- **`resolvedOptions()` reports what was honoured**, not what was asked for. An option this
  implementation does not act on is absent from the result rather than echoed back, because an
  echoed option is indistinguishable from an implemented one.
- **A time zone that is not in the database is refused**, with the name in the message, rather than
  silently becoming UTC. `Intl.DateTimeFormat().resolvedOptions().timeZone` and `os.locale()` both
  report the zone the host is actually in.

Values cross the native boundary as decimal text rather than as doubles, so what is rounded to a
currency's minor units is the number the application meant. Formatters are shared by their resolved
options: constructing the same `Intl.NumberFormat` inside a render loop is a lookup after the first
one.

## Storage

`localStorage` and `sessionStorage` exist, hold what is put in them, and **lose it when the
application exits**. There is no profile directory behind an exported application yet, so both are
one process's memory: `sessionStorage` is therefore exactly right, and `localStorage` is a session
store wearing a longer name.

It is implemented anyway because the absence is not survivable and the forgetfulness is. Libraries
read `localStorage` unguarded inside a render — shadcn's theme provider does it in a `useState`
initialiser — so an absent global takes the application down before first paint, while an empty one
degrades to the default theme. What must not happen is that the difference goes unnoticed, so
`doctor` reports every `localStorage.setItem` as `WEB_STORAGE_MEMORY`, on every build, for as long
as this is true. Keep anything that has to outlive the process in a file the application owns.
Real persistence is tracked separately.

`indexedDB` stays absent.

## Device identity

`navigator` answers three questions — `userAgent`, `platform`, and `language`/`languages` — and
nothing else. Those are facts about the machine the application is running on, and they are
answered for the same reason storage is: Svelte 5 reads `navigator.userAgent` while it hydrates,
without guarding it.

Everything else `navigator` normally carries is capability rather than identity — `clipboard`,
`geolocation`, `mediaDevices`, `serviceWorker`, `sendBeacon`, `permissions`, `onLine`,
`userAgentData` — and all of it stays absent, so a feature test selects a fallback instead of
calling something that cannot work. `screen`, `Notification` and `caches` are absent for the same
reason; the native modules cover what an application actually needs there.

The user-agent string names Blitsen (`Blitsen/<version> (Linux x86_64)`) instead of impersonating a
browser. An application that sniffs it deserves a true answer more than it deserves a code path
written for someone else's engine.

## Unreferenced files

Ingest walks the output directory from `index.html` and collects only what it can reach.
Whatever is left over is listed at the end of the build and dropped, because an unreferenced file
is pure export size. Keep some of it with a repeatable glob (`*` stops at `/`, `**` does not):

```sh
npx blitsen build dist --include 'assets/*.wasm' --include 'locales/**'
```

That is also the escape hatch for a file only a runtime-computed URL reaches.

## Where assets live

`--assets embedded` (the default) puts every asset inside the executable and unpacks them into a
private temporary directory at launch — one file to ship, nothing to install. `--assets
side-loaded` writes them to `<outfile>.assets/` beside the executable instead, which is the right
choice when assets must stay patchable after shipping or are large enough that carrying them in
the binary is wasteful. Each asset is content-hashed with SHA-256 either way, and repeating a
build from the same input directory, output path and working directory produces a byte-identical
executable.

## Diagnostic severity

Severity answers one question: **does the page survive?** It is not a measure of how far outside
the profile something is. An ignored paint property, a refused web font, an absent API a library
feature-detects — the page is still there, slightly plainer or on its fallback path. Those are
warnings, reported on every build, and they do not block one.

An error is reserved for the few constructs a page cannot come back from, and the scanner has to
be able to see that the construct is unconditional — a guarded one is not one of these:

| Error | Why the page does not come back from it |
| --- | --- |
| `WEB_FETCH` | A literal server-root URL at a `fetch` call site is not a capability test, so nothing selects a fallback. The data never arrives, and what renders from it never renders. |
| `HTML_SOURCE_ENTRY` | The document loads `.tsx`, `.jsx`, `.vue` or `.svelte` — source, which nothing here transpiles and no browser would run either. Blitsen was pointed at a source tree rather than at build output, and the fix is one command: `vite build`, then point it at `dist`. |

`ASSET_REMOTE_SCRIPT` used to be the third, on the reading that the loader refusing one remote
`src` left the document running no script at all — which is what stopped wordle-plus loading. The
loader now skips that one script, names it on stderr and runs every other script on the page, so
the reason for the severity is gone and grading it an error only blocks a build that works. What
keeps an exported application from phoning home is the runtime refusing to fetch the script, not
the severity of the rule that noticed it.

**Everything the scanner finds by naming an absent API is a warning**, including `WEB_XHR`,
`WEB_COOKIE`, `WEB_COMPONENTS`, `WEB_CANVAS`, `WEB_NAVIGATION`, `WEB_WORKER`, `WEB_GPU`,
`WEB_DIALOG`, `WEB_STYLE` and `WEB_STORAGE`. What takes a page down is an *unguarded* reference to
an absent global; a guarded one selects a fallback and the page carries on. This scan sees
references, not guards, and in real bundles those references are overwhelmingly guarded —
`typeof XMLHttpRequest<"u"`, `typeof ShadowRoot<"u"`, `"serviceWorker" in navigator`, a
`try`/`catch` around `document.cookie`. Unmodified third-party builds are the evidence:
shadcn-admin carried nineteen such findings and renders its entire admin dashboard, 364 elements
in 16 colours; vue3-realworld carried five and renders. Refusing those builds was the diagnostic
being confidently wrong, and it pointed users at an override that does not exist.

Detecting the guard was the alternative, and it was rejected rather than deferred: the guard is
arbitrary minified JavaScript and may be several frames away from the reference, so a detector
would work often enough to be trusted and then go quiet on the unguarded reference that does kill
a page. Trading a false error for a false silence is a bad trade. The finding is still reported —
every one of them, on every build — at the severity a static reference is actually worth. If your
application uses one of these APIs on a path that runs, the warning is the notice that it will
fail, and the render is what proves it either way.

## What v1 is not

The tier list is the thing the positioning rests on, so the line has to be drawn where the runtime
actually draws it rather than where the pitch would prefer.

| Not in v1 | What a build sees | Tracked as |
| --- | --- | --- |
| Canvas shadows and `filter` | The four `shadow*` properties and `ctx.filter` are **absent**, so `"shadowBlur" in ctx` is false and a feature test selects a fallback. Both need a blur, and the paint pipeline under this renderer has none — the same reason CSS `filter` is reported ignored | [#99](https://github.com/krazyjakee/blitsen/issues/99) |
| `OffscreenCanvas`, `ImageBitmap` | `WEB_CANVAS`, a warning. A canvas that is never in the document is the supported way to draw off-screen: `document.createElement("canvas")` draws, reads back and encodes without being connected | [#99](https://github.com/krazyjakee/blitsen/issues/99) |
| Advanced text input and IME | Text controls support keyboard editing, caret movement, click placement, drag selection and `beforeinput`/`input`; clipboard editing, undo/redo, composition/IME, `contenteditable`, `selectionchange`, target ranges and complex-script coverage remain incomplete | [#103](https://github.com/krazyjakee/blitsen/issues/103) |
| Accessibility | No accessibility tree is exported to the platform, so a screen reader finds nothing | [#102](https://github.com/krazyjakee/blitsen/issues/102) |
| WebGL, WebGPU, WebRTC | `WEB_GPU`, a warning. `<blitsen-view>` is the supported way to put GPU output on screen | — |

`<canvas>` used to be the first row of this table and a build-blocking `HTML_CANVAS` error, on the
reading that an element the renderer paints nothing inside has no degraded appearance to fall back
to. It paints now (issue #99), so the error is gone: an application that draws is no longer the
thing this profile refuses. What is still refused is a GPU context — `getContext("webgl")` answers
`null`, and `<blitsen-view>` is the supported way to put GPU output on screen.

## Capability tiers

**An unimplemented API is absent — the property does not exist — so feature detection works.**
Never a stub that resolves to nothing, and never a silent no-op. That includes the ones the
Phase 1 Bun host supplies itself: they are deleted while the runtime installs, because an API
that works today and vanishes at the Phase 2 engine swap is worse than one that was never there.

The tables below are **generated from the runtime source**. The surface is installed by
`crates/blitsen-host/src/dom_bridge.rs`, and `packages/blitsen/src/api-manifest.mjs` reads that
file: which globals it defines, what each class declares, and which globals it deletes. `blitsen
doctor` reports from the same manifest, and the native harness asserts every absent entry is
genuinely `undefined` in a real runtime — so the diagnostics, this document and the runtime
cannot drift apart. Regenerate with `bun run --cwd packages/blitsen api:sync`.

Blitsen makes no claim either way about the JavaScript host's own utilities — `URL`,
`URLSearchParams`, `TextEncoder`, `crypto`, `structuredClone`, `performance`, `queueMicrotask`,
`DOMException`, `console` — so they are not listed; the host below the DOM supplies them, which
under Phase 1 is Bun and under Phase 2 is `crates/blitsen-host/src/runtime_services/bootstrap.js`.
Two of them are narrower there than in a browser, because the missing part is a real absence
rather than a stub: `crypto` has `getRandomValues` and `randomUUID` and no `subtle`, and
`TextDecoder` decodes the UTF encodings and throws `RangeError` for the legacy single-byte labels.
`test:hosts` asserts the rest of that surface behaves identically on both hosts. Renderer capability (`CSS_*`, `HTML_*`) is not generated
either: no JavaScript declaration describes it, and it is evidenced by the S6 spike and the
determinism gate instead.

<!-- generated: api-manifest -->

| Group | Implemented | Absent |
| --- | --- | --- |
| WEB_DOM | `document`, `Document`, `Node`, `Element`, `NodeList`, `DOMTokenList`, `Attr`, `NamedNodeMap`, `CSSStyleDeclaration`, `MutationObserver`, `HTMLElement`, `HTMLIFrameElement`, `SVGElement`, `Text`, `Comment`, `DocumentFragment`, `HTMLLinkElement`, `HTMLTemplateElement`, `HTMLImageElement`, `Image`, `HTMLImageElement.src`, `HTMLImageElement.naturalWidth`, `HTMLImageElement.naturalHeight`, `HTMLImageElement.complete`, `HTMLImageElement.onload`, `HTMLImageElement.onerror`, `Element.querySelector`, `Element.querySelectorAll`, `Element.closest`, `Element.matches`, `Element.cloneNode`, `Element.contains`, `Element.children`, `Element.previousSibling`, `Element.lastChild`, `Element.parentElement`, `Element.dataset`, `Element.nodeValue`, `Element.before`, `Element.after`, `Element.getElementsByTagName`, `Element.outerHTML`, `Element.insertAdjacentHTML`, `Element.scrollIntoView`, `Element.getElementsByClassName`, `Element.firstElementChild`, `Element.lastElementChild`, `Element.nextElementSibling`, `Element.previousElementSibling`, `Element.childElementCount`, `Element.append`, `Element.prepend`, `Element.replaceChildren`, `Element.getAttributeNS`, `Element.setAttributeNS`, `Element.removeAttributeNS`, `Element.hasAttributes`, `Element.getAttributeNames`, `Element.toggleAttribute`, `Element.getClientRects`, `Element.getRootNode`, `Element.normalize`, `Element.attributes`, `Element.insertAdjacentElement`, `Element.innerText`, `Element.compareDocumentPosition`, `Element.offsetParent`, `Element.clientTop`, `Element.clientLeft`, `Element.hidden`, `Element.tabIndex`, `Element.title`, `Document.title`, `Document.dir`, `Document.getElementsByName`, `Document.elementFromPoint`, `Document.elementsFromPoint`, `Document.scrollingElement`, `Document.characterSet`, `Document.documentURI`, `Document.hasFocus`, `Document.adoptNode`, `HTMLLinkElement.relList`, `HTMLLinkElement.onload`, `HTMLLinkElement.onerror`, `HTMLTemplateElement.content`, `DOMTokenList.supports`, `Document.createElementNS`, `Document.createComment`, `Document.createDocumentFragment`, `Document.getElementsByTagName`, `Document.getElementsByClassName`, `Document.importNode`, `NodeList.item`, `NodeList.forEach` | `Element.attachShadow`, `Document.currentScript` |
| WEB_FORM_CONTROLS | `HTMLInputElement`, `HTMLTextAreaElement`, `HTMLSelectElement`, `HTMLOptionElement`, `HTMLButtonElement`, `HTMLFormElement`, `HTMLInputElement.value`, `HTMLInputElement.defaultValue`, `HTMLInputElement.checked`, `HTMLInputElement.defaultChecked`, `HTMLInputElement.type`, `HTMLInputElement.name`, `HTMLInputElement.disabled`, `HTMLInputElement.form`, `HTMLInputElement.select`, `HTMLInputElement.setSelectionRange`, `HTMLInputElement.selectionStart`, `HTMLInputElement.selectionEnd`, `HTMLInputElement.selectionDirection`, `HTMLTextAreaElement.value`, `HTMLTextAreaElement.defaultValue`, `HTMLTextAreaElement.select`, `HTMLTextAreaElement.setSelectionRange`, `HTMLTextAreaElement.selectionStart`, `HTMLTextAreaElement.selectionEnd`, `HTMLTextAreaElement.selectionDirection`, `HTMLSelectElement.options`, `HTMLSelectElement.selectedIndex`, `HTMLSelectElement.value`, `HTMLSelectElement.length`, `HTMLSelectElement.selectedOptions`, `HTMLSelectElement.multiple`, `HTMLOptionElement.value`, `HTMLOptionElement.text`, `HTMLOptionElement.selected`, `HTMLOptionElement.index`, `HTMLOptionElement.label`, `HTMLOptionElement.defaultSelected`, `HTMLButtonElement.value`, `HTMLButtonElement.type`, `HTMLFormElement.elements`, `HTMLFormElement.requestSubmit` | `HTMLInputElement.files`, `HTMLInputElement.labels`, `HTMLInputElement.validity`, `HTMLInputElement.checkValidity`, `HTMLSelectElement.add`, `HTMLFormElement.submit`, `HTMLFormElement.reset`, `HTMLFormElement.action`, `HTMLFormElement.method`, `HTMLFormElement.checkValidity` |
| WEB_EVENTS | `EventTarget`, `Event`, `CustomEvent`, `SubmitEvent`, `MouseEvent`, `KeyboardEvent`, `FocusEvent`, `InputEvent`, `PointerEvent`, `WheelEvent`, `addEventListener`, `removeEventListener`, `dispatchEvent`, `ErrorEvent`, `Element.setPointerCapture`, `Element.releasePointerCapture`, `Element.hasPointerCapture` | — |
| WEB_SCROLL | `scrollTo`, `scrollBy`, `scroll`, `scrollX`, `scrollY`, `pageXOffset`, `pageYOffset` | — |
| WEB_SELECTION | `getSelection`, `Range`, `Selection`, `CaretPosition`, `Document.createRange`, `Document.getSelection`, `Document.caretRangeFromPoint`, `Document.caretPositionFromPoint`, `Range.setStart`, `Range.setEnd`, `Range.setStartBefore`, `Range.setStartAfter`, `Range.setEndBefore`, `Range.setEndAfter`, `Range.selectNode`, `Range.selectNodeContents`, `Range.collapse`, `Range.cloneRange`, `Range.startContainer`, `Range.startOffset`, `Range.endContainer`, `Range.endOffset`, `Range.collapsed`, `Range.commonAncestorContainer`, `Range.comparePoint`, `Range.compareBoundaryPoints`, `Range.intersectsNode`, `Range.isPointInRange`, `Range.toString`, `Range.getClientRects`, `Range.getBoundingClientRect`, `Selection.anchorNode`, `Selection.anchorOffset`, `Selection.focusNode`, `Selection.focusOffset`, `Selection.isCollapsed`, `Selection.rangeCount`, `Selection.type`, `Selection.direction`, `Selection.getRangeAt`, `Selection.addRange`, `Selection.removeAllRanges`, `Selection.setBaseAndExtent`, `Selection.collapse`, `Selection.extend`, `Selection.selectAllChildren`, `Selection.containsNode`, `Selection.toString`, `CaretPosition.offsetNode`, `CaretPosition.offset`, `CaretPosition.getClientRect` | `Range.deleteContents`, `Range.extractContents`, `Range.cloneContents`, `Range.insertNode`, `Range.surroundContents` |
| WEB_SCHEDULING | `requestAnimationFrame`, `cancelAnimationFrame`, `setTimeout`, `clearTimeout`, `setInterval`, `clearInterval` | `requestIdleCallback`, `cancelIdleCallback` |
| WEB_NETWORK | `fetch`, `Headers`, `Request`, `Response`, `Blob`, `AbortController`, `AbortSignal` | — |
| WEB_URL | `URL`, `URLSearchParams` | `URL.createObjectURL`, `URL.revokeObjectURL` |
| WEB_ROUTING | `window`, `self`, `location`, `history`, `Location`, `History`, `PopStateEvent`, `HashChangeEvent` | — |
| WEB_VIEWPORT | `BlitsenViewElement`, `BlitsenViewSurface` | — |
| WEB_STORAGE | `Storage`, `localStorage`, `sessionStorage` | `indexedDB` |
| WEB_WORKER | `Worker`, `Worker.postMessage`, `Worker.terminate` | `SharedWorker`, `ServiceWorker`, `ServiceWorkerContainer` |
| WEB_MESSAGING | `MessageChannel`, `MessagePort`, `structuredClone`, `postMessage`, `MessagePort.postMessage`, `MessagePort.start`, `MessagePort.close` | `BroadcastChannel` |
| WEB_SOCKET | `WebSocket`, `MessageEvent`, `CloseEvent`, `EventSource`, `WebSocket.url`, `WebSocket.readyState`, `WebSocket.protocol`, `WebSocket.extensions`, `WebSocket.bufferedAmount`, `WebSocket.binaryType`, `WebSocket.send`, `WebSocket.close`, `EventSource.url`, `EventSource.readyState`, `EventSource.withCredentials`, `EventSource.close` | — |
| WEB_INTL | `Intl`, `Intl.NumberFormat`, `Intl.DateTimeFormat`, `Intl.RelativeTimeFormat`, `Intl.PluralRules`, `Intl.Collator`, `Intl.ListFormat`, `Intl.getCanonicalLocales`, `Intl.NumberFormat.format`, `Intl.NumberFormat.resolvedOptions`, `Intl.DateTimeFormat.format`, `Intl.DateTimeFormat.resolvedOptions`, `Intl.Collator.compare`, `Intl.PluralRules.select`, `Intl.ListFormat.format`, `Intl.RelativeTimeFormat.format` | `Intl.NumberFormat.formatToParts`, `Intl.DateTimeFormat.formatToParts`, `Intl.DateTimeFormat.formatRange`, `Intl.Segmenter`, `Intl.DisplayNames`, `Intl.DurationFormat`, `Intl.supportedValuesOf` |
| WEB_XHR | — | `XMLHttpRequest` |
| WEB_STREAM | — | `ReadableStream`, `WritableStream`, `TransformStream`, `Response.body`, `Response.clone` |
| WEB_FORM | — | `FormData`, `File`, `FileReader` |
| WEB_CANVAS | `HTMLCanvasElement`, `CanvasRenderingContext2D`, `ImageData`, `Path2D`, `CanvasGradient`, `CanvasPattern`, `TextMetrics`, `DOMMatrix`, `HTMLCanvasElement.width`, `HTMLCanvasElement.height`, `HTMLCanvasElement.getContext`, `HTMLCanvasElement.toDataURL`, `HTMLCanvasElement.toBlob`, `CanvasRenderingContext2D.canvas`, `CanvasRenderingContext2D.save`, `CanvasRenderingContext2D.restore`, `CanvasRenderingContext2D.reset`, `CanvasRenderingContext2D.scale`, `CanvasRenderingContext2D.rotate`, `CanvasRenderingContext2D.translate`, `CanvasRenderingContext2D.transform`, `CanvasRenderingContext2D.setTransform`, `CanvasRenderingContext2D.resetTransform`, `CanvasRenderingContext2D.getTransform`, `CanvasRenderingContext2D.globalAlpha`, `CanvasRenderingContext2D.globalCompositeOperation`, `CanvasRenderingContext2D.fillStyle`, `CanvasRenderingContext2D.strokeStyle`, `CanvasRenderingContext2D.lineWidth`, `CanvasRenderingContext2D.lineCap`, `CanvasRenderingContext2D.lineJoin`, `CanvasRenderingContext2D.miterLimit`, `CanvasRenderingContext2D.setLineDash`, `CanvasRenderingContext2D.getLineDash`, `CanvasRenderingContext2D.lineDashOffset`, `CanvasRenderingContext2D.font`, `CanvasRenderingContext2D.textAlign`, `CanvasRenderingContext2D.textBaseline`, `CanvasRenderingContext2D.direction`, `CanvasRenderingContext2D.imageSmoothingEnabled`, `CanvasRenderingContext2D.imageSmoothingQuality`, `CanvasRenderingContext2D.beginPath`, `CanvasRenderingContext2D.closePath`, `CanvasRenderingContext2D.moveTo`, `CanvasRenderingContext2D.lineTo`, `CanvasRenderingContext2D.quadraticCurveTo`, `CanvasRenderingContext2D.bezierCurveTo`, `CanvasRenderingContext2D.arc`, `CanvasRenderingContext2D.arcTo`, `CanvasRenderingContext2D.ellipse`, `CanvasRenderingContext2D.rect`, `CanvasRenderingContext2D.roundRect`, `CanvasRenderingContext2D.fill`, `CanvasRenderingContext2D.stroke`, `CanvasRenderingContext2D.clip`, `CanvasRenderingContext2D.isPointInPath`, `CanvasRenderingContext2D.isPointInStroke`, `CanvasRenderingContext2D.fillRect`, `CanvasRenderingContext2D.strokeRect`, `CanvasRenderingContext2D.clearRect`, `CanvasRenderingContext2D.fillText`, `CanvasRenderingContext2D.strokeText`, `CanvasRenderingContext2D.measureText`, `CanvasRenderingContext2D.drawImage`, `CanvasRenderingContext2D.createLinearGradient`, `CanvasRenderingContext2D.createRadialGradient`, `CanvasRenderingContext2D.createConicGradient`, `CanvasRenderingContext2D.createPattern`, `CanvasRenderingContext2D.createImageData`, `CanvasRenderingContext2D.getImageData`, `CanvasRenderingContext2D.putImageData`, `Path2D.moveTo`, `Path2D.lineTo`, `Path2D.bezierCurveTo`, `Path2D.quadraticCurveTo`, `Path2D.arc`, `Path2D.arcTo`, `Path2D.ellipse`, `Path2D.rect`, `Path2D.roundRect`, `Path2D.closePath`, `Path2D.addPath`, `CanvasGradient.addColorStop`, `CanvasPattern.setTransform` | `OffscreenCanvas`, `OffscreenCanvasRenderingContext2D`, `ImageBitmap`, `createImageBitmap`, `HTMLCanvasElement.captureStream`, `HTMLCanvasElement.transferControlToOffscreen`, `CanvasRenderingContext2D.shadowBlur`, `CanvasRenderingContext2D.shadowColor`, `CanvasRenderingContext2D.shadowOffsetX`, `CanvasRenderingContext2D.shadowOffsetY`, `CanvasRenderingContext2D.filter`, `CanvasRenderingContext2D.letterSpacing`, `CanvasRenderingContext2D.wordSpacing`, `CanvasRenderingContext2D.fontKerning`, `CanvasRenderingContext2D.getContextAttributes`, `CanvasRenderingContext2D.drawFocusIfNeeded` |
| WEB_GPU | — | `WebGLRenderingContext`, `WebGL2RenderingContext`, `GPUCanvasContext` |
| WEB_MEDIA | `Audio`, `AudioContext`, `AudioNode`, `AudioParam`, `AudioBuffer`, `AudioBufferSourceNode`, `AudioDestinationNode`, `GainNode`, `StereoPannerNode`, `HTMLAudioElement`, `AudioContext.decodeAudioData`, `AudioContext.createGain`, `AudioContext.createStereoPanner`, `AudioContext.createBufferSource`, `AudioContext.destination`, `AudioContext.currentTime`, `AudioContext.sampleRate`, `AudioContext.resume`, `AudioContext.suspend`, `AudioContext.close` | `webkitAudioContext`, `HTMLMediaElement` |
| WEB_DIALOG | — | `alert`, `confirm`, `prompt`, `print` |
| WEB_NAVIGATION | `stop` | `open`, `close`, `navigation`, `document.write`, `document.writeln`, `document.open`, `document.close`, `location.assign`, `location.replace`, `location.reload`, `location.ancestorOrigins` |
| WEB_COOKIE | — | `document.cookie`, `cookieStore`, `Headers.getSetCookie` |
| WEB_DEVICE | `Navigator`, `navigator`, `navigator.userAgent`, `navigator.platform`, `navigator.language` | `screen`, `Notification`, `caches` |
| WEB_OBSERVER | `ResizeObserver` | `IntersectionObserver`, `PerformanceObserver` |
| WEB_STYLE | `getComputedStyle`, `matchMedia`, `MediaQueryList`, `MediaQueryListEvent`, `CSS`, `CSSStyleSheet`, `StyleSheetList`, `CSSRule`, `CSSRuleList`, `HTMLStyleElement`, `document.styleSheets`, `HTMLStyleElement.sheet`, `HTMLLinkElement.sheet`, `CSSStyleSheet.cssRules`, `CSSStyleSheet.insertRule`, `CSSStyleSheet.deleteRule`, `CSSStyleSheet.ownerNode`, `CSSStyleSheet.href`, `CSSStyleSheet.title`, `CSSRule.cssText`, `CSSRule.parentStyleSheet` | `CSSStyleRule`, `CSSKeyframesRule`, `CSSKeyframeRule`, `CSSMediaRule`, `document.adoptedStyleSheets`, `CSSStyleSheet.disabled`, `CSSStyleSheet.replaceSync`, `CSSStyleSheet.replace`, `CSSRule.style`, `CSSRule.selectorText`, `CSSRule.type` |
| WEB_COMPONENTS | `DOMParser` | `customElements`, `ShadowRoot` |
| WEB_WASM | — | `WebAssembly` |

| Diagnostic | Severity | Reported as |
| --- | --- | --- |
| `WEB_FETCH` | error | fetch names a path this application does not ship, and there is no server behind it. |
| `WEB_STORAGE_MEMORY` | warning | localStorage is in memory only: what it stores is gone when the application exits. |
| `WEB_DOM` | warning | This DOM method is not implemented. |
| `WEB_FORM_CONTROLS` | warning | This form-control API is not implemented. |
| `WEB_SELECTION` | warning | This part of the range API is not implemented; the boundary, text and geometry reads are. |
| `WEB_SCHEDULING` | warning | Idle-callback scheduling is not implemented. |
| `WEB_URL` | warning | Object URLs are not implemented; URL and URLSearchParams are. |
| `WEB_STORAGE` | warning | IndexedDB is not implemented. |
| `WEB_WORKER` | warning | Shared and service workers are not implemented; dedicated Worker is. |
| `WEB_MESSAGING` | warning | BroadcastChannel is not implemented; MessageChannel and Worker are. |
| `WEB_INTL` | warning | This part of Intl is not implemented; the formatters are. |
| `WEB_XHR` | warning | XMLHttpRequest is not implemented. |
| `WEB_STREAM` | warning | Streaming bodies are not implemented; a response is buffered whole. |
| `WEB_FORM` | warning | Multipart form bodies and file objects are not implemented. |
| `WEB_CANVAS` | warning | This canvas API is not implemented; the 2D context is. |
| `WEB_GPU` | warning | WebGL and WebGPU are not implemented. |
| `WEB_MEDIA` | warning | This media API is not implemented; Web Audio and <audio> are. |
| `WEB_DIALOG` | warning | Modal browser dialogs are not implemented. |
| `WEB_NAVIGATION` | warning | Document navigation is deliberately absent; there is no page to leave. |
| `WEB_COOKIE` | warning | There is no origin and no cookie jar behind an exported application. |
| `WEB_DEVICE` | warning | This device API is not implemented. |
| `WEB_OBSERVER` | warning | This observer is not implemented; only ResizeObserver is. |
| `WEB_STYLE` | warning | This part of CSSOM is not implemented; a sheet's rules are its source text. |
| `WEB_COMPONENTS` | warning | Custom elements and shadow DOM are not implemented; DOMParser is. |
| `WEB_WASM` | warning | WebAssembly is not implemented by the JavaScript engine Blitsen hosts. |
| `CSS_TRANSITION` | warning | A property named by `transition` keeps its pre-stylesheet value (Blitz bug 689). |
| `CSS_FIXED` | warning | Fixed and sticky boxes resolve against the root box, not the viewport (Blitz bug 690). |
| `CSS_EFFECT` | warning | This paint effect is ignored rather than applied. |
| `HTML_SOURCE_ENTRY` | error | This document loads source, not built output; nothing in Blitsen transpiles it. |
| `HTML_MEDIA` | warning | Video and text tracks are not implemented; <audio> is. |
| `HTML_SVG` | warning | This SVG feature does not paint; shapes, paths, text, gradients and clipPath do. |
| `ASSET_REMOTE_SCRIPT` | warning | A remote <script src> is not fetched; it is skipped and the rest of the page runs. |
| `ASSET_REMOTE` | warning | A remote asset is not part of a self-contained export; the request is answered with nothing. |

<!-- /generated -->

The scanner cannot prove visual equivalence or determine that an unsupported reference is dead
code. Treat a zero-error report as the build-time gate and retain visual/interaction acceptance tests
for the application itself. See the earlier [S6 renderer evidence](../spikes/s6/README.md) for why
this boundary exists.

## Native modules

The profile above is what a web application may already assume. The `native:` modules are the
other direction: capability the web has no spelling for, imported under a name that makes the
non-portability obvious at the import site.

```js
import app from "blitsen/app";
import clipboard from "blitsen/clipboard";
```

**`native:` is additive, never a superset** (TECH.md §9). Under the Phase 1 host, anything Node
already names keeps its Node name — the command line is `process.argv`, the executable is `process.execPath`, stopping is
`process.exit`, CPU and platform facts are `node:os`, and files are `node:fs`. So there is no
`app.argv` and no `app.quit`: a `native:` member exists only where neither Node nor the web has a
word for the thing.

**Absence is the API.** Outside the runtime — a browser tab, a plain Node script — every access on
these modules throws, because importing them there is a mistake. Inside it, a capability this
build does not implement is genuinely `undefined`, so feature detection works and reads the same
as it does for the web surface:

```js
if (app.requestSingleInstanceLock && !app.requestSingleInstanceLock("My App", relaunchedWith)) {
  process.exit(0);
}
```

The tables are generated from the same runtime source as the tiers above, by the same reader.

### Node compatibility in the shipped runtime

Phase 1 ran inside Bun, so `process`, `node:os` and `node:fs` came free with the host. **The Phase 2
runtime implements none of them, and this is a decision rather than a gap** (issue #87).

Blitsen hosts its own JavaScript engine and supplies only what the DOM and the application actually
rely on: timers, a microtask checkpoint, `performance`, `console`, `reportError`, `DOMException`,
`crypto`, `TextEncoder`/`TextDecoder`, and the web surface in the tables above. Implementing Node's module surface on top of that would
mean reimplementing a large, under-specified API with no conformance corpus, to serve applications
whose input is by definition browser-targeted static output — and every megabyte of it would ship
in every export, which is what Phase 2 exists to stop.

What replaces it:

| Phase 1 | Phase 2 |
| --- | --- |
| `process.argv` | `app.secondInstance` invocations, or the platform's own launch arguments |
| `process.exit` | closing the window |
| `node:os` facts | `blitsen/os` |
| `node:fs` under the app directory | `blitsen/app` directories, and the application's own bundled files |
| `import("node:anything")` | refused at resolution, naming the alternative |

That refusal is the point of listing this here rather than in a release note: an import of a
builtin fails with a message that says the runtime implements no `node:fs` and points at
`blitsen/*`, so the absence is detectable and attributable rather than a blank window
(structural constraint 4). The paragraph above about Node keeping its Node names describes the
Phase 1 host; where Phase 2 has no Node name to keep, the `blitsen/*` module is the whole API.

### TypeScript

The `blitsen` package carries the definitions, so **editor completion works without the runtime
being loadable in a browser context** — the types resolve from `node_modules`, not from a running
application. Extend the published `tsconfig` fragment:

```json
{ "extends": "blitsen/tsconfig.json", "include": ["src"] }
```

It sets the language level the runtime actually runs, resolves the `blitsen/*` subpaths through
package exports, and adds `blitsen/dom` — which is what declares `<blitsen-view>` and its surface,
including the tag-name map and the JSX namespace, so `document.createElement("blitsen-view")` types
as itself rather than as `HTMLElement`. If you would rather not extend it, reference the DOM types
once anywhere in the project:

```ts
/// <reference types="blitsen/dom" />
```

Each `blitsen/<module>` subpath has **its own declaration file**, so importing `blitsen/app` offers
the app module's members and not the clipboard's. Every member is optional, because a capability the
running version does not implement is `undefined` — which means TypeScript will not let you call one
without the feature detection above, and that is the point.

**The definitions cannot promise an API that does not exist.** They are checked against the
generated manifest in both directions: a declared member the runtime does not install, and an
installed member the definitions do not declare, are each a build failure. `bun run test:types`
typechecks a fixture against the package as it will be published — one file that must compile, one
whose every line must be rejected.

What types do *not* do is describe the absent half of the web surface. `lib.dom.d.ts` will still
offer `IndexedDB` and `HTMLCanvasElement`, because a package cannot remove a global from an ambient
lib. The capability tiers above are the list, and `blitsen doctor` is the check.

<!-- generated: native-modules -->

| Module | Implemented | Absent |
| --- | --- | --- |
| `blitsen/app` | `dataDir`, `cacheDir`, `configDir`, `requestSingleInstanceLock`, `relaunch` | `onQuitRequest`, `onSuspend`, `onResume`, `registerProtocol`, `registerFileAssociation` |
| `blitsen/window` | `setSize`, `setFullscreen`, `isFullscreen`, `setDecorations`, `isDecorated`, `setAlwaysOnTop`, `setCursor`, `setCursorVisible`, `setCursorGrab`, `monitors` | `create`, `setTransparent`, `isAlwaysOnTop` |
| `blitsen/dialog` | `openFile`, `openFiles`, `saveFile`, `openFolder`, `openFolders`, `message` | — |
| `blitsen/clipboard` | `readText`, `readHtml`, `readImage`, `writeText`, `writeHtml`, `writeImage`, `clear` | `readMime`, `writeMime` |
| `blitsen/os` | `cpu`, `memory`, `storage`, `host`, `locale` | `displays`, `battery`, `idleTime` |

| Absent member | Why |
| --- | --- |
| `app.onQuitRequest` | A close request is a window event, and windows are issue #77's to expose; delivering one from here would mean a second, competing event loop. |
| `app.onSuspend` | Linux has no process-level suspend notification to report. The desktop portals that come closest describe the session, not this application. |
| `app.onResume` | The counterpart of `onSuspend`, absent for the same reason. |
| `app.registerProtocol` | Registering `myapp://` on Linux means installing a `.desktop` entry that names the executable, which is what `blitsen build` already writes. A running process editing that entry would fight its own packaging. The activation itself arrives: the desktop launches the handler with the URL in `argv`, and the single-instance lock hands that to the instance already running. |
| `app.registerFileAssociation` | The same `.desktop` entry, with `MimeType` instead of a scheme. |
| `window.create` | A second window needs the shared-versus-isolated JavaScript context question answered first: whether two windows see one `document` and one module graph or two decides what `create` even returns, and it cannot be settled by implementing it. The window this run already opened is what the rest of this module operates on. |
| `window.setTransparent` | Transparency is chosen when a window is created — winit's own setter does nothing on X11 after that — so honouring it would mean replacing the window, which is `create`. Run `blitsen` against a directory whose window should be transparent and the attribute belongs on that window, not on a call. |
| `window.isAlwaysOnTop` | winit sets the window level and cannot read it back, and the window manager may change it without telling the application. Remembering what was last set would be a second source of truth that quietly goes stale. |
| `clipboard.readMime` | `arboard` reads the flavours above and no others. Arbitrary MIME needs a different mechanism on each platform — X11 selection targets, `wl_data_offer`, `NSPasteboardType`, a registered Windows format — and no part of that is shared. |
| `clipboard.writeMime` | The counterpart of `readMime`, absent for the same reason. |
| `os.displays` | The monitors are `window.monitors()`, which already reports each one's size, position and scale factor. A second list here could disagree with that one. |
| `os.battery` | Nothing behind this module reports power. The processor, the memory and the volumes come from one library that implements all three per platform; the battery is a fourth source on each — UPower's D-Bus service, IOKit, `GetSystemPowerStatus` — and a desktop with no battery has to read as *absent* rather than as an empty reading, which is a distinction only the real source can make. |
| `os.idleTime` | Seconds since the last input is a different mechanism on every platform, and Wayland has no answer at all for a client that is not focused — the idle-notify protocol reports crossing a threshold the compositor was asked about, not a duration. Reporting zero on the sessions that cannot answer would be indistinguishable from a machine in use. |

<!-- /generated -->

### Platform differences

Where a member exists, it means the same thing everywhere. What differs is underneath it.

| Member | What differs |
| --- | --- |
| `app.dataDir`, `app.cacheDir`, `app.configDir` | The application passes its own name, because the runtime does not know one: during development the executable is the host runtime, and a window title is not an identity. The name must be a single path segment. Linux answers with `$XDG_DATA_HOME`/`$XDG_CACHE_HOME`/`$XDG_CONFIG_HOME` and their `~/.local/share`, `~/.cache`, `~/.config` defaults; macOS with `~/Library/Application Support` and `~/Library/Caches`, so data and config are the same directory; Windows with `%APPDATA%` and `%LOCALAPPDATA%`. The directory is returned, never created — making it is `node:fs`. |
| `app.requestSingleInstanceLock` | Unix only, and absent on Windows rather than approximated: the lock is a Unix domain socket in `$XDG_RUNTIME_DIR`, which is both the claim and the channel the second invocation's `argv` and `cwd` arrive on. Windows wants a named mutex and a named pipe, which is a different design rather than this one with the socket swapped. A socket left behind by a process that crashed is detected and taken over. Second invocations are delivered on the frame turn, alongside `fetch` completions, so an application is never re-entered part-way through a frame. |
| `app.relaunch` | Spawns a copy of this process with the same arguments, environment and working directory, and drops the single-instance lock so the successor can take it. It does not stop this process: that is `process.exit`, and only the application knows what it still has to flush. |
| `clipboard.*` | On X11 and Wayland the process that copied is the one that serves the selection, so **what an exported Blitsen application copies disappears when it exits**, unless the desktop runs a clipboard manager that takes a copy. macOS and Windows hand the data to the system and it survives. `writeHtml` also stores the plain text an application that cannot read HTML will paste instead. Images cross as 8-bit RGBA, `{ width, height, data }`, and are carried as PNG on Linux, `CF_DIB` on Windows and an `NSImage` on macOS — a decoded image is not guaranteed to be byte-identical to the one that was copied. A read finds `null` where the clipboard holds nothing in that flavour; a clipboard the session does not offer at all — a headless process — throws instead, because that is an environment refusing rather than an empty clipboard. |
| `window.*` | Operates on the window the run already opened, and is available from the `load` event onwards — document scripts run before the window exists, and a call before then says so rather than doing nothing. `setAlwaysOnTop` reaches the X11 window manager and Windows and macOS; **Wayland has no protocol for stacking a window above others, so the call is accepted and has no effect there**. `setCursorGrab("locked")` is X11-unsupported and `"confined"` is macOS-unsupported; both throw naming the platform rather than silently degrading. `setSize` asks, it does not assert: the size that arrives is whatever the window manager granted, and it is reported by the `resize` event and `innerWidth`/`innerHeight` like any other resize. |
| `dialog.*` | Linux and the BSDs only, through the XDG desktop portal, and absent on macOS and Windows rather than approximated: those platforms require a file dialog on the main thread, which is the thread this design deliberately leaves free to keep painting. Every dialog is modal to the application window and needs one, so these are available from the `load` event onwards like `window.*`. Each returns a **Promise**, and the frame loop keeps turning while the dialog is up — `requestAnimationFrame` still fires, the window still paints, and the answer is delivered on a frame turn alongside `fetch` completions rather than part-way through one. Blocking instead would stop the application repainting for as long as the dialog was open, which X11 and Wayland compositors treat as a hung client. A dismissed file dialog answers `null`: the portal cannot distinguish Cancel from choosing nothing. Paths are real filesystem paths, not `File` objects. Where no portal is running, the desktop's `zenity` is used instead. |
