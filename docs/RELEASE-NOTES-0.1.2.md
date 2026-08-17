# 0.1.2 — fail closed on the affected 2017 MacBook Pro

Blitsen 0.1.2 corrects the incomplete Intel macOS mitigation in 0.1.1. The CPU rasterizer removed
Vello's GPU workload, but softbuffer presented each CPU-rendered frame by assigning a `CGImage` to
a Core Animation `CALayer`. On `MacBookPro14,3`, that path still produced a client-owned Radeon Pro
560 Metal compute submission, reset the GPU, and terminated WindowServer.

## Fixed

- `MacBookPro14,3` is detected before Blitsen constructs an event loop, AppKit view, window
  renderer, or presentation surface. Window opening returns an actionable error instead of risking
  another forced logout and loss of unsaved desktop work.
- Intel macOS model detection fails closed. If `hw.model` cannot be read, Blitsen refuses windowing
  instead of silently crossing the safety boundary.
- Non-window commands remain available on blocked hardware, including `blitsen doctor` and
  `blitsen build`.
- Other identified Intel Mac models retain the 0.1.1 CPU renderer. Apple Silicon macOS, Linux,
  Windows, and Android retain GPU rendering.

## Evidence and validation

The second incident produced the same diagnostic chain as the first: a `node` process loading
`blitsen.node` owned the first pending Metal command buffer on the Radeon `Compute1` channel,
followed by a GPU reset and WindowServer watchdog termination. The release gate tests the model
blocklist and continues to build and test the Intel macOS artifact on GitHub's Intel runner without
opening the blocked presentation path.

Published macOS artifacts remain unsigned and are not notarised. See
[`docs/RELEASING.md`](RELEASING.md) for the distribution and signing model.
