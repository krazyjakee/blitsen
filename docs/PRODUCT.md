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
| Bare app size | n/a | ~150–250 MB | ~5–15 MB | **target ~30–50 MB** |
| Full OS access | ✗ | ✓ | ✓ | ✓ |
| npm ecosystem | ✓ | ✓ | ✓ | ✓ |
| Adopt without restructuring the project | n/a | partial | partial | ✓ (one dev dependency) |
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
2. **Drop into existing projects; never ask for a rewrite.** Blitsen is an export target that
   consumes static web output, not a framework you start a project in. The developer keeps their
   bundler, their framework and their dev loop. Adoption cost is one dev dependency and one
   script line — and abandonment cost is deleting them.
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

Plus the unlimited escape hatch:

```js
import physics from "./box2d.node";
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

**v0 — proves the architecture**
HTML · CSS · DOM · JS/TS execution · events · `requestAnimationFrame` · `setTimeout`/
`setInterval` · mouse · keyboard · one native viewport element backed by the GPU.

**v1 — makes real apps possible**
`fetch` · `WebSocket` · images · web fonts · audio playback · the first `native:` modules
(dialog, clipboard, window, app).

**v2 — makes real apps comfortable**
`localStorage` · Workers · clipboard events · drag & drop (with real filesystem paths, not
browser `File` abstractions) · gamepads · tray/menu · notifications.

**Later — as demand justifies**
`<canvas>` 2D · WebGL / WebGPU · WebRTC · anything else earning its size.

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
| P1 | Bare exported app size | ≤ 50 MB installed, ≤ 30 MB compressed | Prototype may be 80–120 MB; see §9. |
| P2 | Cold start to first frame | < 500 ms on mid-range hardware | Should beat Electron decisively or the pitch weakens. |
| P3 | Idle RAM, bare app | < 100 MB | |
| P4 | Sustained frame rate | 60 fps for a moderate 2D scene | Measured with the Pong acceptance build. |
| P5 | Platforms, initial | Windows x64, Linux x64, macOS arm64 | Windows is the priority target for size claims. |
| P5b | Platforms, full matrix | win32 x64/arm64, linux x64/arm64, darwin x64/arm64 | One npm platform package each (TECH.md §11). |
| P6 | Render consistency | Byte-identical layout across platforms for the test corpus | The core advantage over WebView-based tools. |
| P7 | npm compatibility | Pure-JS packages install and import unmodified | Native Node addons: best-effort. |
| P8 | No runtime dependency on an installed browser or WebView | Absolute | |
| P9 | Install | `npm i -D blitsen` fetches only the host platform's runtime | No Rust toolchain, no compile step, no postinstall build. |
| P10 | Adoption cost | One dev dependency + one script line, zero source changes, for an app already building to static output | The core adoption claim; measured against real React/Vue/Svelte projects. |

---

## 9. Size budget as a product commitment

Size is the headline claim, so it is tracked as a product metric, not left to the build.

```
Phase 1 — prototype, full Bun runtime embedded
  Bun / JSC runtime            ~60–100 MB
  Blitz + Stylo + Taffy + window ~5–15 MB
  bridge                        ~1–5 MB
  app code                       tiny
  ────────────────────────────────────────
  bare app                    ~70–120 MB

Phase 2 — Bun becomes toolchain only; ship a purpose-built JSC runtime
  JSC + required runtime        ~15–30 MB   (unmeasured)
  Blitz + native rendering       ~5–15 MB   (unmeasured)
  web compatibility layer        ~1–5 MB
  app HTML/CSS/JS               usually <5 MB
  ────────────────────────────────────────
  bare app                     ~25–50 MB

Phase 3 — LTO, stripping, feature gating
  bare app                     ~20–40 MB
```

**These are engineering targets, not measurements.** The Phase 2 and 3 numbers depend on a JSC
+ Blitz build that does not yet exist; establishing the true floor is an early milestone
(TECH.md §15). Public size claims are made only from measured builds.

The key architectural consequence, which belongs in the product spec because it defines what
the user installs: **Bun is the toolchain; JavaScriptCore is the runtime.** The exported app
does not need Bun's package manager, test runner, bundler, transpiler, CLI, dev server or
installer. It needs JavaScript execution.

---

## 10. Acceptance milestones

**M0 — Feasibility spike.** JSC and Blitz compile and link into one binary; measured size
recorded. This either confirms or kills the Phase 2 budget.

**M1 — Hello, DOM.** An `index.html` renders in a native window, a `<script>` runs,
`document.querySelector("#x").textContent = "hi"` visibly updates the screen.

**M2 — Interactive.** Click and keyboard events dispatch to JS listeners with correct
propagation; `requestAnimationFrame` drives a smooth animation; style and class mutation from
JS relayouts correctly.

**M3 — Pong.** A complete playable Pong exists as nothing but `index.html`, `style.css` and
`game.js`, holds 60 fps, and runs from a single exported executable on a machine with no
toolchain installed. **This is the architecture proof** — the point at which the project is
demonstrably real.

**M3b — Drop-in.** An unmodified, existing Vite + React app is exported with nothing but
`npm i -D blitsen` and `blitsen build dist`, and runs. This proves the adoption claim (P10)
independently of the architecture claim, and is the milestone most likely to attract users.

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
| Phase 2 size target proves unreachable; JSC alone is bigger than hoped | High — weakens the headline claim | M0 measures it before anything is promised. Fallback positioning: still far below Electron. |
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
2. **Licence**, and whether JSC's licensing constrains the export model for closed-source apps.
3. ~~Distribution~~ — **settled**: npm dev dependency with per-platform runtime packages
   (§6, TECH.md §11).
4. **Do multiple windows share one JS context** or get isolated ones?
5. **Is TypeScript first-class?** Mostly moot under the export model — the user's existing
   bundler handles TS before Blitsen sees the output. Still open for the no-build-step path.
6. **Where do assets live** in the exported binary — embedded, side-loaded, or either?
7. **Does the dev-server mode ship in v0?** It is the cheapest possible adoption on-ramp
   (`blitsen http://localhost:5173`) and may be worth pulling forward ahead of export.
8. **Native API imports** — `native:dialog` (bare specifier, needs runtime resolver support in
   every bundler) or `blitsen/dialog` (a real npm path that any bundler already resolves)? The
   latter is likely more compatible; the former reads better. Possibly both.

---

*Superseded document: `FIRST.md` (retained in git history at commit `d32f5e3`).*
