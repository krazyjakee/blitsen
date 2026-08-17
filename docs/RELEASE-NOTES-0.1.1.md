# 0.1.1 — safe rendering on Intel macOS

> **Known safety regression:** the CPU rasterizer still presented through a Core Animation
> `CALayer`. On `MacBookPro14,3`, Core Animation submitted Radeon Metal compute work and reproduced
> the GPU reset and WindowServer termination. Do not open Blitsen 0.1.1 windows on that model; use
> 0.1.2 or later, which detects it and refuses the unsafe presentation path.

Blitsen 0.1.1 replaced the GPU Vello/Metal window renderer on `x86_64-apple-darwin` with Vello's
CPU rasterizer and softbuffer. Subsequent hardware evidence showed that softbuffer's macOS
presentation path was not GPU-independent.

## Fixed

- Intel macOS no longer submits Vello rasterizer compute work to Metal. On a 2017 MacBook Pro with a Radeon
  Pro 560, that path caused GPU resets, a WindowServer watchdog termination and a forced logout
  ([#229](https://github.com/krazyjakee/blitsen/issues/229)). The fallback is selected before a
  window surface is created, because device-loss handling cannot recover a desktop session that
  stops responding before wgpu reports an error. This did not account for Core Animation's own
  Metal submission during presentation; see the safety regression above.
- The selected window renderer is written to stderr at startup, so future rendering reports say
  whether they came from the CPU/software or GPU/wgpu path.

Apple Silicon macOS, Linux, Windows and Android continue to use the GPU renderer. Intel macOS may
use more CPU, and GPU-backed custom view composition is unavailable on its software path.

## Validation

The dedicated `darwin-x64` CI job built both release artifacts and ran the real
`blitsen-host` window/surface lifecycle suite successfully on GitHub's `macos-15-intel` runner.

As in 0.1.0, published artifacts are unsigned and are not notarised. See
[`docs/RELEASING.md`](RELEASING.md) for the distribution and signing model.
