# Phase 2 JavaScriptCore acquisition

**Decision date:** 2026-08-10
**Decision:** build a pinned Bun WebKit revision in Blitsen's release matrix, own the small Rust
ABI layer, and dynamically load the resulting JavaScriptCore library in production.

This settles [issue #84](https://github.com/krazyjakee/blitsen/issues/84). It chooses where the
engine artifacts and patches come from; it does not import Bun's runtime. Bun remains the build
tool in Phase 2, while Blitsen supplies module loading, scheduling, web APIs, and the DOM bridge.

## Decision matrix

| Option | Build time and cross-compilation | Patch and upgrade cadence | Size and licensing | Decision |
| --- | --- | --- | --- | --- |
| Vendor upstream WebKit/JSC directly | A full engine build is the slow path; JSCOnly avoids the browser but not WebKit's compiler and generated-source pipeline. WebKit publishes no cross-platform binary release, and Linux, Apple, and Windows have distinct native prerequisites. Every release needs native builders rather than arbitrary host-to-target cross-compilation. | Smallest downstream patch delta, largest release-engineering and security-tracking burden. Upstream release and advisory cadence differs by port. | Same engine floor in principle. We would still need dynamic artifacts and replacement instructions for LGPL compliance. | Rejected for now. Revisit only if Bun's fork diverges from the public C API or stops shipping a required target. |
| Adopt an existing Rust binding | The Rust wrapper itself builds quickly, but merely moves the engine build elsewhere: `rusty_jsc` requires Apple's framework or system JavaScriptCoreGTK, while `rust_jsc` downloads prebuilt static archives. Neither has a Windows build path, so neither solves the six native builds or cross-target release matrix. | Saves wrapper code but does not solve artifact production, six-target testing, engine pinning, or module-loader patch ownership. Its crate and engine-archive release cadence becomes part of Blitsen's upgrade cadence. | System JavaScriptCoreGTK makes installed size and version host-dependent; downloaded `rust_jsc` archives force the static-link compliance path. | Rejected. Small safe-wrapper ideas may be reused, but Blitsen owns its ABI boundary. |
| Reuse Bun's WebKit build lineage | Engine builds remain heavyweight native CI work, but Bun's autobuilds make ordinary development an artifact download/cache rather than a WebKit rebuild. The S0 revision publishes regular and LTO artifacts for Linux x64/arm64, macOS x64/arm64, and Windows x64/arm64. Shared release artifacts are still built and tested natively per OS; this is not arbitrary cross-compilation. | Carries Bun's JSC patches, which Blitsen already exercises in Phase 1. Pin exact revisions and checksums; upgrade deliberately with the dual-host suite rather than following `main`. | S0 measured the static Linux JSC-only floor at 37,980,984 B and the combined JSC+Blitz floor at 52,480,904 B. Production builds dynamically load a replaceable library as required by `LICENSING.md`. | **Chosen.** It is the only evaluated source already producing all six target/architecture combinations and already proven with this renderer. |

Sources checked for this decision:

- [WebKit's ports documentation](https://docs.webkit.org/Ports/Introduction.html) identifies
  JSCOnly and explains that WebKit has no cross-platform binary releases.
- [WebKit's Windows build documentation](https://docs.webkit.org/Ports/WindowsPort.html) shows the
  separate native Windows toolchain and prerequisites.
- [`wasmerio/rusty_jsc`](https://github.com/wasmerio/rusty_jsc) and its
  [`sys/build.rs`](https://github.com/wasmerio/rusty_jsc/blob/main/sys/build.rs) define its wrapper
  and platform linkage.
- [`kevincaicedo/rust-jsc`](https://github.com/kevincaicedo/rust-jsc) documents its supported
  targets, static archive download, and patched WebKit requirement.
- [The pinned Bun WebKit autobuild](https://github.com/oven-sh/WebKit/releases/tag/autobuild-447082ab6897278727b44e1ba3c326ae6e1504c3)
  is the artifact lineage already measured by S0.

## Artifact and ABI contract

1. Release CI builds on each target OS. Linux artifacts are produced for x64 and arm64, macOS for
   x64 and arm64, and Windows for x64 and arm64. A target is not advertised until its native smoke
   test passes.
2. The WebKit revision, Blitsen patch revision, compiler image, artifact checksum, required notices,
   and source offer are recorded together. An engine update is an explicit reviewed change.
3. The shipped runtime opens the JSC shared library at process start. `BLITSEN_JSC_LIBRARY` may
   point at an ABI-compatible replacement; packaging and signing must not disable that override.
4. `crates/blitsen-jsc` owns only the symbols Blitsen uses. Higher bridge crates continue to see
   the engine-neutral `JsEngine` trait. No Bun API enters that crate.
5. The JavaScript context remains process-lived until the teardown assertion recorded by S0 is
   understood. Blitsen still unloads nothing while JSC values are live.

The current acquisition smoke test dynamically loads a compatible host library and evaluates a
script through the public C API:

```sh
BLITSEN_JSC_LIBRARY=/path/to/libJavaScriptCore.so \
  cargo run -p blitsen-jsc --example evaluate -- "6 * 7"
```

On Linux development machines, the loader also probes installed JavaScriptCoreGTK 6.0 and 4.1
libraries. Those fallbacks are for development only; exported applications carry the pinned
Blitsen engine artifact.

## Embedded engine progress

`crates/blitsen-jsc` now implements every method in the engine-neutral `JsEngine` trait over a
dynamically resolved C API. The host conformance test exercises values and coercion, properties and
globals, arrays and typed arrays, Rust callbacks, native classes and instance data, weak references,
script exceptions, and Promise microtask checkpoints against a real shared JSC library.

The audit found no Bun or Node-API access in `blitsen-core`, `blitsen-js`, `blitsen-dom`,
`blitsen-blitz`, or `blitsen-platform`. The Phase 1 DOM installer and browser bootstrap remain in
`blitsen-node`; Phase 2 needs an equivalent adapter, but that code has not escaped into either
engine-neutral trait.

Issue #85 remains open for two dependent acceptance boundaries:

- Module evaluation uses Bun WebKit's `JSLoadAndEvaluateModuleFromSource` extension. The dynamic
  loader detects it without making it a requirement for system-JSC conformance tests; the resolver
  and shipped module graph are issue #86.
- Bare JSC has no host event loop. Promise checkpoints work at C API boundaries, but timer/I/O turns
  and the end-of-job boundary that makes weak targets collectible belong to issue #87.

The global context and its library intentionally remain process-lived. S0 found that releasing the
pinned Bun context without Bun's host initialization asserts during atom-table teardown; unloading
the shared library while that context exists would be invalid for the same reason.
