# 0.1.0 — first cross-platform release

Blitsen 0.1.0 is the first release with native runtimes for Linux, macOS and Windows, on both x64
and arm64. Blitsen is **pre-alpha**: use the published compatibility profile and known gaps below
as the boundary of what this release supports.

## What ships

Seven packages, published together: `blitsen`, which is thin JavaScript, and one runtime package
per target, pinned to it exactly.

| Target | Package | Download |
| --- | --- | --- |
| `linux-x64` | `@blitsen/linux-x64` | 31.4 MB |
| `linux-arm64` | `@blitsen/linux-arm64` | 29.4 MB |
| `darwin-x64` | `@blitsen/darwin-x64` | 27.0 MB |
| `darwin-arm64` | `@blitsen/darwin-arm64` | 25.5 MB |
| `win32-x64` | `@blitsen/win32-x64` | 28.7 MB |
| `win32-arm64` | `@blitsen/win32-arm64` | 26.6 MB |

Download is the compressed platform package: the addon `blitsen run` loads, the executable an
export links into, and the third-party notices for that target's dependency graph. Only one of the
six is installed on any machine.

```sh
npm i -D blitsen
```

`os` and `cpu` on each runtime package are what make your package manager download only the one
your machine needs. There is no postinstall compile step and no Rust toolchain.

The Linux runtimes are built on Ubuntu 22.04 and require glibc 2.35 or newer, plus ALSA
(`libasound.so.2`), OpenSSL 3 (`libssl.so.3` and `libcrypto.so.3`), fontconfig
(`libfontconfig.so.1`) and the display libraries for the active X11 or Wayland session.
The Windows runtimes support Windows 10 or newer (and Server 2016 or newer on x64) and statically
link the Microsoft C runtime, so a separate Visual C++ Redistributable installation is not
required.

The GitHub release also keeps the seven package tarballs (`.tgz`) as durable release assets,
alongside `SHA256SUMS` for independent integrity checks. They are a recoverable snapshot of what
was published, not a second installation channel: `npm install blitsen` remains the normal way to
install and resolve the runtime for the current machine.

## Every platform is unsigned

**No artifact in this release is signed, on any platform.** That includes the Linux, macOS and
Windows runtimes in npm and in the GitHub release tarballs. No Apple Developer ID or Windows
code-signing certificate exists for this project yet, so the release was built with none and the
signing steps recorded that they were skipped.

Inside an npm package this is mostly invisible: neither Gatekeeper nor SmartScreen inspects an
addon a package manager downloaded. Where it is visible is an application *you* export with
`blitsen build` — that is an executable your users launch by name, which is exactly what an OS
gatekeeper checks. Sign it yourself, on a host of that platform: `--sign` takes the command.
Nothing here is notarised either (issue #71).

## What has been tested, and where

Three primary targets run the platform behaviour suite on every push — `linux-x64`,
`darwin-arm64`, `win32-x64`: workspace tests, the native acceptance harnesses, host conformance,
layout conformance and size/benchmark measurement. Frame determinism is a separate Linux x64 gate,
and only Linux x64 has a committed size baseline that can fail a build.

Three run a smoke tier — `linux-arm64`, `darwin-x64`, `win32-arm64`: both release artifacts built,
the package tests against them, a frame through the native harness, a standalone export that is
built and run, the layout corpus, and a size measurement that reports rather than gates. A
regression specific to one of those three is likelier to reach you than one on the first three
(issue #133).

Size, cold start and idle RAM are measured on the three primary targets. The other three targets
have a report-only size measurement. Only Linux x64 has a committed size baseline, so measurements
on the other five targets report rather than gate.

## Built on a patched Blitz

The renderer is [Blitz](https://github.com/DioxusLabs/blitz) at `1efe22d2` plus one commit, taken
from a fork: replaced layout reaches `unreachable!()` when a replaced element carries a custom
widget, which is what stands between `<canvas>` and the compositing seam. Filed as
[blitz#706](https://github.com/DioxusLabs/blitz/issues/706) and offered upstream as
[blitz#719](https://github.com/DioxusLabs/blitz/pull/719); the pin retires when it lands. Every
binary here contains that fork's rendering code, and the third-party notices each package carries
name it.

## Known gaps

- `<canvas>` 2D is not implemented and a document containing it is rejected by `blitsen doctor`.
  WebGL, WebGPU and WebRTC are not implemented either (issue #99 and the compatibility profile).
- Form controls support basic keyboard editing, caret placement and drag selection, but IME
  composition, clipboard/undo, `contenteditable` and complex-script coverage remain incomplete;
  the runtime also exports no platform accessibility tree (issues #103 and #102).
- Cross-platform font fallback is incomplete; verify text rendering on every platform you ship
  (issue #104).
- `blitsen doctor` reports what the compatibility profile does not cover; read it before assuming
  an application is inside the profile.

## Checking the release

Install `blitsen` from npm on a machine that has never built it, export an application, and run the
artifact. The release page's `SHA256SUMS` can also verify a downloaded `.tgz`; see
`docs/RELEASING.md` for the release procedure.
