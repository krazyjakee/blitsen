# Blitsen — Technical Specification

**Status:** Draft v0.1
**Date:** 2026-08-10
**Companion document:** `PRODUCT.md` (what and why; this document is how)

---

## 1. Architecture

```
        Application — static web output
   index.html · assets/index.js · assets/index.css
     (produced by the user's own build tool, §10)
                       │
   ────────────────────┼────────────────────────────
                       ▼
        ┌──────────────────────────────┐
        │  QuickJS-ng (linked in, P2)  │   JS execution, modules,
        │  or Bun/JavaScriptCore (P1)  │   async, timers, npm
        └──────────────┬───────────────┘
                       │
                  ┌────▼─────┐
                  │  BRIDGE  │   ← this project
                  │          │
                  │ window   │   JS object model over the DOM
                  │ document │   event system
                  │ Element  │   web API compatibility layer
                  │ events   │   native: module namespace
                  └────┬─────┘
                       │
        ┌──────────────▼───────────────┐
        │            Blitz             │   HTML parse → DOM →
        │  Stylo (CSS) · Taffy (layout)│   style → layout → paint
        └──────────────┬───────────────┘
                       │
        ┌──────────────▼───────────────┐
        │        Rust platform         │   wgpu · winit · audio ·
        │                              │   input · net · fs · assets
        └──────────────┬───────────────┘
                       ▼
                  Native window
```

The project **is the bridge**. Blitz supplies rendering; the JavaScript engine supplies
execution; the Rust platform layer supplies the OS. Everything novel lives between them.

### Component ownership

| Layer | Source | We own |
| --- | --- | --- |
| HTML parsing, DOM tree, CSS cascade, layout, paint | Blitz (Stylo, Taffy) | Upstream; patches where needed |
| JS execution, modules, event loop primitives | QuickJS-ng (P2, linked in); Bun / JavaScriptCore (P1, via Node-API) | Upstream; consumed through the `JsEngine` trait |
| DOM ↔ JS bindings, event system, web API shims, `native:` modules | — | **Entirely ours** |
| Windowing, GPU surface, input, audio, filesystem, networking | winit, wgpu, rodio/cpal, tokio | Thin Rust wrappers, ours |
| Application bundling, transpilation, module resolution | The user's own tool (Vite, Webpack, Bun, …) | **Nothing. Deliberately not ours** (§16.6) |
| Ingest, link, package, platform distribution | — | **Entirely ours** (§10, §11) |

---

## 2. Host model — and the phase reversal

The instinct is to have Rust launch and embed a JS engine. **Phase 1 does the opposite**,
because it removes almost all of the initial integration work.

### Phase 1 — Bun is the host

The entire Rust engine ships as a **Node-API addon** (`.node`) loaded directly into Bun. Bun
implements most of Node-API, and Bun's own documentation recommends Node-API over `bun:ffi` for
production native integration — `bun:ffi` remains marked experimental. Node-API is also
ABI-stable by design, which matters for the third-party addon story later.

```js
import { Engine } from "blitsen:native";   // a .node addon under the hood

const app = new Engine();
app.openDirectory({ entrypoint: "./index.html" });
```

Bun and the Rust engine live in **one process**, sharing one thread for the main loop. There is
no IPC, no serialisation boundary, no second runtime to synchronise.

Bun already supports bundling native `.node` addons into compiled standalone executables, so
even in Phase 1:

```
bun build --compile app.js --outfile my-app
```

produces a single file containing Bun/JSC, the Rust engine, Blitz, the app and its assets —
with no Chromium, Electron or OS WebView. That is enough to hit M3 (Pong) without ever having
written an embedding layer.

**Cost:** the full Bun runtime is in the export (~60–100 MB).

### Phase 2 — Blitsen is the host

Bun demotes to toolchain. The runtime links **QuickJS-ng** statically and supplies the runtime
services the app actually needs — module loading against the application's own files, timers,
microtask draining — dropping the package manager, test runner, bundler, transpiler, CLI, dev
server and installer from the shipped binary.

An export is therefore one file: the engine is inside the executable, nothing ships beside it,
and the only terms it carries are MIT ones. That is a reversal of what this section originally
specified. The engine chosen here was JavaScriptCore, and because it is LGPL the design required
a dynamically loaded, user-replaceable shared library, a relink flow, and 32 MB alongside every
export. [`spikes/s8`](../spikes/s8/README.md) measured a permissively licensed engine behind the
same trait — 120 golden frames pixel-identical, 59.6 fps windowed, 25× smaller — and the JSC host
was removed once nothing shipped it. [`JSC.md`](JSC.md) keeps the record of that decision and
`LICENSING.md` records what the swap removed.

**The bridge API must not change between phases.** Everything in §5–§9 is specified against a
`JsEngine` trait with two implementations (Node-API-over-Bun, and the engine the Phase 2
executable links). If Phase 1 code reaches for Bun-specific behaviour outside that trait, Phase 2
becomes a rewrite instead of a swap. This is the single most important structural constraint in
the project — and the engine swap above is what proved it was worth holding: it changed one file
in `blitsen-runtime` and nothing at all in `blitsen-host`.

The constraint held. Everything between the DOM and the application lives in `blitsen-host`,
generic over `JsEngine` and naming no engine at all; `blitsen-node` is the Node-API implementation
plus the `#[napi]` surface, and `blitsen-runtime` is the Phase 2 executable. Native callbacks
recover their engine through `JsEngine::from_value` rather than capturing a host environment
pointer, which is what the Phase 1 bridge had been doing. One assumption had escaped: the
document-reload path cleared Bun's `require` cache unconditionally, and now clears whichever module
cache the host has.

---

## 3. Threading and event loop

One OS thread owns the window, the DOM, and the JS context. Blitz's DOM is not thread-safe and
JavaScript contexts are not freely shareable; fighting either is not worth it.

```
main thread
  winit event loop
    ├── OS input events        → queued
    ├── JS event loop turn     → run macrotasks, drain microtasks
    ├── rAF callbacks          → app mutates DOM / scene
    ├── style + layout (dirty subtrees only)
    ├── paint → wgpu submit
    └── present
```

Work that must not block the frame goes off-thread and returns through a queue drained at a
defined point in the turn:

- **I/O, `fetch`, sockets, filesystem** — tokio runtime on a thread pool.
- **Asset decode** (images, audio, fonts, glTF) — rayon pool; results uploaded on the main
  thread.
- **Web Workers** — separate JS contexts on their own threads, structured-clone message
  passing only. No shared DOM access, exactly as on the web.

Workers are built. A worker is a whole engine on a thread of its own — not a second context in
the document's — because the DOM is not thread-safe and neither host's values are shareable
across threads. Messages travel as a flattened record graph plus whole binary payloads through
one process-wide port registry (`blitsen-host/src/ports.rs`), which is what lets a port be
*transferred*: the queue hangs off the port rather than off the context, so a message sent just
before a port was handed to a worker arrives there rather than being left behind. Delivery is
polled at the same point every other off-thread source lands — the start of the animation-frame
stage — so a message cannot arrive part-way through a callback. The cost of that is a message's
latency being bounded by the frame rather than by the thread that sent it.

`blitsen-host` cannot create an engine, only use one, so the crate that chose the engine
registers a launcher (`worker::WorkerLauncher`) and everything after the engine exists is
engine-neutral. The shipped runtime launches QuickJS-ng; the Phase 1 addon launches QuickJS-ng
too, rather than Node-API, because there is no second `napi_env` to be had — which means a
worker's global scope is the same source and the same behaviour on both hosts.

### Event loop integration is the sharpest Phase 1 hazard

Bun owns an event loop. winit owns an event loop. Both want to be the outer one.

Options investigated by S1:

1. **Drive winit from a JS callback** — pump winit with `ControlFlow::Poll` from a repeating
   task registered on Bun's loop. Simple; risks input latency and frame pacing jitter.
2. **Drive Bun's loop from winit** — call into Node-API to advance the JS loop once per frame.
   Bun 1.3.14 on POSIX exposes no supported way to do this: `uv_run` aborts as unsupported and
   the actual uSockets tick is private.
3. **Two threads with a channel** — winit on the main thread (required on macOS), JS on
   another, all DOM mutation marshalled. Most robust, most overhead, and it breaks the
   single-context assumption above.

**S1 decision: option 1.** On Linux, Bun-driven non-blocking winit pumping sustained about 62
paint callbacks per second with 0.053 ms interval standard deviation and p99 synthetic
input-to-paint at 16.034 ms (600 samples). A 4 ms JS-work simulation produced the same result.
Input, redraw and lifecycle work stays synchronous inside the winit pump. Option 3 remains the
contingency if the full Pong workload or later Windows/macOS validation exceeds one frame. These
are host-pacing measurements, not the final P4 renderer benchmark; see `spikes/s1/README.md`.

---

## 4. Blitz integration

Blitz already provides an HTML parser, CSS engine, DOM, layout engine, window integration and
renderer, and its plain `blitz` frontend accepts an HTML string directly. What it does not
provide is interactivity — the interactive VirtualDOM/event path currently runs through
Dioxus/RSX, and Blitz states it has no JavaScript bindings. That absence is precisely the gap
this project fills.

What we need from Blitz, in likely order of friction:

| Need | Expected difficulty |
| --- | --- |
| Parse HTML → DOM, style, layout, paint | Available today |
| Stable node handles that survive tree mutation | Medium — must confirm the handle model |
| Imperative external mutation (insert, remove, set attribute, set style) with correct invalidation | **Hardest.** Blitz's mutation path is shaped for VirtualDOM diffs, not arbitrary external writes. |
| Hit testing for pointer events | Likely available via layout tree |
| Custom/native element with app-controlled painting | Blitz explicitly wants custom widgets and extensibility, so this fits its design |
| Fine-grained invalidation (dirty only affected subtrees) | Medium; naive full relayout is the fallback for v0 |

**Upstream contribution is expected**, not incidental. The bridge sits behind our own
`DomBackend` trait so that upstream shape changes are absorbed in one place, and so that a
patched Blitz fork can be swapped for upstream when features land.

---

## 5. The DOM ↔ JS bridge

The core of the project. The goal is not spec completeness — it is that **the DOM feels real**
to ordinary web code.

### Object model

Blitz owns the authoritative tree. JS gets handle objects, never copies.

```
JS side                     Rust side
────────                    ─────────
Element  ──┐
Node     ──┼── NodeId(u32, generation) ──► Blitz DOM node
Document ──┘
```

- A JS wrapper holds a generational `NodeId`. Generation counters make use-after-free a
  catchable error rather than silent corruption of an unrelated node.
- **Identity is preserved**: two lookups of the same node return the same JS object, so
  `a === b`, framework-owned properties and `WeakMap` keying behave as authors expect. A document
  context strongly interns wrappers over the native `NodeId → JsWeakRef` table. The strong context
  cache is required because frameworks attach listener/fiber state directly to connected wrappers;
  M3b demonstrated that weak-only wrappers can be reclaimed in a compiled Bun host.
- The context cache is cleared on document replacement. Detached wrappers may therefore remain
  until that boundary in v0; bounded context lifetime is preferred over observable identity loss.

### v0 surface

The minimum for the DOM to feel real:

```
window                    document
  requestAnimationFrame     querySelector / querySelectorAll
  addEventListener          getElementById
  setTimeout / setInterval  createElement / createTextNode
  innerWidth / innerHeight  body / documentElement
  MutationObserver          nodeType / nodeName / ownerDocument

Node / Element
  appendChild · insertBefore · removeChild · remove · replaceWith
  parentNode · childNodes · firstChild · nextSibling
  textContent · innerHTML
  getAttribute · setAttribute · removeAttribute
  classList (add/remove/toggle/contains)
  style (CSSStyleDeclaration, camelCase + setProperty)
  addEventListener · removeEventListener · dispatchEvent
  getBoundingClientRect
```

`innerHTML` requires the parser to be reachable as a fragment parser, not only at document
load. Confirm early — a surprising amount of real-world JS depends on it.

### v0 script loading subset

Directory mode collects script elements after HTML parsing and executes supported entries in
document order. Inline and local relative `src` scripts are supported, including `type="module"`
graphs resolved by Bun. Until incremental parsing and networking land, `async` and `defer` are
accepted but deliberately use the same deterministic post-parse document order. Non-JavaScript
data-script types are skipped and remote script URLs are rejected. Export ingest normalizes local
server-root HTML/CSS references into paths relative to each staged file, allowing Vite's default
`/assets/...` output without modifying the user's `dist` directory.

### Mutation and invalidation

Every setter is a Rust call that mutates the Blitz tree and marks it dirty. No shadow tree, no
diffing, no reconciliation on our side — the DOM is the single source of truth.

```
JS: el.style.left = "40px"
      ↓ binding
Rust: dom.set_inline_style(node, prop, value)
      ↓
mark node dirty (style) → ancestors dirty (layout)
      ↓
next frame: restyle + relayout dirty subtrees only
```

Blitz incremental layout is enabled by default. Each bridge layout flush consumes a separate
style-dirty set and ancestor-propagated layout-dirty set, and records the scheduled node counts
for that frame. Native harness snapshots expose those counters. If incremental layout is disabled,
the tracker switches to the explicit full-document fallback and reports the document's full node
count for both phases, making the cost visible rather than silently claiming fine-grained work.

Reads that depend on layout (`getBoundingClientRect`, `offsetWidth`, `scrollTop`) force a
synchronous layout flush if the tree is dirty — the same layout-thrashing hazard as the web,
with the same fix (batch reads before writes). We do not attempt to hide it.

The v0 bridge exposes `getBoundingClientRect`, `offsetWidth`/`offsetHeight`,
`clientWidth`/`clientHeight`, and readable/writable `scrollLeft`/`scrollTop`. Scroll setters clamp
the requested element without bubbling into an ancestor. Forced flushes are counted until the
next frame boundary; setting `BLITSEN_DEV_LAYOUT_WARNINGS=1` prints that count once per frame.

### Event system

```
OS input (winit)
   → hit test against the layout tree
   → build the propagation path (root → target)
   → capture phase, target, bubble phase
   → JS listeners invoked at each step
   → default action (focus change, scroll, text input) unless preventDefault()
```

v0 events: `click`, `mousedown`, `mouseup`, `mousemove`, `wheel`, `keydown`, `keyup`, `resize`, `load`.
`Event`, `MouseEvent` and `KeyboardEvent` carry the properties authors actually read
(`target`, `currentTarget`, `key`, `code`, `clientX/Y`, `button`, `preventDefault`,
`stopPropagation`).

A listener that throws must not corrupt dispatch: exceptions are caught per listener, reported,
and dispatch continues — as on the web.

`addEventListener` accepts `capture`, `once`, and `passive`. Passive listeners are tracked and
cannot cancel an event; Blitsen does not currently use the flag for browser-style scrolling
latency optimization.

Native pointer coordinates are converted from physical pixels to CSS pixels using the current
window scale. High-frequency `mousemove` input is coalesced to the latest position once per frame.
`mouseenter`, `mouseleave`, `mouseover`, and `mouseout` are deferred to v1; v0 uses bubbling
`mousemove` plus hit testing. An uncancelled `wheel` scrolls the nearest scrollable ancestor.
Uncancelled arrow, Page Up/Down, Home/End, and Space keydowns scroll from the active element and
bubble through the same ancestor-scrolling path. All of these defaults run only after propagation
has finished, so any non-passive listener on the path can suppress them with `preventDefault()`.

`document.activeElement` initially resolves to `body`. An uncancelled click focuses the nearest
enabled form control, link, or element with a nonnegative `tabindex`; Tab and Shift+Tab traverse
those connected elements in document order. `focus()` and `blur()` update active state before
dispatching their non-bubbling events. Keyboard input targets the active element, with logical
`key`, physical `code`, repeat state, and tracked modifiers. Text input and IME remain outside v0.

`DOMContentLoaded` exists in v0. It fires on `document` after the post-parse script list has
completed, moving `document.readyState` from `loading` to `interactive`. `load` then fires once on
`window` after Blitz reports that no critical document subresources remain, and moves the state to
`complete`. Native resize updates Blitz's viewport and `innerWidth`/`innerHeight` before dispatching
`resize`; listener mutations are therefore included in the redraw requested for that resize.

---

## 6. Frame pipeline

```
vsync / timer tick
  │
  ├─ drain OS input → dispatch DOM events
  ├─ run expired timers, drain microtasks
  ├─ run requestAnimationFrame callbacks   ← app mutates DOM here
  ├─ restyle dirty nodes        (Stylo)
  ├─ layout dirty subtrees      (Taffy)
  ├─ paint → display list
  ├─ record native viewport contents into the same frame
  ├─ submit to wgpu
  └─ present
```

`requestAnimationFrame` callbacks receive a `DOMHighResTimeStamp` measured from app start.
Mutations made during rAF land in the frame being built, not the following one — otherwise
animation is a frame behind and games feel wrong.

If a frame overruns budget, timers and rAF are not run twice to catch up; `dt` is passed
honestly and the app decides.

In Phase 1, Bun owns `setTimeout`, `setInterval`, cancellation, callback arguments and the
microtask checkpoint after each timer macrotask. The CLI yields to Bun between non-blocking
winit pumps, so expired timers and their promise jobs complete before the next pump invokes rAF.
The engine-neutral fallback queue used by Phase 2 mirrors the browser's 4 ms minimum delay after
five nested timers. Intervals rearm from the turn in which they actually ran and never burst to
catch up after an overrun.

---

## 7. The native viewport element

One custom element gives the app a GPU surface inside the layout, without making the DOM
responsible for high-performance rendering.

```html
<div id="hud">
  <progress id="health" max="100" value="100"></progress>
</div>
<blitsen-view id="view"></blitsen-view>
```

- Blitz lays it out as a replaced element; layout gives it a rect and a z-position.
- Its contents are drawn by the app into a texture, composited into the same wgpu frame as the
  painted DOM — one swapchain, one present, correct interleaving with DOM content above and
  below it.
- This is the seam through which `<canvas>`, WebGL and WebGPU later arrive. Get the compositing
  correct once, and each of those is an API over an existing mechanism rather than a new
  pipeline.

**Deliberately out of scope:** turning HTML elements into a 3D scene graph, or expressing
real-time transform state through CSS. CSS is not a good channel for per-frame 3D state. An
application that wants a scene graph brings one — through a native addon, WASM, or JS — and
Blitsen renders its output through the viewport. The runtime does not ship an ECS, a physics
engine, or a scene format.

---

## 8. Web API compatibility layer

Each API is implemented as a JS-visible shim over a Rust crate. The renderer stays a renderer —
Blitz's authors argue that things like WebSockets and `localStorage` should come from ordinary
Rust crates rather than bloating the engine, which suits this structure exactly.

**The tier table is generated, and is not restated here.** `COMPATIBILITY.md` carries it, built
from `packages/blitsen/src/api-manifest.json`, which is in turn read out of `dom_bridge.rs`. This
section used to keep its own copy and it drifted — it listed `fetch` as v1 "or Bun's, in Phase 1"
long after Blitsen supplied its own, called `@font-face` and `<img>` unbuilt after they shipped,
and promised an `Image` constructor that did not exist. One source of truth, or none.

What remains here is what a table cannot express: why a thing sits where it does.

| Not implemented | Backed by, when it is |
| --- | --- |
| `WebSocket` | tokio-tungstenite |
| `Audio`, basic Web Audio | rodio / cpal |
| Clipboard, drag & drop | arboard, winit |
| `navigator.getGamepads` | gilrs |
| Pointer lock, fullscreen | winit |
| `<canvas>` 2D | vello / tiny-skia into the viewport |
| WebGL / WebGPU | wgpu through the viewport |
| WebRTC | webrtc-rs |

Deliberately absent, with no plan: same-origin policy, CSP, cookies, **document navigation**,
service workers, `SharedWorker`, `BroadcastChannel`, `document.write`, quirks mode. The three
after navigation are all about sharing something between documents, and there is one document. `history` and `location` do exist, but as an
in-memory session history at a synthetic address — enough for a client-side router, and nothing
that navigates. `navigator` answers identity (`userAgent`, `platform`, `language`) and no
capability. `localStorage` and `sessionStorage` hold data for the life of the process and say so
through a `doctor` warning on every build; durable storage is a separate question.

### Where the DOM surface is narrower than its name

Three shapes the tier table cannot show, because the API is present and answers correctly within
them.

- **Collections are static.** `children`, `querySelectorAll`, `getElementsByTagName` and
  `getElementsByClassName` all return a `NodeList` snapshot. A re-query sees a mutation; the list
  handed out before it does not. Live `HTMLCollection` semantics need a document-versioned
  collection object, and nothing measured has held one across a mutation.
- **A namespaced attribute is keyed by namespace and local name, with no prefix.** That is the
  pair `getAttributeNS` asks for, so `setAttributeNS(xlink, "xlink:href", …)` round-trips, and
  `getAttribute("href")` correctly does not see it. What is lost is the prefix on the way back
  out: `getAttributeNames()` reports `href`, and serialization writes `href="…"`. The same is
  already true of markup the parser read, so this is the backend's attribute model rather than a
  bridge choice.
- **`getClientRects` returns one rect per line box.** Anything with a box of its own has exactly
  one, and it is the border box `getBoundingClientRect` returns, off the same layout flush and
  charged as the same forced layout. An inline element is not laid out as a box at all — it is a
  run of styled text inside its block — so one that wraps reports a fragment per line, and the
  bounding rectangle is only their union. A `<br>` and a `display: none` element report none:
  nothing laid them out, and an empty box at the origin would be an invention. There is still no
  fragmentation across columns or pages, because Blitz has neither.

### Compatibility policy

An unimplemented API is **absent** — the property does not exist — so feature detection works.
Never a stub that resolves to nothing, and never a silent no-op.

This is enforced rather than reviewed. `api-manifest.mjs` parses the bootstrap as the JavaScript
it is and refuses to emit a manifest that disagrees with it; the native harness asserts every API
the manifest calls absent is genuinely `undefined` against a real bridge context; and `doctor`
reads the same manifest, so a diagnostic cannot describe a capability the runtime does not have.
Enforcement was worth building: it found the Phase 1 host leaking `Worker`, `WebSocket`,
`FormData`, `ReadableStream`, `MessageChannel`, `alert` and more into every application, all of
which would have vanished at the Phase 2 engine swap.

### Subresources: images and web fonts

`<img>`, CSS `background-image` and `mask-image`, `@font-face` and external stylesheets all load
through one seam — the Blitz net provider — and Blitz decodes and paints all of them. Blitsen
supplies the provider, the codec selection and the observable loading state.

**Providers.** Headless harnesses use `LocalResources`: `file:` and `data:` resolved
synchronously on the calling thread, everything remote refused. Answering before `fetch` returns
matters, because an unanswered subresource is invisible — the document lays out unstyled,
textless and imageless and every assertion still passes. Refusing remote rather than skipping it
keeps an offline machine's frame identical to a connected one's. A window uses Blitz's real
provider over the winit event loop; either way the provider is wrapped so that each request's
outcome is recorded, since a failed fetch otherwise drops its handler in silence and leaves an
`<img>` in exactly the state it had while still loading.

**Image formats: PNG, JPEG, GIF (first frame), WebP, SVG.** Blitz decodes with the `image` crate but
declares it `default-features = false`, which compiles in *no* codecs at all; the format set is
chosen by depending on `image` directly from `blitsen-blitz` and letting feature unification
apply it. Without that every image fails to decode and the element silently lays out at zero
height — the same shape of failure as the `system-fonts` one. **SVG is not one of those codecs**:
it is not a raster format, and it arrives through blitz-dom's `svg` feature, which parses it with
usvg and paints it as vectors (#238). That feature did not compile until upstream fixed it
(blitz#687), which is why SVG images were absent before the pin moved.

**Font formats: WOFF2, WOFF, TTF, OTF.** SVG and EOT fonts are refused by Blitz and are not
coming. `@font-face` descriptors — `font-family`, `font-weight`, `font-style` — are what select a
face, not the metadata inside the file, so a family whose files disagree with the CSS about their
own name still matches.

**Blitsen is FOUT, never FOIT.** Nothing registers a font as a render-blocking resource, so text
paints in the fallback face immediately and reshapes when the web font arrives. The alternative
trades a restyle for a blank window on every cold start.

One layout flush resolves repeatedly while subresources are still landing, because a
`background-image` is only discovered once style resolves — after the pass that would have
applied it. Without that, every backdrop flashes empty for one frame.

**`Image`, `naturalWidth`, `naturalHeight`, `complete`, `load` and `error`.**
`DomBackend::image_state` reads the decode state, gated on a layout snapshot like the geometry
reads, because decoded data is applied while layout resolves. `complete` is true for an image with
no source and for one whose request is over — including one that failed, so a poller is never
stuck. `HTMLImageElement` is the wrapper interface for `<img>` and reads all three over that seam,
charging a forced synchronous layout exactly as `getBoundingClientRect` does; `new Image(w, h)`
builds one, with the two arguments as the content attributes a browser writes.

Blitz announces nothing when a subresource lands, so `load` and `error` are delivered by polling
the elements that owe an outcome, at the top of the frame where `fetch` completions are settled.
An element owes one from the moment a source is written to it, and a listener attached to an
element that has already settled is owed nothing: browsers fire no retroactive `load`, which is
what `complete` is there to answer. An image still in flight keeps the host turning — unless it is
detached, since Blitz requests a source only once the element is in the document, and waiting on
one that will never be asked for is waiting forever.

---

## 9. Native API layer

Capability the web does not have, under a namespace that makes the non-portability obvious at
the import site:

```js
import { Window }    from "native:window";
import { clipboard } from "native:clipboard";
import { openFile }  from "native:dialog";
import { app }       from "native:app";
```

| Module | Surface |
| --- | --- |
| `native:app` | app-data/cache/config paths, single-instance lock, restart, quit request, suspend/resume, file associations and `myapp://` protocol handling |
| `native:window` | create, resize, fullscreen, borderless, always-on-top, transparency, cursor control, monitor enumeration, DPI |
| `native:dialog` | open/save file, folder picker, message box |
| `native:clipboard` | text, images, arbitrary MIME |
| `native:tray` | tray icon, context menu, application menu |
| `native:notify` | desktop notifications |
| `native:input` | raw keyboard/mouse state, gamepads, potentially raw HID |
| `native:os` | processor, memory, storage volumes and OS identity; displays, battery, locale, idle time |

**The rule: `native:` is additive, never a superset.** Anything the Node surface already names
keeps its Node name — `process.argv`, `process.execPath`, `process.exit`, `node:os` for CPU /
memory / platform / arch / username, `node:fs`, `node:child_process`, `node:net`, `node:dgram`,
`bun:sqlite`. A `native:` module exists only for capability that has no Node spelling at all. This
is what keeps existing packages working unmodified, and it is why there is no `native:fs` or
`native:net`: filesystem watching is `node:fs.watch` and raw sockets are `node:net`/`node:dgram`.
Where a genuine gap remains (memory-mapped buffers, raw HID), it gets a narrowly named module of
its own rather than a parallel re-spelling of a module Bun already ships.

The rule reads against a host that ships Node's modules, which is what Phase 1 did. The shipped
Phase 2 runtime implements none of them and refuses `node:*` at resolution (COMPATIBILITY.md,
"Node compatibility in the shipped runtime"), so for the facts `node:os` would have named there is
no Node spelling left to keep and `native:os` is the whole API — which is what that section's
Phase 1 → Phase 2 table already routes `node:os` facts to. This is the rule applied, not waived:
the test is whether the *shipped runtime* has another word for the thing.

The rule also has a Phase 2 cost argument behind it. Once the embedded host replaces Bun (§3),
every Node module the design leans on becomes ours to supply — deferred work, not avoided work.
A `native:` module that duplicates one is that work done twice.

**The escape hatch is a first-class feature.** `.node` addons load at runtime — in development
and from an exported executable — so users write Rust/C/C++ extensions and reach them from
application code. Node-API's ABI stability is what makes this a durable promise rather than a
version-locked one.

A document script loads an addon through `require`, not through the ESM graph. Bun rejects
`import physics from "./box2d.node"` with *"To load Node-API modules, use require() or
process.dlopen instead of import"*, and a Phase 2 host that resolved `.node` as a module would
have to invent a semantics Node does not have. The spelling is therefore:

```js
// index.html: <script type="module" src="./main.js">
import { createRequire } from "node:module";
const physics = createRequire(import.meta.url)("./box2d.node");
```

`import.meta.url` is what makes this layout-independent: a module script is executed from the
directory the export materialized, so a specifier relative to the script resolves to the carried
addon under either asset layout. A classic (non-module) script has no `import.meta`, so an addon
is reachable only from `type="module"` scripts.

This is why the runtime can afford to be unopinionated: anything we do not provide, the user
can add without waiting for us.

### Carrying an addon into an export

An addon is **declared**, never inferred. `--addon <path>` (repeatable) and the `addons` array in
the `blitsen` config name one; the ingest walk and `--include` are both bounded by the directory
being ingested, and an addon normally lives outside it — `node_modules/<package>/build/Release`,
`target/release`. A declared addon inside the output keeps its path, so the specifier the
application was written against still resolves; one from outside lands at the top of the
application tree under its own file name. An addon the walk *does* reach, because a reachable
script names it as a literal that resolves to a file that exists, is carried by that reference
alone — declaring is an addition to reachability, not a loosening of it.

Every `.node` in an export is checked before it ships, however it arrived. Its container header
(ELF, Mach-O including universal slices, PE) must be a dynamic object for the host platform and
architecture, and it must export `napi_register_module_v1`; anything else fails the build naming
the file and the mismatch. This is the same refusal `--target` makes for a cross-target export,
for the same reason: every other asset is portable bytes, and an addon is the one thing that can
be architecturally wrong. An addon that ships and then fails at `dlopen` in front of a user is
worse than one that was never carried.

Both layouts load. `--assets embedded` materializes the addon into the private temporary
directory beside the scripts that require it; `--assets side-loaded` writes it into
`<outfile>.assets/`. Neither needs an execute bit — `dlopen`/`LoadLibrary` read the mapping — but
an embedded export does depend on the temporary directory being mappable, so a `noexec` `TMPDIR`
is a side-loaded case.

Declaration is an export concern only. `blitsen <directory>` runs the directory as it stands, so
an addon a development run has to reach is one the user's build already put there; `--addon` is
how that same addon survives into an executable when it did not.

---

## 10. Build and export pipeline

**The input is a directory of static web output, not a source tree.** Blitsen does not bundle,
transpile or resolve modules for the application — the user's existing toolchain already did
that, and duplicating it would make Blitsen a competitor to Vite instead of a target for it.

```
        the user's existing build          Blitsen's job starts here
   ┌──────────────────────────────┐   ┌────────────────────────────────┐
   src/ ──► vite/webpack/bun ──► dist/ ──► ① ingest ──► ⑤ package ──► MyApp.exe
                                  │
                     index.html · assets/index.js · assets/index.css · …
```

| Step | Action |
| --- | --- |
| ① **Ingest** | Walk the output directory from its HTML entrypoint. Preserve relative references, normalize local server-root HTML/CSS references inside staging, and refuse remote subresources. |
| ② **Scan** | Static analysis of the bundle for web API usage; anything the target runtime lacks is reported (`blitsen doctor`, and as a build error or warning). |
| ③ **Collect** | Hash and collect the reachable assets. Embedded in the binary or laid out beside it, per config. |
| ④ **Link** | Runtime + application bundle + assets → one executable. |
| ⑤ **Package** | Platform artifacts around the linked executable: icon, Linux `.desktop` entry, Windows application manifest, macOS `.app` bundle, and the signing hook. |

- **Phase 1** step ④ is `bun build --compile` with the Rust engine as an embedded `.node` addon.
  Bun already compiles JS/TS — and HTML entrypoints — into standalone executables, so this path
  is implemented for current-platform architecture proofs. Its output carries a full copy of
  the Bun runtime, which is the Phase 1 size cost (PRODUCT.md §9). Redistribution remains gated
  on the automated licensing checks in LICENSING.md.
- **Phase 2** step ④ links Blitsen's own runtime and appends the bundle as a binary section read
  at startup, implemented in `blitsen_core::bundle`. The section carries a version header, an
  index and the file data, followed by a trailer that locates and checksums it; startup reads the
  index and then reads files from their recorded offsets, never unpacking to disk. **Append first,
  then sign** — the signing hook in step ⑤ already runs last, which is what keeps a macOS or
  Authenticode signature valid, and the trailer is *found* rather than assumed to be the final
  bytes because a signature legitimately follows it. This is what an export links into now that the
  platform packages carry the Phase 2 runtime, and it is why an ordinary export is 37 MB rather
  than 145 MB (PRODUCT.md §9).

  One thing sends an export back to Phase 1: a carried `.node` addon, because Node-API is Bun's to
  provide and Blitsen's own runtime has none (§12). It is decided from what the export collected
  and reported in step ③ along with what the copy of Bun costs, because an executable that is
  smaller and does not start is not smaller.

  Module scripts do not enter that decision. They used to: while the Phase 2 runtime loaded
  JavaScriptCore at run time, the library it found might lack the module entry point — a patch a
  stock JSC does not carry — so the exporter asked it with `--engine-report` and fell back to the
  Bun host when the answer was no. QuickJS-ng loads modules through its stock public API and is
  linked in, so there is no longer a build whose engine could turn up without one, and the probe
  went with the engine that needed it. `BLITSEN_HOST` overrides the decision in either direction
  and refuses the combination that cannot work; it is deliberately not a CLI flag or a config key,
  so the npm surface is identical across the swap (§16.7).

Step ② keeps the compatibility promise honest. Blitsen will be handed output it cannot run; the
failure must arrive at build time with a named API and file, not as a blank window at runtime.
A profile **error** fails `blitsen build`; warnings are printed and the build proceeds.

Each step announces itself as it finishes, with what it produced — a link that takes ten seconds
must not look like a hang, and a dropped file must be attributable:

```
⓪ build   vite build (configured in /app/package.json)
① ingest  /app/dist/index.html
② scan    6 files, 0 errors, 1 warnings
③ collect 7 embedded assets
          dropped 2 files unreachable from index.html (--include <glob> keeps them): …
④ link    /app/MyApp
⑤ package linux: /app/MyApp.desktop, /app/MyApp.png
```

Exit codes are the CI contract: `0` on success, `1` for any refusal, with the message on stderr
naming the file that caused it (the unresolvable reference, the incompatible source, the icon).
Compatibility errors are written to stderr under their step; warnings stay on stdout.

### Reachability, not directory recursion

Step ① starts at `index.html` and follows references transitively: HTML `src`/`href`/`poster`/
`data` subresource attributes, CSS `url()` and `@import`, and the statically analysable module
edges in reachable scripts (`import`/`export … from`, a literal `import("…")`, and
`new URL("…", import.meta.url)`). Anchor `href`s are navigation targets, not subresources, and
are left exactly as authored.

- A local HTML or CSS reference that resolves to nothing fails the build, naming the referring
  file. Script references are matched heuristically, so an unresolved one is ignored rather than
  treated as a broken build.
- Files the walk never reaches are reported and dropped — an unreferenced file is pure export
  size. `--include <glob>` keeps them, which is also the escape hatch for a URL the walk cannot
  see because the application computes it at runtime.
- A native `.node` addon is declared with `--addon <path>` or the config's `addons` array, not
  kept with `--include`: both the walk and `--include` are bounded by the ingested directory, and
  an addon usually lives outside it. §9 covers what a declared addon is checked for and how
  application code loads one.

### Collect and layout

Every collected asset is hashed with SHA-256 over its **staged** bytes, so the hash describes what
ships rather than what the bundler emitted. `--assets embedded` (the default) imports each asset
into the executable and materializes it into a private temporary directory at launch;
`--assets side-loaded` writes them to `<outfile>.assets/` next to the executable and the runtime
opens them in place. Embedded is the default because single-file distribution is the product
claim; side-loaded exists for patchable assets and for media too large to justify carrying in the
binary.

Export is reproducible: the same input directory, output path, working directory, Bun version and
platform produce a byte-identical executable. Staging therefore lives at a stable path derived
from the output path rather than in a randomly named temporary directory — `bun build --compile`
records the compiled entrypoint's path inside the executable.

### Package and signing hooks

Step ⑤ runs over the linked artifact when a packaging option (`--icon`, `--bundle-id`,
`--app-version`) is given; without one the export is the bare executable it has always been.
Packaging targets the requested platform. `--target <platform>-<arch>` accepts any of the six
desktop triples and resolves that target's runtime package on demand. File generation works across
hosts; signing and notarisation still require the target platform or an external signing service.

| Host | Produced beside or around the executable |
| --- | --- |
| Linux | `<name>.desktop` with absolute `Exec` and `Icon`, plus the PNG or SVG icon |
| Windows | `<name>.exe.manifest` (`asInvoker`, per-monitor-v2 DPI, UTF-8 code page, `supportedOS` for 8.1 and 10/11) and `<name>.ico` |
| macOS | `<name>.app/` containing `Contents/MacOS/<name>`, `Info.plist`, `PkgInfo` and `Resources/<name>.icns`; side-loaded assets move into `Contents/MacOS/` with the executable |

One square PNG is converted into the container each platform wants: an `.ico` carrying a PNG
directory entry (Vista and later, so ≤ 256 px) and an `.icns` using the PNG-bearing `ic07`–`ic10`
types (128, 256, 512 or 1024 px). A prebuilt `.icns`, `.ico` or `.svg` is copied through, and any
other combination fails the build naming the file and the sizes it would accept.

**Windows executable resources are not embedded.** Writing an icon or a `VERSIONINFO` resource
into the PE image needs a resource compiler Blitsen does not carry, so nothing is patched into the
executable: the manifest ships as the sidecar Windows loads at startup, the `.ico` ships for
shortcuts and installers, and the build prints that this is what happened rather than implying an
embedded icon.

Signing stays outside Blitsen. `--sign <command>` runs the command with the artifact as its single
positional argument — the `.app` bundle on macOS, the executable elsewhere — after packaging and
before the build reports success; a non-zero exit fails the build. `codesign`, `signtool` and
notarization workflows are the user's, and Blitsen never touches certificates or keychains.

### Optional build wrapping

Config in `package.json` lets Blitsen invoke the existing build first, so the user has one command:

```json
{ "blitsen": { "build": "vite build", "output": "dist", "name": "My App" } }
```

`npx blitsen build` with no directory shells out to `build`, then ingests `output`. It never
inspects or configures the build tool itself — that coupling is exactly what the design avoids
(structural constraint 6). The command is handed to the platform shell exactly as written, run
from the directory holding that `package.json` with `node_modules/.bin` on `PATH`, and a non-zero
exit fails the build naming the command. A directory argument means "ingest this": it skips the
wrapping entirely, and every CLI flag overrides the configured value.

- **The `blitsen` key of `package.json` is the only config location.** No `blitsen.config.*`
  file, no cascade, nothing to search for — one file per project, the one the user's build
  scripts already live in.
- `output` is required and resolved against that `package.json`; `name` sets the window title and
  the default output file name; unknown or malformed keys fail before anything runs, naming the
  key and the file.
- The schema is published as `blitsen/config.schema.json` (JSON Schema draft-07) for editor
  completion, and it is the same object the CLI validates against, so the two cannot drift.
  `defineConfig` from the `blitsen` package runs that validation on a config object written in JS.

### Development modes

Both skip ③–⑤ entirely.

```bash
npx blitsen .                        # ① directory mode: watch files, reload
npx blitsen http://localhost:5173    # ② proxy mode: load from a running dev server
```

**Proxy mode is the strategically important one.** The runtime fetches the document and its
subresources over HTTP from the user's own dev server, so Vite/Webpack HMR, source maps and the
entire existing inner loop keep working — the native window simply replaces the browser tab.
This requires `fetch` and a module loader that can resolve over HTTP, which is a real constraint
on when it can ship. **S7 decision: proxy mode is v1, not v0.** Bun 1.3.14 can execute a
pre-scanned Vite graph and connect to `vite-hmr`, but runtime resolver callbacks are synchronous,
HTTP modules receive a synthetic `file:///http://…` identity, source-map identity is not
preserved, and the actual browser-facing HMR client still depends on the v1 web-platform surface.
One of S7's blockers has since gone: `EventSource` is implemented (#236), so the transport an HMR
client listens on is no longer among the reasons proxy mode waits. Directory mode is the v0 path; see `spikes/s7/README.md`.

Directory mode reload granularity: CSS swaps live via re-cascade with no reload; HTML and JS
restart the JS context and reparse the document. Preserving JS state across reload is not
attempted — HMR is the user's bundler's job, and in proxy mode it already is. The directory
watcher waits for 100 ms of quiet before acting, deduplicates paths, reloads a CSS-only batch
through Blitz's linked-stylesheet hook, and escalates any mixed or non-CSS batch to one document
replacement in the existing native window. Replacing a document discards its DOM listeners,
rAF callbacks, timers, configurable globals, and local module cache before its scripts run again.

---

## 11. Distribution and packaging

Blitsen ships as an **npm dev dependency that orchestrates a prebuilt native runtime**. The JS
package contains no runtime; the runtime is a per-platform binary package resolved at install
time.

```
blitsen                        ← thin JS: CLI, config, TypeScript definitions
├── bin/blitsen                  (dev · build · run · doctor)
└── optionalDependencies:
    ├── @blitsen/win32-x64     ┐
    ├── @blitsen/win32-arm64   │
    ├── @blitsen/linux-x64     │  each contains one compiled binary:
    ├── @blitsen/linux-arm64   │    Rust host · Blitz · JS runtime ·
    ├── @blitsen/darwin-x64    │    DOM↔JS bridge · web APIs · winit/wgpu
    └── @blitsen/darwin-arm64  ┘
```

- `optionalDependencies` + `os`/`cpu` fields in each platform package's manifest means npm,
  pnpm, yarn and Bun each install **only** the host's binary. This is the same mechanism esbuild
  and swc use, and it is well-supported across package managers.
- **No postinstall compile step, no Rust toolchain requirement** (product requirement P9).
  Install is a download.
- The JS package carries the TypeScript definitions for the `native:` APIs, so editor
  completion works without the runtime being loadable in a browser context.
- Cross-platform export (`blitsen build --target win32-x64` from Linux) requires that target's
  package, which the CLI can fetch on demand rather than at install.

The six manifests live in `packages/platforms/`, one directory per target, each declaring its
`os`/`cpu` pair, the addon `blitsen.node` and the executable an export links into. Every target has
a native runner in `.github/workflows/release.yml`; no binary is committed. The manual workflow
packs and dry-runs unless its `publish` input is explicitly true, then publishes all six platform
packages before the thin `blitsen` package and records the same tarballs on the GitHub release.
They are deliberately not workspace members: putting platform-specific packages in the root
lockfile would make a frozen install depend on artifacts that do not exist in a checkout.

### Resolving the runtime

`packages/blitsen/src/runtime.mjs` owns one ordered resolver for both native binaries. The addon
used by `run` and Phase 1 takes `BLITSEN_NATIVE_PATH`; the Phase 2 executable used by ordinary
exports takes `BLITSEN_RUNTIME_PATH`. Each variable accepts a path or `file:` URL, is visibly
reported as an unversioned override, and must contain a readable library/executable whose binary
format and architecture match the requested target.

After an override, both binaries follow the same ladder: the exact installed
`@blitsen/<platform>-<arch>` package, a release build in this checkout, then the versioned
cross-target download cache. A cross-target build may fetch the exact platform package when local
sources are exhausted; a host run does not start a network download merely because its installed
runtime is missing. The addon and executable are resolved independently because a checkout may
have built only one, but the installed pair is version-checked against the CLI before either is
used.

### Runtime version pinning

The platform package locks to the JS package **exactly** — `blitsen@X` declares
`"@blitsen/<target>": "X"`, no range. The two halves are one Node-API ABI plus one launcher
contract, built from the same commit and tested only as a pair; a range would license a
combination nobody ran. A mismatch is therefore an error, not a warning, raised at resolution
before a build command runs, and it names both versions and both ways out (move `blitsen`, or move
the runtime).

An application pins the runtime by pinning `blitsen` — its `devDependencies` entry and its
lockfile, the mechanism it already has. Blitsen deliberately adds no second pin in its own config:
a runtime version in `package.json`'s `blitsen` key could disagree with the dependency that
installs it, and then one of the two is lying. Deliberately staying on an older runtime means
staying on the matching `blitsen`, which keeps working and stays fetchable — nothing is
unpublished — for as long as npm serves it.

Every export records the runtime it linked: target, version, package name and how it was resolved,
stamped into the executable as a contiguous literal (`Symbol.for("blitsen.runtime")`, readable with
`strings`) and reported on the `blitsen build` line that announces the artifact. So an artifact
whose rendering is in question can name what produced it, which is also the answer to a saved
export config outliving a runtime release: the config never carried a version, the artifact
always does.

Support window: the current minor and the one before it. Older versions keep working; a report
against them is answered with an upgrade. A runtime change that alters rendering is not silent —
it re-records the committed layout goldens (§14), so it arrives as a reviewable diff and a release
note rather than as a surprise in a user's window.

### Consequence for the Phase 1 → Phase 2 transition

This packaging boundary is what makes the host-model change (§2) invisible to users. In Phase 1
the platform package contains Bun-plus-addon; in Phase 2 it contains our own runtime.
It now contains both — `blitsen.node` for `blitsen run` and the Phase 1 export path, and
`blitsen-runtime` for the ordinary export — built, signed and published together, because a package
carrying one without the other fails at whichever command needs the missing half. Neither carries a
JavaScript engine alongside it: `blitsen-runtime` links QuickJS-ng statically (`LICENSING.md`), so
the platform package is two files and not three.
That invisibility is now checked rather than intended: `bun run --cwd packages/blitsen test:hosts`
builds one project on both runtimes and fails on any difference in CLI output, config handling,
refusals, artifact layout or the exported application's own self-check, and replays the committed
frame trace on the new runtime against the digests the old one recorded. The user-facing note is
[`MIGRATION.md`](MIGRATION.md), and it says one thing.
The npm package, the CLI, the config format and the user's `package.json` are identical across
that change. The host transition is still expected to reduce size, but M0 disproved the original
25–50 MB target; both installed executable and shipped dynamic-library size must be remeasured.
Keeping the distribution boundary stable remains a strong argument for building it early.

### Native API resolution

The `native:` specifier form is not resolvable by ordinary bundlers, which will try to bundle or
fail on it — a real problem for a design whose premise is that the user's bundler runs first and
unmodified. Two mitigations, likely both:

- Ship real module paths (`blitsen/dialog`, `blitsen/window`) that any bundler resolves today, with
  the package's browser/module field pointing at a stub that throws outside the runtime.
- Provide optional bundler plugins that mark `native:*` external, for users who prefer that
  spelling.

Generic system access needs neither: `node:fs`, `node:child_process` and `bun:sqlite` are already
understood by the ecosystem, and in Phase 1 they come free from Bun's Node compatibility — Bun
implements roughly 95% of Node-API, which is also what makes the addon strategy in §2 viable.

---

## 14. Testing

Interactive verification is the user's job; everything below is designed to run headlessly.
GitHub-hosted CI runs the full Rust, native acceptance, layout and metrics suites on Linux x64,
macOS arm64 and Windows x64. Linux arm64, macOS x64 and Windows arm64 each run a release-artifact
smoke tier covering both binaries, the package tests, a frame, a standalone export and the layout
corpus. Android has a cross-compile/package smoke tier. Run the bridge suite locally with
`bun run --cwd packages/blitsen test:native`; it builds and stages the platform's addon before
executing the same native assertions on Linux, macOS, or Windows.

- **Layout conformance** — a corpus of HTML/CSS cases rendered headless to PNG, compared against
  golden images per platform. Guards product requirement P6 (cross-platform identical layout),
  the main advantage over WebView-based tools. Seeded from a targeted subset of WPT reftests
  for the CSS features actually claimed.
- **Bridge unit tests** — DOM operations driven from JS, asserted against the Rust tree state.
- **Event dispatch tests** — synthetic events injected at the bridge boundary (below the OS), so
  propagation order, `preventDefault` and `stopPropagation` are testable without touching a real
  input device. The target-based injection helper is installed only by the headless native
  harness and is absent from shipped windows.
- **Frame determinism** — record/replay of an input trace at a fixed timestep, producing a
  deterministic frame hash sequence.
- **Size regression** — installed and gzip size are recorded against a committed per-platform
  baseline, and CI fails on growth beyond 2%. The toolchain is pinned in that job so a compiler
  bump is a deliberate re-baseline rather than a mystery failure.
- **Startup benchmark** — cold start and idle RSS, recorded per commit but not gated: hosted
  runners are too noisy to fail a build on. Headless runs measure documented proxies; the real
  windowed metrics need a desktop session and are opt-in (`bench:windowed`).

---

## 15. Feasibility spikes

All eight spikes were completed on Linux x64, so every number in this section is a Linux number.
The consolidated outcome is **go, re-scoped**; see [the M0 decision](M0.md). Windows and macOS are
no longer deferred — all six targets build and test in CI (§11) — but nothing here was re-measured
on them ([issue #123](https://github.com/krazyjakee/blitsen/issues/123)).

| # | Question | Kills / changes what |
| --- | --- | --- |
| **S0** | Compile JSC + Blitz into one binary. What does it actually weigh, stripped, with LTO? | The entire Phase 2 size budget, and the headline product claim. Run first. |
| **S1** | Can Bun's event loop be pumped externally from winit at frame rate, without input latency or pacing jitter? | §3 host model; possibly forces a two-thread design. |
| **S2** | Does Blitz tolerate arbitrary external DOM mutation with correct invalidation, or is its mutation path VirtualDOM-shaped only? | §5 — the core bridge. May force upstream work or a fork. |
| **S3** | Can a Rust Node-API addon reliably own a winit window inside a Bun process on all three platforms? (macOS main-thread window requirements are the hazard.) | Phase 1 entirely. |
| **S4** | Is Blitz's HTML parser reachable as a fragment parser for `innerHTML`? | §5 surface completeness. |
| **S5** | Can an app-rendered wgpu texture composite into Blitz's paint output in one frame, correctly z-interleaved? | §7 viewport, and everything canvas/WebGL later depends on. |
| **S6** | Take an unmodified `vite build` output from a real React app and render it. How much of its CSS does Blitz get right, and what breaks first? | §10 ingest and the entire drop-in premise. Cheap to run today with Blitz alone — **no bridge required** — which makes it the best early read on feasibility. |
| **S7** | Can the runtime load a document and its module graph over HTTP from a running dev server? | §10 proxy mode, the cheapest adoption on-ramp. |

S0 killed the original 25–50 MB estimate, S2 validated bridge-driven mutation without a fork,
and S6 narrowed the unrestricted drop-in claim to a documented compatibility profile. The
remaining spikes validated the Linux host architecture while identifying bounded upstream work.

---

## 16. Structural constraints

Rules that, if broken, cost a rewrite rather than a refactor:

1. **All JS engine access goes through the `JsEngine` trait.** No Bun-specific behaviour leaks
   into bridge code. This is what makes Phase 2 a swap.
2. **All DOM access goes through the `DomBackend` trait.** Blitz is a dependency, not an
   assumption.
3. **The bridge never keeps a shadow copy of the tree.** One source of truth, always Blitz's.
4. **No web API is added without its absence being detectable.** Feature detection must work.
5. **Nothing about games, scenes, ECS or physics enters the runtime.** The viewport element is
   the boundary; past it is application territory.
6. **Blitsen never bundles or transpiles the application.** The input is built static output.
   The runtime may load its already-built module graph, but the moment Blitsen owns the user's
   source build it becomes a competitor to Vite rather than a target for it.
7. **The npm surface — CLI, config, package layout — is stable across the Phase 1 → Phase 2 host
   change.** Users must experience that migration as a smaller binary and nothing else.

---

## 17. Open technical questions

1. **Engine acquisition for Phase 2: decided, then reversed, and closed.** The first answer was
   JavaScriptCore: build a pinned Bun WebKit revision in Blitsen's native release matrix, own the
   narrow Rust ABI layer, and dynamically load the replaceable production library
   ([`JSC.md`](JSC.md)). [`spikes/s8`](../spikes/s8/README.md) then measured QuickJS-ng behind the
   same trait and it won on every axis that mattered — MIT rather than LGPL, 25× smaller, no
   library to ship, no patch needed for module loading, and the golden frames unchanged. The
   runtime links it statically and the JSC host has been removed; there is no release-matrix
   engine build and no acquisition problem left.
2. **Module resolution in the shipped binary: decided.** A runtime resolver over the
   application's own files, addressed by an internal `blitsen://app/` origin, with linking left to
   the engine's module loader. A pre-bundled single graph was rejected because real framework
   output is already split and every router in the audience documents `import()` as the way to code
   split. See [`MODULES.md`](MODULES.md).
3. **Multi-window JS contexts** — one shared context or one per window? Shared is simpler and
   matches the single-thread model; isolated is safer and matches the web.
4. **DOM property access cost: decided — no wrapper-side cache.** Measured on the Phase 1 host
   (Linux x64, release): a `style.top` write costs 3.37 µs, a style read 1.41 µs, `setAttribute`
   1.69 µs, `getAttribute` 0.61 µs, `textContent` 1.04/0.48 µs, `getElementById` 2.03 µs, against
   0.05 µs for a plain JS property write. So a bridge call is roughly 30–60× a JS property access,
   and that ratio is the tempting case for a cache. It is not worth taking: Pong's four writes per
   frame total ~14 µs, which is 1.7% of its measured 0.81 ms frame and 0.08% of the 16.7 ms budget,
   while a cache would have to be invalidated by `cssText`, `setAttribute("style")`,
   `removeProperty` and `innerHTML` — four ways to serve a stale value in exchange for saving
   nothing that is currently spent. Revisit only if a profile shows property access on a hot path.
   Note what *does* cost: `getBoundingClientRect` is 10.5 µs clean and 66.8 µs after a write, so
   layout flushing, not property access, is the thing worth avoiding in a frame.
5. **Text input and IME** — a large, easily underestimated surface; where does it live?
6. **Accessibility** — Blitz's AccessKit story, and whether v0 can defer it. Deferring has a
   real cost for the dashboard/tooling audience.
7. **Font fallback and shaping** across platforms without pulling in a large font stack.
8. **Hot reload state** — accept full restart, or attempt module-level replacement? Largely moot
   in proxy mode, where the user's own HMR handles it.
9. **Absolute-path assets: decided.** Rewrite at ingest. Server-root subresource URLs in HTML and
   CSS are resolved against the output directory and rewritten to document-relative paths **in the
   staged copy only**; the user's `dist` is never modified. A configured bundler base (`/app/…`)
   is handled by dropping leading URL segments until one resolves against the real output layout,
   so default and custom `base` builds both ingest without configuration. An internal origin was
   rejected for v0: it needs a loader that resolves an embedded namespace for every subresource
   kind, and it buys nothing the rewrite does not already deliver for static output. The cost is
   that URLs computed at runtime are not rewritten — see §10 and `COMPATIBILITY.md`.
10. **Client-side routers.** History API and `location` are on the "deliberately absent" list
    (§8), but React Router and equivalents are ubiquitous in exactly the apps being courted.
    Reconsider: a minimal in-memory `history` may be a v1 requirement rather than a "later".
11. **Binary size vs. platform package count** — six prebuilt runtimes to build, sign, notarise
    and publish per release. What is the CI cost, and can it be cut with cross-compilation?
12. **Runtime version pinning: decided.** The platform package locks to the JS package exactly,
    the application pins the pair by pinning `blitsen`, a mismatch is a hard error at resolution,
    and every export records the runtime it linked so an artifact can name what produced it. See
    §11; the mechanical parts are implemented, the support window is policy.

---

*Superseded document: `FIRST.md` (retained in git history at commit `d32f5e3`).*
