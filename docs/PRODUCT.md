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
- **A JavaScript engine** provides execution, the module system, async and timers. The shipped
  runtime links **QuickJS-ng** statically; the Phase 1 addon gets the same services from **Bun**,
  which additionally supplies npm resolution and the native-addon interface.

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
| Bare app size | n/a | ~150–250 MB | ~5–15 MB | **55.8 MB, measured on Linux x64** (§9) |
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

**Both forms work today.** A dev server is a third source of an application's files, beside a
directory and the section inside an export, and everything downstream is unchanged — the document
is on the application origin, modules resolve against it, and `fetch` reads through it. Measured
against a real `vite dev`: React mounts and `[vite] connected.` appears on the console. What is not
implemented is source-map consumption in stack frames; see
[COMPATIBILITY.md](COMPATIBILITY.md#development-your-own-dev-server) for that and for the one Vite
log line it costs to leave `location` on the application origin.

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
`fetch` · `WebSocket` · images · web fonts · audio playback · pointer events (mouse, touch and
pen, with pressure, multi-touch and pointer capture) · the first `blitsen/*` modules
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
drag & drop (with real filesystem paths, not browser `File` abstractions) · gamepads · notification
activation and management. Declarative and runtime tray/menu control, desktop notification
submission and focused native input snapshots have landed; the generated native matrix records the
remaining members rather than treating those modules as all-or-nothing.

**Later — as demand justifies**
WebGL / WebGPU · WebRTC · anything else earning its size. `<canvas>` 2D was on this list and has
landed early (#99).

**Where the v1 line was drawn, stated plainly.** Advanced text input and IME (#103) and
accessibility (#102) are **not v1**. Basic keyboard editing, caret placement and drag selection in
text controls have landed; clipboard editing, undo/redo, composition and `contenteditable` have
not. `<canvas>` was the third entry here, and the sharpest one: the element shipped in the document
and nothing painted inside it, so it was a `doctor` **error** rather than a warning, and an
application that drew was refused at export. It draws now — a full 2D context composited into the
same frame as the DOM — so the error is gone and what is still refused is a GPU context.
[COMPATIBILITY.md](COMPATIBILITY.md#what-v1-is-not) carries the same table.

### The compatibility boundary is the runtime, never the exporter

Because the exporter takes static web output, "does React work?" is never a question about the
exporter. It is always: **does the runtime implement the web APIs that this code path touches?**

| Library | Gated on |
| --- | --- |
| React, Vue, Svelte, Solid, HTMX, jQuery | v0 DOM + events. These are the target of v0. |
| Pixi (canvas renderer), Phaser (canvas renderer) | The 2D context, which landed with #99. |
| Three.js, and anything WebGL-only | WebGL — still the "later" tier. |
| State, utility, data libraries (lodash, zustand, …) | Nothing. Plain JS runs today. |

This is worth stating loudly because it sets honest expectations: a DOM-driven React dashboard
is an early target, while a Three.js scene waits on WebGL. Blitz is also still pre-alpha and
deliberately does not implement the whole browser platform, so the near-term boundary is tighter
than the tier list's endpoint suggests.

`blitsen doctor` reports which web APIs a built bundle references that the runtime does not
provide — so the answer for any given project is mechanical, not guesswork.

### Android is a goal

This document listed "a mobile or console target in the initial phases" as a non-goal until the
cost was measured rather than reasoned about. #139 measured it, and the answer moved: the
workspace cross-compiles under the NDK with nothing in the engine stack objecting, and the engine
has been seen painting a correct frame on Android under both Vello and a CPU rasteriser. Android
is therefore a goal, and the console non-goal stands on its own.

Stated precisely, so the claim is not read as more than it is:

- **Established.** The dependency graph cross-compiles. Vello renders correctly on *Android the
  OS*, against a real Vulkan implementation.
- **Not established.** That Vello holds up across *Android the hardware population*, which is the
  one question an emulator cannot answer and only a physical device can (#151). The public record
  is lopsided rather than merely thin: Mali carries live, unfixed device-loss reports against the
  exact `vello`/`wgpu` pair this workspace ships, while every Adreno correctness report since 2021
  resolved to something else. So the risk is real and it is not evenly distributed.
- **Landed, and not for Android's sake.** Touch and PointerEvents (#145). The host no longer
  discards a touch or a stylus, and the web surface behind them — `pointerType`, `pointerId`,
  `pressure`, multi-touch and pointer capture — is a desktop feature that Android happens to need
  too. Scroll and momentum from a touch drag are deliberately not part of it; see
  [COMPATIBILITY.md](COMPATIBILITY.md#pointer-events).
- **Settled.** The `native:` matrix (#147) — each module has a decision below — and application
  files out of an APK (#144), read in place rather than extracted.
- **Not started.** The entry point (#142), lifecycle (#146) and packaging (#148) are Blitsen's own
  desktop assumptions, not engine limits.

Android does not join P5b. It is a cross-compiled APK/AAB with per-ABI builds and keystore
signing, not a seventh npm platform package an install resolves — see P5c.

**The `native:` modules on Android.** §7's rule — absent rather than approximated — decides most
of them, and "absent" here is a position rather than a gap in the port. What makes it a usable
position is that an absent module's members are `undefined` rather than throwing, so
`if (clipboard.writeText)` selects a fallback; and `blitsen doctor --target android-arm64` reports
every module an application imports that the target does not have, with the reason.

| Module | On Android | Why |
| --- | --- | --- |
| `os` | **Present, whole** | `sysinfo` reads the same `/proc` there. The facts a platform will not give already arrive as `null` by design, so nothing had to change. |
| `window` | **Absent** | winit accepts every setter on Android and discards it, then answers the getter as though the request had never been made: `setDecorations(false)`, then `isDecorated()` saying true, on a platform with no decorations. The monitor list is the one worth naming, because it looks like the survivor — winit enumerates no monitors there, so `monitors()` would report a device with no display. Immersive mode and orientation are the real capabilities and are not these under another name (#146). |
| `clipboard` | **Absent** | `arboard` has no Android backend and does not compile. `ClipboardManager` would not settle it either: Android refuses a read to an unfocused application, and these readers report an empty clipboard as `null`, so the refusal and the empty clipboard would arrive as the same value. A module shaped for that, over JNI. |
| `app` | **Absent** | The directories are the Activity's `filesDir`/`cacheDir`; the XDG variables Android does not set would resolve to a path nothing can write to. `relaunch` has no executable to spawn inside an APK. Single-instance ownership is the platform's own — a second launch is an `Intent` to the process already running, not a command line to hand over. |
| `dialog` | **Absent** | No XDG portal. Already absent off the portal platforms, for its own reasons (#141). |
| `input` | **Present, partial** | Focus-scoped keyboard and pointer snapshots are fed by the same winit events as desktop. Gamepads and device discovery remain absent everywhere. |
| `tray` | **Absent** | Android has no desktop status item or context-menu surface. A persistent Android notification is not a tray icon under another name. |
| `notify` | **Present, partial** | `android-activity` and `jni` bridge the platform `NotificationManager`: a stable default channel, API 33 permission, submission, session-stable replacement IDs and close are implemented. NativeActivity provides no tap/action/dismiss intent callback, so action submissions reject and lifecycle activation remains in #252 rather than exposing inert controls. |

The two that were load-bearing are `clipboard` and `app`: they are what stood between the
workspace and a clean `cargo ndk check` without scaffolding.

### Non-goals

- Rendering arbitrary websites, or passing the web platform test suite.
- A same-origin policy, CSP, or any sandbox for first-party application code.
- Legacy layout quirks, `document.write`, or the quirks-mode parser path.
- An opinionated UI framework, component model, or state library of our own.
- A console target.
- iOS. Android is a goal (below); iOS is not, and nothing in the Android work should be
  read as a step toward it — the packaging, the entry point and the store model all differ.
- Prescribing how anything is drawn inside the app. CSS transforms, a physics library, a WASM
  build of Box2D, a native addon and `<canvas>` are all equally valid.

---

## 8. Product requirements

| # | Requirement | Target | Notes |
| --- | --- | --- | --- |
| P1 | Bare exported app size | **55.8 MB installed, 20.9 MB compressed** — measured on Linux x64, against 131.6 MB for the same app on Phase 1. It was 38.1 MB before `Intl` and SVG added 12.0 MB, Linux tray support added 3.7 MB and `<canvas>` 2D added 1.2 MB; §9 has each trade and what could be given back | S0 disproved the original ≤50 MB target against a design that shipped an engine library alongside; statically linking QuickJS-ng put the shipped total well inside it, and production capabilities have taken it back out — that estimate is withdrawn either way, and this row is a measurement rather than a target. Five targets unmeasured, and no measured Electron or Tauri comparison yet; see §9. Android is not one of the five and never joins this row: an APK is a different artifact, and it is P1b. |
| P1b | Bare APK size, per ABI | **35.2 MB installed, 14.7 MB downloaded** — measured `arm64-v8a`, release, on the same bare application P1 uses (#150) | The budget is one ABI's, because a device installs one ABI and runs it. The two-ABI APK `blitsen build --android` defaults to is **74.6 MB**, and the half of it the device cannot use is carried anyway — so `--android-abi arm64-v8a` is the shipping build and the default set is the one a developer can also put on an emulator. Both numbers are stated because they answer different questions: the APK is what a sideload transfers, and 14.7 MB is what Play's own `bundletool get-size` reports a per-ABI split delivering. Android's vendored OpenSSL is measured rather than asserted, at **≥3.6 MB** of the library, and it does not show up as a premium: at equal architecture the whole APK is *smaller* than the desktop executable. Play measures every limit on the compressed download, and this is 3% of the 500 MB base-module ceiling — size is not the argument for an AAB. Breakdown, method and limits in §9. |
| P2 | Cold start to first frame | < 500 ms on mid-range hardware | Should beat Electron decisively or the pitch weakens. |
| P3 | Idle RAM, bare app | < 100 MB | |
| P4 | Sustained frame rate | 60 fps for a moderate 2D scene | Measured with the Pong acceptance build. |
| P5 | Platforms, initial | Windows x64, Linux x64, macOS arm64 | Windows is the priority target for size claims. |
| P5b | Platforms, full matrix | win32 x64/arm64, linux x64/arm64, darwin x64/arm64 | One npm platform package each (TECH.md §11). Two tiers of evidence: `linux-x64`, `darwin-arm64` and `win32-x64` run the whole suite in CI; `linux-arm64`, `darwin-x64` and `win32-arm64` run a smoke tier — the release artifacts built, the package tests against them, a frame through the native harness, a standalone export and the layout corpus — with the product-behaviour suites and the size gate left to the first three (issue #133). |
| P5c | Platforms, Android | `arm64-v8a` shipping, `x86_64` for emulators | A distinct artifact, not a P5b row: a cross-compiled APK with keystore signing, produced by `blitsen build --android`, not a runtime an install resolves from npm (#148). It is a flag rather than a `--target` value because one APK carries every ABI, and `--target` picks one prebuilt runtime to link. Both defaults ship in one artifact; `armeabi-v7a` builds on request and is unproven. Signed with the Android debug key unless `--android-keystore` names one, and the build says on every run that a debug-signed APK is not distributable. Compiled with `cargo ndk` against the `crates/blitsen-android` cdylib #142 landed, and packaged with the SDK's own `aapt2`, `zipalign` and `apksigner`. `cargo apk` was tried and dropped for two independent reasons: it cannot store assets uncompressed on a release profile, which is #144's one packaging ask, and it cannot package an entry crate that is itself the cdylib, which is the shape #143 proved links. Every entry in the APK is stored, so assets are read in place and the `.so` is mapped rather than extracted at install. `minSdk` is 26 rather than 24, because the audio backend reaches `libaaudio` and the NDK ships it from 26 — a floor found by building, not by reading. **Not an AAB**, so not a Google Play upload — nothing on this path emits one. A Gradle backend is not what that would take, though, and #150 priced it rather than assuming it: an AAB of this workspace's own libraries was built with `aapt2 link --proto-format` and `bundletool`, two of Google's own tools, on the JDK `apksigner` already runs on. What is missing is a path through the CLI, and no split out of that bundle has been installed on a device. The NDK is a prerequisite the CLI detects and never installs: an Android build is a cross-compile, so P9's "no toolchain" does not reach it. Its size budget is its own and is **P1b** — P1 is a `linux-x64` figure and does not transfer. **Evidence: a smoke tier, and a thinner one than P5b's** (#149). CI cross-compiles the entry point for both default ABIs, checks each `.so` is the architecture it claims to be and exports `android_main`, resolves the third-party notices an Android artifact owes — which nothing had ever done for an Android triple — and then packages an APK with `blitsen build --android` itself and reads the archive back: both ABIs present, every entry the build wrote stored, the notices inside it, the certificate it was signed with. It runs the command rather than a re-implementation of it, per #133's line that a target's job builds exactly what the target's artifact is; layout conformance, determinism, the size gate and the product-behaviour suites do not change with the target and stay where they are. What CI does *not* do is boot an emulator, and that is the whole of the gap now that #148 has made an engine-bearing APK possible. The harness is written and runnable — `bun run --cwd packages/blitsen test:android --apk <path> --package <id>`, which installs, launches and compares the framebuffer against a control frame taken before the install — and the GPU mode it will run under is decided: `-gpu lavapipe`, because #139 measured `swiftshader_indirect` killing the emulator at wgpu initialisation and a hosted runner has no GPU for `-gpu host`. Two things are unknown rather than deferred: whether lavapipe carries Vello's compute pipelines, which nobody has tried, and whether a standard hosted runner has the KVM the emulator wants, which is user-reported and undocumented. |
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
  gzip level 9                           tracked  (same file; CI fails on >2% growth)

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

Bare APK, the same application (2026-08-16, 07a0ed1, `bun run --cwd packages/blitsen size:android`)
  arm64-v8a  libblitsen_android.so   35,160,952 B  measured  release; llvm-strip finds a further 512
             signed APK              35,172,930 B  measured  library and assets stored, not deflated
             per-ABI split delivers  14,680,852 B  measured  bundletool get-size total, Play's own sum
  x86_64     libblitsen_android.so   39,426,560 B  measured
             signed APK              39,440,959 B  measured
             per-ABI split delivers  15,552,431 B  measured
  both ABIs in one APK               74,605,200 B  measured  what `blitsen build --android` defaults to
  the application itself                    391 B  measured  index.html and the asset listing
  ── inside the arm64-v8a library ──────────
  every sized symbol                 23,494,991 B  measured  66.8% of the file; the method sees no more
  vendored OpenSSL                    3,631,074 B  measured  a floor: 12,530 of libcrypto.a + libssl.a's
                                                             18,283 symbols, sized in the linked object
  QuickJS-ng                            918,068 B  measured  a floor: 1,324 of libquickjs.a's 1,522
  ─────────────────────────────────────────
  linux-x64, the same commit         39,470,504 B  measured  `size:phase2`, for the comparison below
  x86_64 APK against it                 -29,545 B  measured  the entire Android premium, at equal
                                                             architecture, vendored OpenSSL included
  the whole record                        tracked  packages/blitsen/test/metrics/android-size.json
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
QuickJS-ng contributes about **1.5 MB** to the same total and brings no ICU at all. `Intl` was
absent from the compatibility profile for exactly that reason until #237, which supplies it from
ICU4X instead — at a measured 12 MB rather than JSC's 36, and only because the engine brings none.

### Android: a different artifact, so a different budget (P1b)

**The budget is one ABI's.** A device installs an APK, picks the one ABI it can run, and ignores
the rest; nothing about carrying a second ABI reaches the user except the bytes. So the figure that
describes a user is **35.2 MB for `arm64-v8a`**, and the figure that describes what
`blitsen build --android` produces today is **74.6 MB**, because its default ABI set exists so that
one artifact also installs on an emulator (P5c). Both are above. `--android-abi arm64-v8a` is the
shipping build, and the 39.4 MB it drops is the emulator's, not any phone's.

**A sideload transfers the APK; Play would transfer far less.** Every Google Play limit is measured
on the *compressed download*, not the installed archive — 500 MB for a base module, 100 MB for the
legacy signed-APK route, with a non-blocking warning to users on mobile data above 200 MB. The APK
above stores its library uncompressed, which `android:extractNativeLibs="false"` requires and which
is why nothing is extracted into `/data` at install; that is a deliberate trade of download for
device footprint, and it costs **20.5 MB** on `arm64-v8a` — the same archive at `zip -9` is
14,651,970 B. `bundletool get-size total`, which is Play's own arithmetic, says a per-ABI split
delivers **14,680,852 B**. Two methods, 0.2% apart.

**`native-tls-vendored` is real, is Android's alone, and costs less than it looks.** It is not
ours: `blitz-net` asks `reqwest` for it under `cfg(target_os = "android")` and nowhere else, so the
desktop runtime links `libssl.so.3` and `libcrypto.so.3` from the system — zero bytes in the
artifact, and a dependency on a library Android does not offer an NDK application. Vendoring is
therefore not a choice to reverse; the alternative is rustls, which is a different TLS stack and a
different decision. What it costs is **≥3,631,074 B** of the `arm64-v8a` library, about 10% of it.

**How that was attributed, and what the method cannot see.** The symbol *names* come from the
`libcrypto.a` and `libssl.a` that `openssl-src` built for the target; the *sizes* come from the
linked shared object, so what is counted is what survived the link rather than what was compiled.
It needs a symbol table, which `[profile.release]` strips, so the count is taken on `release-dbg` —
the same profile with its symbols back — and the two builds' `.text` differ by 53,096 B, 0.24%.
Every sized symbol in that object totals 23,494,991 B against a 35,160,952 B file, so the method is
blind to a third of it: unnamed `.rodata`, `.eh_frame`, `.gcc_except_table` and the relocation
tables carry bytes no symbol is named for. **3.6 MB is a floor, not a share.** QuickJS-ng comes out
at 918,068 B by the same method, consistent with the ~1.5 MB recorded above for the desktop build.

**And the premium is not where it looks.** At equal architecture the Android artifact is *smaller*:
the `x86_64` APK is 39,440,959 B against 39,470,504 B for the `linux-x64` export from the same
commit — **29,545 B less**, complete with the 3.6 MB of OpenSSL the desktop build does not carry.
Only that net is measured. The obvious reading is that what Android drops pays for what it vendors
— winit's X11 and Wayland backends, `arboard`, the XDG portal and every `native:` module §7 records
as absent are all out of its graph — but nothing here priced those omissions, so that sentence is
an inference and the 29,545 B is the measurement. What it does settle is the direction: the
vendored-OpenSSL cost is a real line in the breakdown and it is not a reason the APK is large.

**An AAB is not a size decision, and it is not a Gradle decision either.** At 14.7 MB the compressed
download is 3% of Play's base-module ceiling and 7% of the threshold that warns a user on mobile
data, so nothing about size forces the format. What an AAB buys is a Play listing, and #148
recorded that as needing a Gradle backend because `cargo apk` emits no bundle. That second half is
wrong, and it was cheaper to disprove than to argue: `aapt2 link --proto-format` writes the
protobuf module a bundle is made of and `bundletool build-bundle` assembles it — both Google's own,
on the JDK `apksigner` is already a wrapper around — and the 30,220,813 B bundle whose splits are
tabled above was produced that way, from this workspace's own libraries, with no Gradle, no AGP and
no Android project checked in. `size:android --bundletool <jar>` reproduces it. So the price of an
AAB is `bundletool` and a CLI path, not a build system. **It is still not adopted**: no split out of
that bundle has been installed on a device or run, `bundletool` is a downloaded jar the repository
does not vendor, and P5c's artifact remains the APK until somebody wants a Play listing.

**What P1b does not cover.** One ABI is unmeasured — `armeabi-v7a` builds on request and nothing
here built it. The application is the bare one, so nothing says what a real application's assets
add. `release-min` was not tried against Android, so the Phase 3 levers above are desktop numbers.
And the APKs measured here were assembled by `size:android` along `spikes/s9`'s proven path rather
than by `blitsen build --android`, whose entry point was mid-reconciliation (#148) when this was
taken; the library in them is the product's, but the packaging step is the script's.

One number above is not Android's and should be read carefully: the `linux-x64` export measured
**39,470,504 B** on 2026-08-16, against the **38,090,586 B** recorded on 2026-08-13 in the block
above. That is 3.6% of desktop growth in three days that nothing in this section explains, and it
is P1's to explain, not P1b's — the figure is here only because a comparison across architectures
has to come from one commit.

**The Intl and SVG work is +12.0 MB, and the budget was moved to take it.** Issues #236–#238 added
CLDR through ICU4X, the platform time-zone database through `jiff`, and the SVG stack the Blitz pin
bump turned on. Measured on `linux-x64`, the same way every other figure here was: **50.9 MB
installed against the previous 38.8 MB (+30.8%), and 19.2 MB gzipped against 15.3 MB (+24.6%)**.
The size gate failed on both, which is what it is for — "every megabyte added to the export has to
be an argued-for decision" — and the argument was made and accepted when the features landed
rather than waved through: what it buys is the whole of `Intl` for every CLDR locale with nothing
to configure, and SVG that paints.

**Native Linux tray support is +3.7 MB installed and +1.4 MB compressed.** The StatusNotifierItem
implementation brings its D-Bus protocol stack into the standalone runtime so a tray icon does not
depend on GTK or AppIndicator development libraries being present on the user's system. That cost
exists even when an individual application does not configure a tray, because the runtime is one
prebuilt binary. The baseline was re-recorded at **54.6 MB installed and 20.5 MB compressed**, so
the gate measures drift from the accepted capability cost rather than staying red.

**`<canvas>` 2D is +1.2 MB installed and +0.4 MB compressed.** Three things account for it, and
none is the drawing itself — recording a display list is the scene the renderer already builds.
`skrifa` reads glyph outlines, which is what `measureText` reports as its actual bounding box; the
CPU rasteriser answers the readbacks the specification demands a synchronous answer for
(`getImageData`, `toDataURL`, `toBlob`, and one canvas drawn into another); and the PNG and JPEG
encoders are what `toDataURL` hands back. The baseline was re-recorded at **55.8 MB installed and
20.9 MB compressed** so the gate measures drift from the accepted cost rather than staying red.
The rasteriser is the piece that could be given back — it exists for the readback paths, and an
application that never reads a canvas back never reaches it — but it is linked either way, because
the runtime is one prebuilt binary.

What can still be traded, if the number later matters more than the coverage: currency *names*
(`currencyDisplay: "name"`), localised time-zone names (`timeZoneName`, and
`timeStyle: "full"`/`"long"`), and collation are the three largest pieces of data linked, and each
is a feature that could go rather than a saving to be found in the build.

Worth reading beside the JavaScriptCore comparison above: a self-contained JSC was measured folding
in **36 MB** of ICU for the same class of capability, and this is 12 MB for it.

**Still outstanding for P1.** Only Linux x64 is measured — the other five targets build and test in
CI (TECH.md §11) but have no committed size baseline, so the gate reports on them and gates nothing
(TECH.md §14) — and no Electron or Tauri build has been measured on the same machine with the same
application, so the comparison remains a claim rather than a number. Both are what the remaining P1
work is.

The S0 floor already exceeded the old 25–50 MB installed estimate before production services or
application code, and that estimate, along with the derived 20–40 MB Phase 3 estimate, stays
withdrawn. Installed and compressed sizes are always reported separately.

What the numbers above do settle is that the phase reversal was worth making: the same bare
application exports 2.89× smaller, and the 95 MB it drops is Bun's runtime. The Phase 3 profile
takes another 29 MB off the executable and still renders the Pong replay to the same layout and
pixel digests at 60 fps, so it is a measurement rather than a hope. A public numeric claim still
waits on the two outstanding items.

The key architectural consequence, which belongs in the product spec because it defines what
the user installs: **Bun is the toolchain; Blitsen's own runtime is what ships.** The exported app
does not need Bun's package manager, test runner, bundler, transpiler, CLI, dev server or
installer. It needs JavaScript execution, and it carries an engine that does nothing else.

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
| "Drop-in" invites projects the runtime cannot yet render (Three.js, WebGL-heavy apps) | Medium — disappointed first impressions | `blitsen doctor` reports unsupported API usage before the user hits it at runtime; capability tiers published prominently. |
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
2. ~~Licence and engine constraints~~ — **settled, and then settled more cheaply:** Blitsen is
   `MIT OR Apache-2.0` and closed-source applications are supported. The first answer accepted
   JavaScriptCore's LGPL-family terms, which meant a dynamically loaded, user-replaceable engine
   library and a relink flow; QuickJS-ng is MIT, so the shipped runtime links it statically and
   the most demanding term left in the tree is Stylo's file-level MPL-2.0. See `LICENSING.md`.
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
