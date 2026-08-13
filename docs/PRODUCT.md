# Blitsen — Product Specification

**Status:** Draft v0.1
**Date:** 2026-08-10
**Name:** Blitsen — npm package `blitsen`, CLI `blitsen`, platform packages `@blitsen/*`

---

## 1. What it is

Blitsen is a **browserless implementation of enough of the web platform to run HTML/CSS/JS
applications as native desktop executables.**

HTML, CSS and JavaScript in. A single native binary out. No Chromium, no Electron, no OS
WebView.

The one-sentence pitch for the README:

> Write an app in HTML, CSS and TypeScript. Ship a native executable. No browser included.

---

## 2. The problem

Developers who want to build a desktop application with web technology have three options
today, and all three have a defect that the other two do not.

| Option | Defect |
| --- | --- |
| **Electron / NW.js** | Ships an entire Chromium + Node stack per app. Hundreds of MB, heavy RAM baseline, slow cold start. |
| **Tauri / WebView-based** | Small binary, but rendering is delegated to whatever WebView the OS happens to have. Behaviour differs per platform and per OS version, and the app inherits the browser sandbox and its update schedule. |
| **Native toolkits (Qt, GTK, egui, Godot, …)** | Consistent and small, but abandons the web programming model, the CSS layout engine, and the npm ecosystem entirely. |

The gap: **a rendering engine you ship and control, at a size that isn't absurd, that still
speaks HTML/CSS/JS and still reaches the OS.**

The pieces to fill that gap now exist independently and have not been joined:

- **Blitz** (DioxusLabs) natively renders HTML and CSS in Rust — real HTML parser, Stylo CSS
  engine, DOM, Taffy layout, painting, windowing. Its plain HTML frontend already accepts an
  HTML string directly. What it lacks is interactivity: it has no JavaScript bindings, and the
  interactive path currently runs through Dioxus/RSX instead.
- **JavaScriptCore**, as shipped and driven by **Bun**, provides JS/TS execution, the module
  system, npm resolution, async, timers, and a native-addon interface.

Blitsen is principally **the layer that joins them**: the DOM ↔ JS bridge, the web API
compatibility surface, and the packaging story.

---

## 3. Positioning

Blitsen is **not a browser** and does not aspire to be one. It is a native application runtime
that happens to use the web platform as its UI and rendering model.

That distinction drives every scoping decision:

- No same-origin policy, no sandbox, no permission prompts by default. The application is
  trusted native software, not an untrusted document.
- No obligation to implement a web API just because browsers have one. Coverage is chosen by
  demand, not by specification completeness.
- Missing platform pieces are supplied by ordinary Rust crates rather than being absorbed into
  the renderer — the same philosophy Blitz's authors state for things like WebSockets and
  `localStorage`.

| | Browser | Electron | Tauri | **Blitsen** |
| --- | --- | --- | --- | --- |
| Renderer you control & ship | ✗ | ✓ | ✗ | ✓ |
| Consistent across OS versions | ✗ | ✓ | ✗ | ✓ |
| Bare app size | n/a | ~150–250 MB | ~5–15 MB | **budget pending full-host measurement** |
| Full OS access | ✗ | ✓ | ✓ | ✓ |
| npm ecosystem | ✓ | ✓ | ✓ | ✓ |
| Adopt without restructuring the project | n/a | partial | partial | compatible apps: one dev dependency |
| Web spec completeness | ✓✓✓ | ✓✓✓ | ✓✓✓ | partial, by design |

Honest statement of the trade: **Blitsen will render less of the web than a browser does.** It
wins on binary size versus Electron and on consistency and control versus Tauri, and it loses
on spec coverage against all three. An app targeting Blitsen is authored against Blitsen, not
ported blind from the web.

---

## 4. Who it is for

**Primary — the desktop app author who already thinks in web.**
Editors, dashboards, media tools, launchers, kiosk and signage software, internal tooling.
They want CSS layout and npm, and they resent shipping Chromium to get it.

**Secondary — the 2D game developer.**
A game is one thing somebody builds with Blitsen, not the product's definition. But it is the
sharpest proof: it demands a real frame loop, real input latency, and real GPU output, so it
validates the architecture harder than a settings dialog does.

**Tertiary — the native developer who wants a UI layer.**
Has Rust/C++ that does the real work; wants HTML/CSS for the front of it and a stable native
addon boundary rather than an IPC protocol to a browser process.

**Explicitly not for:** anyone who needs to render arbitrary third-party web content. Use a
browser engine.

---

## 5. Product principles

1. **The runtime decides as little as possible.** Blitsen supplies the web platform, native
   execution, native rendering and native packaging. The application supplies architecture,
   libraries, physics, state management, networking and rendering technique. A game, a
   dashboard, an editor and a kiosk are all just applications.
2. **Target existing projects, with compatibility stated up front.** Blitsen is an export target
   that consumes static web output, not a framework you start a project in. The developer keeps
   their bundler and framework. For applications inside the published compatibility profile,
   adoption is one dev dependency and one script line; `blitsen doctor` must name unsupported
   features before export.
3. **Web-standard API before bespoke API.** If the web already names a thing, use that name and
   that shape. Invent new surface only where the web has no answer (the OS).
4. **Partial is fine; incoherent is not.** An unimplemented API should be absent and documented
   as absent, never present-and-subtly-wrong.
5. **Always an escape hatch.** Anything the runtime does not provide can be reached through a
   native addon. Users are never blocked waiting on us.
6. **Size is a feature.** Every megabyte in the export is a product decision, and gets
   justified.

---

## 6. Developer experience

### Blitsen is an export target, not a framework to start projects in

The distribution model is a **dev dependency that acts as a native export toolchain**, while the
application stays an ordinary web project. Nothing about the project's shape, bundler or
framework is prescribed.

```bash
npm install -D blitsen
```

An existing project is unchanged:

```
my-app/
├── package.json
├── index.html
├── src/
│   ├── main.ts
│   └── style.css
└── node_modules/
```

It gains one script:

```json
{
  "scripts": {
    "dev": "vite",
    "build": "vite build",
    "native": "blitsen build ./dist"
  }
}
```

```bash
npm run build     # existing toolchain produces static output
npm run native    # static output → native/MyApp.exe
```

This is the single most important product decision in the document. **The input to Blitsen is a
directory of static web output**, which makes it bundler- and framework-agnostic by
construction:

```bash
vite build && blitsen build dist      # React, Vue, Svelte, Solid, …
webpack && blitsen build dist
bun build && blitsen build dist
blitsen build .                       # vanilla HTML, no build step at all
```

Three.js, Phaser, Pixi, jQuery, HTMX — the exporter does not care what produced the files or
what they import, **provided the web APIs those libraries need are implemented by the runtime.**
That proviso is the real compatibility boundary, and it is a runtime question (§7), never an
exporter question.

### Optionally, wrap the build too

Configuration can absorb the existing build command so there is one step:

```json
{
  "blitsen": {
    "build": "vite build",
    "output": "dist",
    "name": "My App"
  }
}
```

```bash
npx blitsen build
```

```
        existing project
               │
        existing build tool  (Vite / Webpack / Bun / …)
               │
        static web output
               │
            blitsen
               │
      ┌────────┴────────┐
      ▼                 ▼
 embed application   native runtime
      └────────┬────────┘
               ▼
           MyApp.exe
```

### Development

No export needed while developing:

```bash
npx blitsen .                          # open index.html in a native window
npx blitsen http://localhost:5173      # point at a running Vite dev server
```

The second form matters: developers keep their existing dev server, HMR and tooling exactly as
they are, and simply see the result in the native runtime instead of a browser. Production
export then consumes the same tool's static output. Nothing about the inner loop changes.

### Application code

Ordinary web code, ordinary npm:

```js
import Matter from "matter-js";
import { vec3 } from "gl-matrix";

document.querySelector("#score").textContent = score;

requestAnimationFrame(function update(t) {
  // ...
  requestAnimationFrame(update);
});
```

Plus OS capability the browser cannot give, under a clearly-marked namespace:

```js
import { openFile } from "native:dialog";
import { clipboard } from "native:clipboard";
import { Window } from "native:window";

const path = await openFile();
await clipboard.writeText("hello");
const tools = new Window({ width: 800, height: 600, html: "./tools.html" });
```

Plus generic system access through the interfaces that already exist:

```js
import fs from "node:fs";
import { spawn } from "node:child_process";
```

The two never overlap. `native:` covers only what has no Node spelling at all — windows, trays,
dialogs, clipboard. Files, processes, sockets and process metadata keep their Node names, so
existing packages work unmodified.

Plus the unlimited escape hatch — a Node-API addon, loaded from a module script. `import` is not
the spelling: Bun refuses it for Node-API modules, and it took building the thing to find that out.

```js
import { createRequire } from "node:module";
const physics = createRequire(import.meta.url)("./box2d.node");
```

### The capability story, stated plainly

```
Browser                    Blitsen
HTML + CSS + JS            HTML + CSS + JS
      │                          ├── filesystem
      ▼                          ├── processes
 sandbox boundary                ├── sockets
      ✗                          ├── native libraries
                                 └── Rust/C/C++ addons
```

---

## 7. Scope by tier

Availability is incremental. A browser having an API does not oblige Blitsen to ship it.

**v0 — proves the architecture** — *met*
HTML · CSS · DOM · JS/TS execution · events · `requestAnimationFrame` · `setTimeout`/
`setInterval` · mouse · keyboard · one native viewport element backed by the GPU.

**v1 — makes real apps possible** — *met, with three members partial*
`fetch` · `WebSocket` · images · web fonts · audio playback · the first `blitsen/*` modules
(dialog, clipboard, window, app). The published profile is
[v1](COMPATIBILITY.md), generated from the runtime, and `doctor` checks against it.

| Partial | Why |
| --- | --- |
| `dialog.*` | Linux and the BSDs only, by design: macOS and Windows require a file dialog on the main thread, which is the thread kept free to paint. Absent there rather than approximated. |
| `app.requestSingleInstanceLock` | Unix only. The lock is a Unix domain socket that doubles as the channel a second invocation's `argv` arrives on; Windows wants a mutex plus a named pipe, which is a different design. |
| `window.create` | Absent — a second window is #105, and the run opens one window. |

**v2 — makes real apps comfortable** — *partly landed early*
`localStorage`/`sessionStorage` (in memory, not persisted) · Workers (dedicated, with
`MessageChannel`, `MessagePort` and `structuredClone`) have landed. Still open: clipboard events ·
drag & drop (with real filesystem paths, not browser `File` abstractions) · gamepads · tray/menu ·
notifications.

**Later — as demand justifies**
`<canvas>` 2D · WebGL / WebGPU · WebRTC · anything else earning its size.

**Where the v1 line was drawn, stated plainly.** `<canvas>` (#99), text input and IME (#103) and
accessibility (#102) are **not v1**. `<canvas>` in particular is a `doctor` **error** rather than a
warning — the element ships in the document and nothing paints inside it, and unlike an image or a
font it has no degraded appearance to fall back to. So "v1 makes real apps possible" and "canvas is
an error" are true at the same time, and that is the one place the tiers can be read as more
generous than the runtime is: a DOM-and-CSS application is inside v1 whole, and an application that
draws is refused at export until #99. [COMPATIBILITY.md](COMPATIBILITY.md#what-v1-is-not) carries
the same table.

### The compatibility boundary is the runtime, never the exporter

Because the exporter takes static web output, "does React work?" is never a question about the
exporter. It is always: **does the runtime implement the web APIs that this code path touches?**

| Library | Gated on |
| --- | --- |
| React, Vue, Svelte, Solid, HTMX, jQuery | v0 DOM + events. These are the target of v0. |
| Pixi, Phaser, Three.js | `<canvas>` / WebGL — the "later" tier. Not v0 or v1. |
| State, utility, data libraries (lodash, zustand, …) | Nothing. Plain JS runs today. |

This is worth stating loudly because it sets honest expectations: a DOM-driven React dashboard
is an early target, while a Three.js scene waits on WebGL. Blitz is also still pre-alpha and
deliberately does not implement the whole browser platform, so the near-term boundary is tighter
than the tier list's endpoint suggests.

`blitsen doctor` reports which web APIs a built bundle references that the runtime does not
provide — so the answer for any given project is mechanical, not guesswork.

### Non-goals

- Rendering arbitrary websites, or passing the web platform test suite.
- A same-origin policy, CSP, or any sandbox for first-party application code.
- Legacy layout quirks, `document.write`, or the quirks-mode parser path.
- An opinionated UI framework, component model, or state library of our own.
- A mobile or console target in the initial phases.
- Prescribing how anything is drawn inside the app. CSS transforms, a physics library, a WASM
  build of Box2D, a native addon and (eventually) `<canvas>` are all equally valid.

---

## 8. Product requirements

| # | Requirement | Target | Notes |
| --- | --- | --- | --- |
| P1 | Bare exported app size | **38.1 MB, and that is the whole download** — measured on Linux x64, against 131.6 MB for the same app on Phase 1 | S0 disproved the original ≤50 MB target against a design that shipped an engine library alongside; statically linking QuickJS-ng put the shipped total back inside it. Five targets unmeasured, and no measured Electron or Tauri comparison yet; see §9. |
| P2 | Cold start to first frame | < 500 ms on mid-range hardware | Should beat Electron decisively or the pitch weakens. |
| P3 | Idle RAM, bare app | < 100 MB | |
| P4 | Sustained frame rate | 60 fps for a moderate 2D scene | Measured with the Pong acceptance build. |
| P5 | Platforms, initial | Windows x64, Linux x64, macOS arm64 | Windows is the priority target for size claims. |
| P5b | Platforms, full matrix | win32 x64/arm64, linux x64/arm64, darwin x64/arm64 | One npm platform package each (TECH.md §11). |
| P6 | Render consistency | Byte-identical layout across platforms for the test corpus | The core advantage over WebView-based tools. |
| P7 | npm compatibility | Pure-JS packages install and import unmodified | Native Node addons: best-effort. |
| P8 | No runtime dependency on an installed browser or WebView | Absolute | |
| P9 | Install | `npm i -D blitsen` fetches only the host platform's runtime | No Rust toolchain, no compile step, no postinstall build. |
| P10 | Adoption cost | One dev dependency + one script line, zero source changes, for an app already building to static output **and inside the published compatibility profile** | `blitsen doctor` must identify unsupported web APIs and renderer features. |

---

## 9. Size budget as a product commitment

Size remains a product metric, but M0 invalidated the original Phase 2 estimate.

```
S3 Phase 1 prototype, full Bun runtime embedded (Linux x64)
  compiled executable          105,814,144 B  measured

M3 Phase 1 standalone Pong, optimized Rust host (Linux x64)
  compiled executable                    tracked  packages/blitsen/test/metrics/size-baseline.json
  gzip -9                                tracked  (same file; CI fails on >2% growth)

S0 Phase 2 floor, stripped + LTO (Linux x64)
  JSC + Blitz only              52,480,904 B  measured
  gzip -9                       24,076,701 B  measured

Bare app on the shipping host (Linux x64, 2026-08-13, `bun run --cwd packages/blitsen size:phase2`)
  Phase 1 export, same app     131,631,232 B  measured   (144,726,144 B before `strip`)
  Blitsen runtime export        38,090,586 B  measured   3.46x smaller — and the default
  gzip -9                       15,005,053 B  measured   (50.2 MB for Phase 1)
  ── of which ──────────────────────────────
  runtime executable            38,090,000 B  Blitz, Vello, wgpu, winit, tokio, the bridge,
                                              and QuickJS-ng linked in (~1.5 MB of it)
    .text                       26,100,000 B  largest section
  appended application                 640 B  the bare app itself
  engine library, alongside              0 B  there is not one — see LICENSING.md
  ─────────────────────────────────────────
  shipped total                 38,090,586 B  the executable, and that is all

Adopted since the measurement above
  strip = "symbols" on release  13,078,232 B  off both artifacts; it is in [profile.release], so a
                                              checkout's own build weighs what a released one does

Phase 3 levers, measured on the same build
  release-min profile           20,763,000 B  fat LTO + one codegen unit + opt-level=z + strip
                                              43.8% off the stripped runtime executable
  thin LTO, one CGU, opt-level 3
                                32,631,976 B  11.7% off, without the size-first codegen — the
                                              option that does not trade frame time for bytes
  panic = "abort"                   rejected  the native callback boundary turns a panic into a
                                              JavaScript exception; aborting takes the process down
```

**Neither LTO lever is adopted.** Both cost build time, and `opt-level = "z"` buys its 16 MB with
size-first codegen through Blitz's layout and paint — which is P4's budget, and has not been
measured. Adopt either against a frame-time reading, not against this table.

**The engine line is zero because the engine is inside the executable.** That is the whole of the
QuickJS-ng decision ([`spikes/s8`](../spikes/s8/README.md)): MIT rather than LGPL, so it can be
statically linked and dead-stripped instead of shipped beside the binary as a replaceable library.

It is worth recording what the alternative cost, because the comparison is what justified the
swap. JavaScriptCore's shipped total was **68.9 MB** on this machine — and that understated it,
because the 32 MB system library carries no ICU: it links `libicudata` (30,795,392 B),
`libicui18n` (3,455,304 B) and `libicuuc` (2,140,336 B) dynamically, plus GLib and GIO, none of
which exist on a machine that has never had a GTK desktop. A self-contained JSC has to fold that
in, and S0 measured it at **37,980,984 B** for the engine alone ([`spikes/s0`](../spikes/s0/README.md)).
QuickJS-ng contributes about **1.5 MB** to the same total and brings no ICU at all — which is also
why `Intl` is absent from the compatibility profile.

**Still outstanding for P1.** Only Linux x64 is measured — the other five targets have no runner
yet (TECH.md §14) — and no Electron or Tauri build has been measured on the same machine with the
same application, so the comparison remains a claim rather than a number. Both are what the
remaining P1 work is.

The S0 floor already exceeded the old 25–50 MB installed estimate before production services or
application code, and that estimate, along with the derived 20–40 MB Phase 3 estimate, stays
withdrawn. Installed and compressed sizes are always reported separately.

What the numbers above do settle is that the phase reversal was worth making: the same bare
application exports 2.89× smaller, and the 95 MB it drops is Bun's runtime. The Phase 3 profile
takes another 29 MB off the executable and still renders the Pong replay to the same layout and
pixel digests at 60 fps, so it is a measurement rather than a hope. A public numeric claim still
waits on the two outstanding items.

The key architectural consequence, which belongs in the product spec because it defines what
the user installs: **Bun is the toolchain; JavaScriptCore is the runtime.** The exported app
does not need Bun's package manager, test runner, bundler, transpiler, CLI, dev server or
installer. It needs JavaScript execution.

---

## 10. Acceptance milestones

**M0 — Feasibility spike: complete, go/re-scope.** JSC and Blitz compile and link into one
binary. The core Linux architecture survived, while the 25–50 MB budget and unrestricted
drop-in claim did not. See [the M0 decision](M0.md).

**M1 — Hello, DOM: complete.** An `index.html` renders in a native window, a `<script>` runs,
`document.querySelector("#x").textContent = "hi"` visibly updates the screen.

**M2 — Interactive: complete on Linux x64.** Click and keyboard events dispatch to JS listeners
with correct propagation; `requestAnimationFrame` drives a smooth animation; style and class
mutation from JS relayouts correctly. Input enters through the same hit test the native window
uses. See [the M2 acceptance evidence](M2.md).

**M3 — Pong: complete on Linux x64.** A complete playable Pong exists as nothing but `index.html`,
`style.css` and `game.js`, holds 60 fps, and runs from a single exported executable on a machine
with no toolchain installed. **This is the architecture proof** — the point at which the project is
demonstrably real. See [the M3 acceptance evidence](M3.md).

P4 now rests on two wall-clock measurements rather than the game's own readout, which was circular:
headless frame cost is p50 0.809 ms against a 16.7 ms budget with zero frames over, and the
windowed standalone export sustains 60 fps on a real display.

**M3b — Compatible adoption: met.** It was first declared complete on an acceptance application
written in this repository, which tests the export pipeline rather than the adoption claim.
Measured against six applications nobody here wrote — three real ones and three stock `create-vite`
templates — **all six failed**.

After the work that measurement prompted, all six build and render from their own unmodified
`vite build` output, including a full React admin dashboard with Tailwind 4, Radix, TanStack and
Recharts. Zero source changes and no flags, so P10 is met. Remote scripts are still never fetched;
they are skipped, with the rest of the document running, and reported as a warning rather than
blocking the export.

See the [M3b evidence](M3B.md) and [published v0 profile](COMPATIBILITY.md) for the deviations
each application renders with.

**M4 — Ships.** `npm i -D blitsen` resolves the correct runtime on all six platform targets,
`blitsen build` produces distributable artifacts, and a non-trivial third-party app (an editor or
dashboard) is built by someone who is not us.

---

## 11. Risks

| Risk | Impact | Response |
| --- | --- | --- |
| Blitz's DOM is not designed for external mutation at JS frequency | High — undermines the core bridge | Spike first (M1). Upstream contribution may be required; Blitz already intends to support custom widgets and extensibility. |
| Blitz is pre-alpha; CSS coverage may not survive contact with real framework CSS | High — a drop-in exporter that renders real apps wrong is worse than one that refuses them | Golden-image corpus built from actual React/Vue/Svelte output early, not synthetic cases. Treat CSS gaps as upstream contributions. |
| "Drop-in" invites projects the runtime cannot yet render (Three.js, canvas-heavy apps) | Medium — disappointed first impressions | `blitsen doctor` reports unsupported API usage before the user hits it at runtime; capability tiers published prominently. |
| The original Phase 2 size target is unreachable; the measured floor is already 52.48 MB | High — removes the numeric headline | Withdraw the 25–50 MB claim. Measure the complete host, set a platform budget, and use only the fallback positioning: materially below Electron. |
| Partial web platform frustrates users who expect browser parity | Medium | Documented capability tiers; absent APIs absent, never half-working. Positioning never says "browser". |
| Upstream churn in Blitz or Bun | Medium | Pin versions; keep the bridge behind our own interface so upstream shape changes are contained. |
| Effort scale — this is a multi-year systems project | High | Ruthless v0. Pong, then re-evaluate. |
| No sandbox by default | Medium | Explicit product stance: apps are trusted native software. Must be stated prominently, never discovered. |

## 12. Open questions

1. ~~Name~~ — **settled**: Blitsen. Outstanding registration work before anything is published:
   claim `blitsen` on npm and the `@blitsen` scope (the scope matters most — the platform
   packages depend on it), plus `blitsen` on crates.io and a domain.
   *Note the name's proximity to Blitz, the upstream renderer.* That is a fair signal of what
   the project is built on, but it should never be allowed to imply that Blitsen is an official
   DioxusLabs project — worth a line in the README, and worth care if the two are ever
   discussed together upstream.
2. ~~Licence and JSC constraints~~ — **settled:** Blitsen is `MIT OR Apache-2.0` and
   closed-source applications are supported subject to JSC's LGPL-family distribution terms.
   Phase 2 production exports dynamically load a user-replaceable JSC library; static linking is
   reserved for spikes or an export that supplies a complete relinking kit. See `LICENSING.md`.
3. ~~Distribution~~ — **settled**: npm dev dependency with per-platform runtime packages
   (§6, TECH.md §11).
4. **Do multiple windows share one JS context** or get isolated ones?
5. **Is TypeScript first-class?** Mostly moot under the export model — the user's existing
   bundler handles TS before Blitsen sees the output. Still open for the no-build-step path.
6. ~~Where do assets live in the exported binary~~ — **settled: either, embedded by default.**
   `blitsen build --assets embedded` (the default) carries every asset inside the executable,
   which is what makes the single-file distribution claim true. `--assets side-loaded` writes them
   to `<outfile>.assets/` next to the executable for applications whose assets must stay patchable
   after shipping, or whose media is large enough that embedding is wasteful. Assets are
   content-hashed either way and the export is byte-for-byte reproducible (TECH.md §10).
7. ~~Does the dev-server mode ship in v0?~~ — **settled by S7: no; target v1.** Bun can fetch
   and execute a pre-scanned Vite module graph and service its WebSocket transport, but its
   plugin loader does not preserve HTTP module identity or source-map URLs, async resolution is
   unavailable, and the browser-facing HMR client still needs the v1 web-platform surface.
   Directory watching plus full context reload remains the v0 development path.
8. **Native API imports** — `native:dialog` (bare specifier, needs runtime resolver support in
   every bundler) or `blitsen/dialog` (a real npm path that any bundler already resolves)? The
   latter is likely more compatible; the former reads better. Possibly both.

---

*Superseded document: `FIRST.md` (retained in git history at commit `d32f5e3`).*
