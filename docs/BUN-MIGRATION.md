# Bun runtime decision

Blitsen is moving to one desktop runtime: Bun hosts the JavaScript application and
loads Blitsen's renderer and desktop integration through its Node-API addon.
Windows, macOS and Linux use the same architecture. Bun also runs application
workers. QuickJS is no longer a shipping runtime or an export option.

The purpose is to reduce the standard-library behaviour Blitsen maintains. Bun owns
JavaScript execution, internationalisation, text encoding, standard body and URL
objects, networking, structured cloning and worker execution. Blitsen owns its DOM
bindings, rendering, desktop integration, application assets and document lifetime.

Application code may use Bun's `node:*` and `bun:*` modules. Database work should
run in a worker when it can block, including synchronous `bun:sqlite` transactions.
Keep a complete read/check/write transaction in one worker operation. Filesystem
access uses `node:fs/promises`; bounded reads and atomic replacements are application
or ecosystem helpers, not another Blitsen filesystem API. This is the product
direction for issues #439 and #440. Strong process containment and crash recovery
in #441 remain a separate supervisor concern: ordinary spawning is not containment.

Android support is withdrawn and iOS is not supported. Mobile may be reconsidered
when Bun provides supported Android/iOS runtime ports. Blitsen will not maintain a
second JavaScript runtime to keep a mobile target alive. A future Bun port would
still need Blitsen integration and qualification before becoming a supported target.

## Resource budget

The migration trades a larger executable and potentially higher memory use for
less runtime code and a broader ecosystem. Historical QuickJS size gates are not
valid Bun baselines. Record new baselines from the resulting release artifacts;
do not infer linker savings from source lines removed.

A Linux x64 release build measured on 2026-09-14 with Bun 1.3.14 produces a
144.3 MB bare executable, including native dependency notices, or 54.7 MB with gzip
level 9. Its included native addon is 48.7 MB. Before removing duplicated
runtime services, the same Bun version with the 0.2.6 addon measured 154.3 MB / 58.3 MB gzip.
The previous QuickJS executable was 60.8 MB / 22.8 MB gzip. These are local reference
measurements, not guarantees for other applications or platforms.

Run `bun run --cwd packages/blitsen size:runtime --out runtime-size.json` to reproduce
the release size measurement. The comparison scripts use the same bare HTML for
Electron and Tauri; their platform dependencies and packaging boundaries differ.

Three Linux x64 headless runs of the migrated release interpreter (800×600 bare document,
two seconds after readiness, `/proc` RSS sampling) measured 128.9–129.7 MB peak and
128.3–128.5 MB near the end. The earlier tiny headless QuickJS comparison was about
44 MB peak; workloads and window/GPU resources can change these figures substantially.
These measurements are not a windowed idle-memory baseline.

## Compatibility boundary

Bun's standard APIs follow the pinned Bun release. They are not restricted to the
former QuickJS subset. Rendering and desktop APIs still follow Blitsen's published
capability matrix. Both kinds of capability must work in packaged exports and
application UI test sessions, not only during development.

Document reloads must release document-owned timers, workers and network activity.
Native callbacks and painting remain on the window thread; Bun owns the outer
event loop, so native code must return to it rather than attempting to run a nested
Bun event loop. No JavaScript callback may interrupt another callback or a paint.

## Implementation and retained boundaries

The native addon has no QuickJS dependency. QuickJS and the old language shims remain only as
Rust unit-test fixtures for engine-neutral DOM regression coverage; no production build enables
them. The native Bun harness verifies the shipping surface independently.

Exports embed a Bun launcher, native addon and collected application files. The separate
`blitsen-runtime` diagnostic interpreter is now compiled with Bun from that same launcher.
`blitsen/test` uses bounded native calls with an asynchronous Bun command loop. Runtime types
include the pinned `@types/bun` definitions.

`EventSource` stays in Blitsen because the qualified Bun 1.3.14 runtime does not provide it.
Desktop Web Storage keeps Blitsen's application identity and persistence semantics. Desktop
sidecars, dialogs, notifications, native input and rendering retain their existing contracts.

Bun owns standard-library compatibility. In particular, `Request` needs an absolute URL,
WebSocket binary types should be explicit, and worker globals are Bun's. Document modules use
Bun's synchronous `require` path: put asynchronous startup in an async function instead of
module-level `await`. Workers may use top-level `await`. Bare dependencies still need to be
bundled or included in exported assets. Applications remain trusted desktop code with full
process access, not browser-sandboxed content.

The upstream Bun license inventory is included alongside the native notices. Neither that
inventory nor the Cargo notice audit constitutes complete redistribution clearance; see
[LICENSING.md](LICENSING.md) for the Bun/JavaScriptCore source and relinking obligations.
