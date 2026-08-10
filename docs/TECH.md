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
        │        JavaScriptCore        │   JS/TS execution, modules,
        │      (hosted by Bun, P1)     │   async, timers, npm
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

The project **is the bridge**. Blitz supplies rendering; JSC supplies execution; the Rust
platform layer supplies the OS. Everything novel lives between them.

### Component ownership

| Layer | Source | We own |
| --- | --- | --- |
| HTML parsing, DOM tree, CSS cascade, layout, paint | Blitz (Stylo, Taffy) | Upstream; patches where needed |
| JS/TS execution, modules, npm resolution, event loop primitives | Bun / JavaScriptCore | Upstream; consumed via Node-API |
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
app.loadHTML("./index.html");
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

Bun demotes to toolchain. We load JSC directly (`rusty_jsc`-style bindings or our own) and
supply the runtime services the app actually needs — module loading against a pre-bundled
graph, timers, microtask draining — dropping the package manager, test runner, bundler,
transpiler, CLI, dev server and installer from the shipped binary.

Production exports dynamically load a user-replaceable JSC shared library to keep
closed-source distribution on the clean LGPL path. A one-file wrapper may carry and extract the
default library, but must allow an ABI-compatible replacement. Static JSC is for internal spikes
or a future mode that emits complete relinking materials. See `LICENSING.md`.

**The bridge API must not change between phases.** Everything in §5–§9 is specified against a
`JsEngine` trait with two implementations (Node-API-over-Bun, embedded-JSC). If Phase 1 code
reaches for Bun-specific behaviour outside that trait, Phase 2 becomes a rewrite instead of a
swap. This is the single most important structural constraint in the project.

---

## 3. Threading and event loop

One OS thread owns the window, the DOM, and the JS context. Blitz's DOM is not thread-safe and
JSC contexts are not freely shareable; fighting either is not worth it.

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

- **I/O, `fetch`, sockets, filesystem** — tokio runtime on a worker pool.
- **Asset decode** (images, audio, fonts, glTF) — rayon pool; results uploaded on the main
  thread.
- **Web Workers** — separate JS contexts on their own threads, structured-clone message
  passing only. No shared DOM access, exactly as on the web.

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
  `a === b` and `WeakMap` keying behave as authors expect. Implemented via a
  `NodeId → JsWeakRef` table.
- Wrappers are collected by JSC; a finalizer drops the table entry. The DOM node's lifetime is
  the tree's, never JS's — a detached node stays alive only while JS references it.

### v0 surface

The minimum for the DOM to feel real:

```
window                    document
  requestAnimationFrame     querySelector / querySelectorAll
  addEventListener          getElementById
  setTimeout / setInterval  createElement / createTextNode
  innerWidth / innerHeight  body / documentElement

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
data-script types are skipped; server-root and remote script URLs are rejected.

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

| API | Tier | Backed by |
| --- | --- | --- |
| `requestAnimationFrame`, timers | v0 | frame loop |
| DOM, CSSOM, events | v0 | bridge + Blitz |
| `fetch`, `Headers`, `Request`, `Response` | v1 | reqwest / hyper (or Bun's, in Phase 1) |
| `WebSocket` | v1 | tokio-tungstenite |
| `Image`, `<img>` decode | v1 | image / resvg |
| Web fonts, `@font-face` | v1 | Blitz font stack |
| `Audio`, basic Web Audio | v1 | rodio / cpal |
| `localStorage`, `sessionStorage` | v2 | SQLite or a keyed file store |
| `Worker` | v2 | second JS context + channel |
| Clipboard, drag & drop | v2 | arboard, winit |
| `navigator.getGamepads` | v2 | gilrs |
| Pointer lock, fullscreen | v2 | winit |
| `<canvas>` 2D | later | vello / tiny-skia into the viewport |
| WebGL / WebGPU | later | wgpu through the viewport |
| WebRTC | later | webrtc-rs |

Deliberately absent, with no plan: same-origin policy, CSP, cookies, history/navigation,
service workers, `document.write`, quirks mode.

### Compatibility policy

An unimplemented API is **absent** — the property does not exist — so feature detection works.
Never a stub that resolves to nothing, and never a silent no-op. `blitsen doctor` reports which
web APIs a bundle references but the target runtime does not provide, at build time.

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
| `native:app` | argv, executable path, app-data paths, single-instance lock, restart, quit, suspend/resume, file associations and `myapp://` protocol handling |
| `native:window` | create, resize, fullscreen, borderless, always-on-top, transparency, cursor control, monitor enumeration, DPI |
| `native:dialog` | open/save file, folder picker, message box |
| `native:clipboard` | text, images, arbitrary MIME |
| `native:tray` | tray icon, context menu, application menu |
| `native:notify` | desktop notifications |
| `native:input` | raw keyboard/mouse state, gamepads, potentially raw HID |
| `native:os` | CPU, memory, displays, username, platform, arch, battery, locale |
| `native:fs` | watching, temp files, memory-mapped buffers (beyond `node:fs`) |
| `native:net` | TCP/UDP sockets and listeners, beyond HTTP/WebSocket |

Generic system access uses the interfaces that already exist rather than new names —
`node:fs`, `node:child_process`, `node:net`, `bun:sqlite` — so existing packages work
unmodified.

**The escape hatch is a first-class feature.** `.node` addons load at runtime, so users write
Rust/C/C++ extensions and import them directly. Node-API's ABI stability is what makes this a
durable promise rather than a version-locked one.

```js
import physics from "./box2d.node";
```

This is why the runtime can afford to be unopinionated: anything we do not provide, the user
can add without waiting for us.

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
| ① **Ingest** | Walk the output directory from its HTML entrypoint. Resolve relative references; refuse absolute URLs that assume a web server root, with a clear error naming the file. |
| ② **Scan** | Static analysis of the bundle for web API usage; anything the target runtime lacks is reported (`blitsen doctor`, and as a build warning). |
| ③ **Collect** | Hash and collect assets. Embedded in the binary or laid out beside it, per config. |
| ④ **Link** | Runtime + application bundle + assets → one executable. |
| ⑤ **Package** | Icon, Windows manifest/version info, macOS `.app` bundle and `Info.plist`, code signing hooks. |

- **Phase 1** step ④ is `bun build --compile` with the Rust engine as an embedded `.node` addon.
  Bun already compiles JS/TS — and HTML entrypoints — into standalone executables, so this path
  mostly exists. Its output carries a full copy of the Bun runtime, which is the Phase 1 size
  cost (PRODUCT.md §9).
- **Phase 2** step ④ links our JSC-based runtime and appends the bundle as a binary section read
  at startup.

Step ② keeps the compatibility promise honest. Blitsen will be handed output it cannot run; the
failure must arrive at build time with a named API and file, not as a blank window at runtime.

### Optional build wrapping

Config in `package.json` lets Blitsen invoke the existing build first, so the user has one command:

```json
{ "blitsen": { "build": "vite build", "output": "dist", "name": "My App" } }
```

Blitsen shells out to `build`, then ingests `output`. It never inspects or configures the build
tool itself — that coupling is exactly what the design avoids.

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
preserved, `EventSource` is absent, and the actual browser-facing HMR client still depends on the
v1 web-platform surface. Directory mode is the v0 path; see `spikes/s7/README.md`.

Directory mode reload granularity: CSS swaps live via re-cascade with no reload; HTML and JS
restart the JS context and reparse the document. Preserving JS state across reload is not
attempted — HMR is the user's bundler's job, and in proxy mode it already is.

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

### Consequence for the Phase 1 → Phase 2 transition

This packaging boundary is what makes the host-model change (§2) invisible to users. In Phase 1
the platform package contains Bun-plus-addon; in Phase 2 it contains our own JSC-based runtime.
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
GitHub-hosted CI is deliberately disabled. Run the cross-platform bridge suite locally with
`bun run --cwd packages/blitsen test:native`; it builds and stages the platform's addon before
executing the same native assertions on Linux, macOS, or Windows.

- **Layout conformance** — a corpus of HTML/CSS cases rendered headless to PNG, compared against
  golden images per platform. Guards product requirement P6 (cross-platform identical layout),
  the main advantage over WebView-based tools. Seeded from a targeted subset of WPT reftests
  for the CSS features actually claimed.
- **Bridge unit tests** — DOM operations driven from JS, asserted against the Rust tree state.
- **Event dispatch tests** — synthetic events injected at the bridge boundary (below the OS), so
  propagation order, `preventDefault` and `stopPropagation` are testable without touching a real
  input device.
- **Frame determinism** — record/replay of an input trace at a fixed timestep, producing a
  deterministic frame hash sequence.
- **Size regression** — the local release verification records bare-app size and fails on
  regression beyond a threshold. Restore this check to automation only when CI is re-enabled.
- **Startup benchmark** — cold start to first frame, tracked per commit (P2).

---

## 15. Feasibility spikes

All eight spikes were completed on Linux x64. The consolidated outcome is **go, re-scoped**;
see [the M0 decision](M0.md). Windows and macOS validation is deferred.

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

1. **JSC acquisition for Phase 2** — vendor WebKit's JSC, use an existing Rust binding, or
   extract Bun's build? Each has a very different maintenance cost.
2. **Module resolution in the shipped binary** — pre-bundled single graph (simplest), or a
   runtime resolver against embedded files (supports dynamic `import()`)?
3. **Multi-window JS contexts** — one shared context or one per window? Shared is simpler and
   matches the single-thread model; isolated is safer and matches the web.
4. **DOM property access cost** — is a Rust call per property read fast enough under JSC, or do
   hot properties need caching on the JS wrapper with invalidation?
5. **Text input and IME** — a large, easily underestimated surface; where does it live?
6. **Accessibility** — Blitz's AccessKit story, and whether v0 can defer it. Deferring has a
   real cost for the dashboard/tooling audience.
7. **Font fallback and shaping** across platforms without pulling in a large font stack.
8. **Hot reload state** — accept full restart, or attempt module-level replacement? Largely moot
   in proxy mode, where the user's own HMR handles it.
9. **Absolute-path assets.** Bundler output routinely references `/assets/index.js`, assuming a
   server root. Rewrite at ingest, or serve the embedded bundle through an internal origin so
   absolute paths resolve unchanged? The latter is more faithful and probably necessary for
   frameworks with a configurable base path.
10. **Client-side routers.** History API and `location` are on the "deliberately absent" list
    (§8), but React Router and equivalents are ubiquitous in exactly the apps being courted.
    Reconsider: a minimal in-memory `history` may be a v1 requirement rather than a "later".
11. **Binary size vs. platform package count** — six prebuilt runtimes to build, sign, notarise
    and publish per release. What is the CI cost, and can it be cut with cross-compilation?
12. **Runtime version pinning** — does the platform package version lock to the JS package
    exactly, and what happens when an app's saved export config outlives a runtime release?

---

*Superseded document: `FIRST.md` (retained in git history at commit `d32f5e3`).*
