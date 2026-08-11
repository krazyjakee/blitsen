# v0 compatibility profile

Blitsen v0 accepts built static applications that stay within the surface below. The profile is
deliberately narrower than “works in a browser”: it describes what the current runtime and Blitz
renderer can support consistently enough to make an adoption claim.

Run the check against build output, not source:

```sh
npx blitsen doctor dist
npx blitsen doctor dist --json
```

`doctor` exits non-zero for profile errors. Warnings identify absent APIs a library normally
feature-detects, and renderer features that are not yet portable to the production-shaped Phase 2
host. A build repeats every diagnostic and **fails on any error** — an export that cannot run is
not worth producing — while warnings let feature-detected fallback paths through. The scan is
static and conservative: it finds references, not only executed paths. Every rule it applies to
JavaScript comes from the [generated manifest](#capability-tiers) below.

## Strict v0 surface

| Area | In profile |
| --- | --- |
| Application shape | One built `index.html` plus the local files reachable from it; root-relative HTML/CSS asset URLs are normalized while ingesting without changing `dist` |
| JavaScript | ES modules already emitted by the application's bundler |
| Framework DOM | Stable node identity, standard node type/name/owner fields, `MutationObserver`, creation/insertion/removal, text and attributes |
| Selection and collections | `querySelector`, `querySelectorAll`, `getElementById`, static `NodeList`, `classList` |
| Events | Capture/target/bubble listeners, click, mouse, wheel, keyboard, focus, resize and lifecycle events |
| Scheduling | `requestAnimationFrame`, timers and microtasks |
| Networking | `fetch`, `Headers`, `Request`, `Response`, `Blob`, `AbortController` over `http`/`https`, with buffered bodies |
| Routing | In-memory `history` and `location`, `popstate` and `hashchange` |
| CSS | Static block, flex and grid layout; bounded absolute positioning; spacing, borders, backgrounds, colors and system typography |
| Subresources | `<img>` and CSS `background-image` (PNG, JPEG, GIF, WebP), and `@font-face` web fonts (WOFF2, WOFF, TTF, OTF), loaded from local files; SVG images, `<audio>` and `<video>` are not |

The M3b acceptance app intentionally uses the normal Vite default output, including
root-relative `/assets/...` references and Vite's module-preload bootstrap. It contains no
Blitsen imports or runtime branches.

## Asset URLs

There is no web server behind an exported application, so a URL that assumes a server root has to
be resolved at build time. **Blitsen rewrites server-root URLs while ingesting, in its own staging
copy — your `dist` directory is never modified.**

| You wrote | Blitsen does |
| --- | --- |
| `href="./assets/app.css"` | Nothing; relative URLs already work. |
| `src="/assets/app.js"` (default `base`) | Rewrites to the equivalent document-relative path. |
| `src="/app/assets/app.js"` (custom `base: "/app/"`) | Drops the base prefix that does not exist in the output, then rewrites. |
| `url("/assets/hero.png")` in CSS, and `@import` | Same rewrite, applied transitively. |
| `<a href="/settings">` | Nothing; anchors are navigation, not subresources. |
| `src="https://cdn…"` or `//cdn…` | Fails the build. A self-contained export cannot fetch it. |

**Only HTML and CSS are rewritten.** JavaScript is left byte-identical, because a path assembled
at runtime cannot be safely edited by a regular expression. In practice:

- `new URL('./x.png', import.meta.url)` and a relative `import()` **work** — the export preserves
  your directory layout, so relative resolution lands on the same file it did on a server.
- `new URL('/assets/x.png', …)` and any specifier built from a variable or template literal **do
  not work**, and are not diagnosed. Configure your bundler with a relative base (Vite:
  `base: './'`) if your application computes asset URLs from a server root.
- `fetch('/data.json')` does not work either — see [Networking](#networking) — but a literal URL
  is diagnosed as an error.

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
| `fetch("/api/data")` | Fails. There is no server behind the document address; `doctor` reports it as an error. |
| `new Request(…)`, `new Headers(…)`, `new Response(…)` | Full subset above, including `AbortSignal`. |
| `response.text/json/arrayBuffer/blob()` | Supported; a body is readable once. |
| `response.body`, `response.clone()`, `FormData` bodies | Absent. |

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
| `popstate` and `hashchange` on `window`, `PopStateEvent`, `HashChangeEvent` | `navigation` (the Navigation API), `document.location` |

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

## Capability tiers

**An unimplemented API is absent — the property does not exist — so feature detection works.**
Never a stub that resolves to nothing, and never a silent no-op. That includes the ones the
Phase 1 Bun host supplies itself: they are deleted while the runtime installs, because an API
that works today and vanishes at the Phase 2 engine swap is worse than one that was never there.

The tables below are **generated from the runtime source**. The surface is installed by
`crates/blitsen-node/src/dom_bridge.rs`, and `packages/blitsen/src/api-manifest.mjs` reads that
file: which globals it defines, what each class declares, and which globals it deletes. `blitsen
doctor` reports from the same manifest, and the native harness asserts every absent entry is
genuinely `undefined` in a real runtime — so the diagnostics, this document and the runtime
cannot drift apart. Regenerate with `bun run --cwd packages/blitsen api:sync`.

Blitsen makes no claim either way about the JavaScript host's own utilities — `URL`,
`URLSearchParams`, `TextEncoder`, `crypto`, `structuredClone`, `performance`, `queueMicrotask`,
`DOMException`, `console` — so they are not listed; the Phase 2 engine has to supply them. Renderer capability (`CSS_*`, `HTML_*`) is not generated
either: no JavaScript declaration describes it, and it is evidenced by the S6 spike and the
determinism gate instead.

<!-- generated: api-manifest -->

| Group | Implemented | Absent |
| --- | --- | --- |
| WEB_DOM | `document`, `Document`, `Node`, `Element`, `NodeList`, `DOMTokenList`, `CSSStyleDeclaration`, `MutationObserver`, `HTMLElement`, `HTMLIFrameElement`, `SVGElement` | `Element.querySelector`, `Element.querySelectorAll`, `Element.closest`, `Element.matches`, `Element.cloneNode`, `Element.contains`, `Element.children`, `Element.previousSibling`, `Element.lastChild`, `Element.parentElement`, `Element.dataset`, `Element.outerHTML`, `Element.insertAdjacentHTML`, `Element.attachShadow`, `Element.scrollIntoView`, `Document.createElementNS`, `Document.createDocumentFragment` |
| WEB_EVENTS | `EventTarget`, `Event`, `CustomEvent`, `MouseEvent`, `KeyboardEvent`, `addEventListener`, `removeEventListener`, `dispatchEvent` | — |
| WEB_SCHEDULING | `requestAnimationFrame`, `cancelAnimationFrame`, `setTimeout`, `clearTimeout`, `setInterval`, `clearInterval` | `requestIdleCallback`, `cancelIdleCallback` |
| WEB_NETWORK | `fetch`, `Headers`, `Request`, `Response`, `Blob`, `AbortController`, `AbortSignal` | — |
| WEB_ROUTING | `window`, `location`, `history`, `Location`, `History`, `PopStateEvent`, `HashChangeEvent` | — |
| WEB_VIEWPORT | `BlitsenViewElement`, `BlitsenViewSurface` | — |
| WEB_STORAGE | — | `localStorage`, `sessionStorage`, `indexedDB` |
| WEB_WORKER | — | `Worker`, `SharedWorker`, `ServiceWorker`, `ServiceWorkerContainer` |
| WEB_MESSAGING | — | `MessageChannel`, `MessagePort`, `BroadcastChannel`, `postMessage` |
| WEB_SOCKET | — | `WebSocket`, `EventSource` |
| WEB_XHR | — | `XMLHttpRequest` |
| WEB_STREAM | — | `ReadableStream`, `WritableStream`, `TransformStream`, `Response.body`, `Response.clone` |
| WEB_FORM | — | `FormData`, `File`, `FileReader` |
| WEB_CANVAS | — | `HTMLCanvasElement`, `CanvasRenderingContext2D`, `OffscreenCanvas`, `ImageData`, `Path2D` |
| WEB_GPU | — | `WebGLRenderingContext`, `WebGL2RenderingContext`, `GPUCanvasContext` |
| WEB_MEDIA | — | `Image`, `Audio`, `AudioContext`, `webkitAudioContext`, `HTMLMediaElement`, `MediaQueryList`, `matchMedia` |
| WEB_DIALOG | — | `alert`, `confirm`, `prompt`, `print` |
| WEB_NAVIGATION | — | `open`, `close`, `navigation`, `document.write`, `document.writeln`, `document.open`, `document.close`, `location.assign`, `location.replace`, `location.reload`, `location.ancestorOrigins` |
| WEB_COOKIE | — | `document.cookie`, `cookieStore`, `Headers.getSetCookie` |
| WEB_DEVICE | — | `navigator`, `screen`, `Notification`, `caches` |
| WEB_OBSERVER | — | `ResizeObserver`, `IntersectionObserver`, `PerformanceObserver` |
| WEB_STYLE | — | `getComputedStyle`, `CSSStyleSheet`, `StyleSheetList` |
| WEB_COMPONENTS | — | `customElements`, `ShadowRoot`, `HTMLTemplateElement`, `DOMParser` |

| Diagnostic | Severity | Reported as |
| --- | --- | --- |
| `WEB_FETCH` | error | fetch resolves this URL against an address with no server behind it. |
| `WEB_DOM` | warning | This DOM method is not implemented. |
| `WEB_SCHEDULING` | warning | Idle-callback scheduling is not implemented. |
| `WEB_STORAGE` | error | Browser storage is not implemented. |
| `WEB_WORKER` | error | Web workers are not implemented. |
| `WEB_MESSAGING` | warning | Message channels are not implemented. |
| `WEB_SOCKET` | warning | Browser network streams are not implemented. |
| `WEB_XHR` | error | XMLHttpRequest is not implemented. |
| `WEB_STREAM` | warning | Streaming bodies are not implemented; a response is buffered whole. |
| `WEB_FORM` | warning | Multipart form bodies and file objects are not implemented. |
| `WEB_CANVAS` | error | Canvas is not in the v0 compatibility profile. |
| `WEB_GPU` | error | WebGL and WebGPU are not implemented. |
| `WEB_MEDIA` | warning | Audio and the media element constructors are not implemented. |
| `WEB_DIALOG` | error | Modal browser dialogs are not implemented. |
| `WEB_NAVIGATION` | error | Document navigation is deliberately absent; there is no page to leave. |
| `WEB_COOKIE` | error | There is no origin and no cookie jar behind an exported application. |
| `WEB_DEVICE` | warning | This device API is not implemented. |
| `WEB_OBSERVER` | warning | Layout and performance observers are not implemented. |
| `WEB_STYLE` | error | Computed style and the stylesheet objects are not implemented. |
| `WEB_COMPONENTS` | error | Custom elements, shadow DOM and DOM parsing are not implemented. |
| `CSS_TRANSITION` | warning | A property named by `transition` keeps its pre-stylesheet value (Blitz bug 689). |
| `CSS_FIXED` | warning | Fixed and sticky boxes resolve against the root box, not the viewport (Blitz bug 690). |
| `CSS_EFFECT` | warning | This paint effect is ignored rather than applied. |
| `HTML_CANVAS` | error | <canvas> is not implemented. |
| `HTML_MEDIA` | warning | Audio and video elements are not implemented. |
| `HTML_SVG` | warning | SVG rendering is currently limited and not in the strict profile. |

<!-- /generated -->

The scanner cannot prove visual equivalence or determine that an unsupported reference is dead
code. Treat a zero-error report as the build-time gate and retain visual/interaction acceptance tests
for the application itself. See the earlier [S6 renderer evidence](../spikes/s6/README.md) for why
this boundary exists.
