# Blitsen

> Write an app in HTML, CSS and TypeScript. Ship a native executable. No browser included.

Blitsen is an experimental native runtime for applications built from static HTML, CSS and
JavaScript output. It hosts a JavaScript engine directly and pairs it with
[Blitz](https://github.com/DioxusLabs/blitz)'s native HTML/CSS renderer. It does not embed Chromium, and it does not use the operating system's
WebView.

**The project is pre-alpha.** All six targets build and test in CI — Linux, macOS and Windows on
x64 and arm64 — with the full suite on `linux-x64`, `darwin-arm64` and `win32-x64` and a smoke tier
on the other three. Nothing is published to npm yet, size and startup have a baseline on Linux x64
alone, and every harness is headless: no Blitsen window has been watched paint on Windows or macOS
outside CI ([issue #123](https://github.com/krazyjakee/blitsen/issues/123)).

## What it is not

**Blitsen is not a browser and does not aspire to be one.** It is a native application runtime that
happens to use the web platform as its UI model. Three consequences, none of them accidental:

- **Blitsen will render less of the web than a browser does.** That is the trade: you get a renderer
  you ship and control, at a size that is not absurd, and you lose specification coverage. An
  application targeting Blitsen is authored against Blitsen, not ported blind from the web. The
  boundary is published as [capability tiers](docs/COMPATIBILITY.md), generated from the runtime
  itself rather than hand-maintained, and `blitsen doctor` reports what a bundle uses that the
  runtime lacks before you hit it at runtime.
- **There is no sandbox by default.** An application is trusted native software, not an untrusted
  document. No same-origin policy, no permission prompts.
- **It is not for rendering arbitrary third-party web content.** Use a browser engine for that.

An unimplemented API is *absent* — the property does not exist — so feature detection works. Never
a stub that resolves to nothing, never a silent no-op. That is enforced rather than reviewed: the
API manifest is parsed out of the runtime source, and a test asserts every API the manifest calls
absent is genuinely `undefined` against a real bridge context.

## Where it is

**Developing against your own dev server.** `blitsen http://localhost:5173` opens what Vite (or
anything else serving over HTTP) is serving, rather than a directory of output: modules load as
they are served, hot reload keeps its channel open, and the window is the tab. Measured against a
real `vite dev` — React mounts and `[vite] connected.` appears — and gated headlessly by
`bun run --cwd packages/blitsen test:proxy`. Source-map consumption in stack frames is the one
part not implemented; see the [compatibility profile](docs/COMPATIBILITY.md#development-your-own-dev-server).

**Rendering real applications.** Six applications written by other people — a React admin dashboard
using Tailwind 4, Radix, TanStack and Recharts; a Vue 3 app with vue-router and Pinia; a Svelte
game; and the three stock `create-vite` templates — all render from their own unmodified
`vite build` output, and all six export with nothing but a dev dependency and a script line. All
six failed when first measured. See the [M3b evidence](docs/M3B.md).

![Shadcn Admin rendered by Blitsen](docs/shadcn-admin.png)

*[Shadcn Admin](https://github.com/satnaing/shadcn-admin) (MIT), unmodified, rendered without a
browser engine. The empty chart panel is Recharts SVG —
[tracked upstream](https://github.com/DioxusLabs/blitz/issues/448).*

**The architecture proof.** [`examples/pong`](examples/pong) is a two-player Pong app that is
nothing but `index.html`, `style.css` and `game.js`, and runs from a single exported executable on a
machine with no toolchain installed. Frame cost is 0.809 ms median against a 16.7 ms budget, and the
windowed export sustains 60 fps. See the [M3 acceptance evidence](docs/M3.md).

![Pong running in Blitsen](docs/pong.gif)

*Every frame is HTML and CSS laid out by Blitz and mutated from JavaScript — the paddles, the ball
and the scoreboard are ordinary DOM nodes. The recording comes from the same harness the acceptance
gate asserts on, so it cannot drift from what the tests verify.*

**Past what a browser can answer.** [`examples/hardware`](examples/hardware) is a CPU-Z-shaped
report on the machine it is running on — processor and per-thread load, memory and swap, every
mounted volume, kernel and boot time — read through [`blitsen/os`](docs/COMPATIBILITY.md#native-modules).
None of it has a web spelling: the closest the platform comes is `navigator.hardwareConcurrency`,
one deliberately coarsened number. It is three files with no build step, and it runs with
`bun run --cwd packages/blitsen example:hardware`.

Input, animation and restyle are proven together by [`examples/interactive`](examples/interactive),
whose gate drives the document through the same coordinate hit test the native window uses
([M2 evidence](docs/M2.md)). Phase 2 — the runtime hosting its own JavaScript engine instead of
running inside Bun — **is what a build produces now**, and Bun is linked only by an application
carrying a `.node` addon ([migration note](docs/MIGRATION.md)). The
[acquisition decision](docs/JSC.md) chose JavaScriptCore and was superseded by
[`spikes/s8`](spikes/s8/README.md), which measured QuickJS-ng behind the same engine-neutral trait:
120 golden frames pixel-identical, 59.6 fps windowed, MIT rather than LGPL, and statically linked.

## Size

Every size figure in this project comes from a measured build, never an estimate. The tracked
baseline lives in
[`packages/blitsen/test/metrics/size-baseline.json`](packages/blitsen/test/metrics/size-baseline.json)
and CI fails on growth beyond 2%. An export links Blitsen's own runtime rather than a copy of Bun,
which took the standalone Pong build from 144.7 MB to **38.1 MB** on Linux x64 — and that is the
whole download, because the JavaScript engine is statically linked rather than shipped beside it.
An application still links Bun when only Bun can run it, which now means one thing: it carries a
`.node` addon ([migration note](docs/MIGRATION.md)). The original 25–50 MB target was withdrawn
when the [M0 measurement](docs/M0.md) showed it was unreachable against a design that shipped an
engine library alongside; the shipped total is now inside it, which is a result of the engine
choice rather than a walk-back of the measurement.

**An export carries the notices it owes.** The third-party notices are generated from the
dependency graph the runtime was built from, shipped inside the platform package, and embedded in
the executable — `./MyApp --licenses` prints them back out of the artifact. The
[LICENSING.md](docs/LICENSING.md) acceptance gate is an automated test
(`bun run --cwd packages/blitsen test:licensing`): it builds a real export, reads the notices out
of it, checks every linked package and every licence text against what `cargo` resolved, and
repeats the check after signing. An export that carries none says so on the build line, which is
what a Phase 1 export — the one that carries a copy of Bun — still gets.

## Documentation

[Product specification](docs/PRODUCT.md) · [Technical specification](docs/TECH.md) ·
[Compatibility profile](docs/COMPATIBILITY.md) · [Licensing](docs/LICENSING.md) ·
[Known Blitz gaps](docs/BLITZ-GAPS.md) · [M0 decision](docs/M0.md)

## Attribution

Blitsen is an independent project built on Blitz. **It is not an official DioxusLabs project and is
not endorsed by DioxusLabs** — the name's proximity to Blitz reflects what it is built on, nothing
more. Rendering gaps found here are [reported upstream](docs/BLITZ-GAPS.md) with reproductions.

## Licence

Blitsen source is dual-licensed under Apache-2.0 or MIT. Exported applications also contain
third-party components with their own terms — the JavaScript engine is MIT, and the most demanding
term in the tree is Stylo's file-level MPL-2.0. Closed-source applications are supported. An export
that carries a `.node` addon links the Bun host instead and inherits LGPL obligations through it.
Read [docs/LICENSING.md](docs/LICENSING.md) before distributing an application.
