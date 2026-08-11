# Blitsen

Blitsen is an experimental native runtime for applications built from static
HTML, CSS, and JavaScript output. It combines JavaScriptCore with Blitz's native
HTML/CSS renderer; it does not embed Chromium or an operating-system WebView.

The project is pre-alpha. Its Linux x64 feasibility review concluded **go,
re-scoped**; Windows and macOS validation is deferred. See the
[M0 decision](docs/M0.md), [product specification](docs/PRODUCT.md),
[technical specification](docs/TECH.md), and
[licensing/export requirements](docs/LICENSING.md).

Input, animation and restyle are proven together on Linux x64 by
[`examples/interactive`](examples/interactive), whose gate drives the document through the same
coordinate hit test the native window uses. See the [M2 acceptance evidence](docs/M2.md).

The Linux x64 architecture proof is now complete: [`examples/pong`](examples/pong) is a
three-file, two-player Pong app that runs at 60 Hz from a single Phase 1 executable. See the
[M3 acceptance evidence](docs/M3.md). Phase 1 exports are not yet cleared for redistribution.

![Pong running in Blitsen](docs/pong.gif)

Every frame above is HTML and CSS laid out by Blitz and mutated from JavaScript — the paddles,
the ball and the scoreboard are ordinary DOM nodes. The recording is produced by the same
document-animation harness the acceptance gate asserts on, so it cannot drift from what the
tests verify.

The compatible-adoption proof is also complete on Linux x64: an untouched Vite + React production
bundle passes [`blitsen doctor`](docs/COMPATIBILITY.md), exports as one executable, mounts React,
and handles delegated input with no toolchain on `PATH`. See the [M3b evidence](docs/M3B.md).

Phase 2 is underway. The [JavaScriptCore acquisition decision](docs/JSC.md) pins Bun's WebKit
lineage while keeping the production engine dynamically replaceable behind Blitsen's own ABI.

Blitsen is an independent project built on Blitz. It is not an official
DioxusLabs project and is not endorsed by DioxusLabs.

## Licence

Blitsen source is dual-licensed under Apache-2.0 or MIT. Exported applications
also contain third-party components with their own terms. In particular,
JavaScriptCore includes LGPL-family code: closed-source applications are
supported, but exporters must preserve notices/source offers and a recipient's
ability to replace or relink JSC. Read [docs/LICENSING.md](docs/LICENSING.md)
before distributing an application.
