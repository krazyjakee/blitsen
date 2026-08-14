# 0.1.0 — first cross-platform release

Draft. This is what goes in the GitHub release and the npm README banner when 0.1.0 publishes;
it is committed so the claims in it are reviewable before they are made rather than written in a
hurry against a registry that cannot take them back.

Blitsen is **pre-alpha**. What follows is written to be true rather than to be attractive.

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

## Every platform is unsigned

**No artifact in this release is signed, on any platform.** No Apple Developer ID and no Windows
code-signing certificate exist for this project yet, so the release was built with none and the
signing steps recorded that they were skipped.

Inside an npm package this is mostly invisible: neither Gatekeeper nor SmartScreen inspects an
addon a package manager downloaded. Where it is visible is an application *you* export with
`blitsen build` — that is an executable your users launch by name, which is exactly what an OS
gatekeeper checks. Sign it yourself, on a host of that platform: `--sign` takes the command.
Nothing here is notarised either (issue #71).

## What has been tested, and where

Three targets run the full suite on every push — `linux-x64`, `darwin-arm64`, `win32-x64`:
workspace tests, the native acceptance harnesses, layout conformance, frame determinism and the
size gate.

Three run a smoke tier — `linux-arm64`, `darwin-x64`, `win32-arm64`: both release artifacts built,
the package tests against them, a frame through the native harness, a standalone export that is
built and run, the layout corpus, and a size measurement that reports rather than gates. A
regression specific to one of those three is likelier to reach you than one on the first three
(issue #133).

Size, cold start and idle RAM are measured on Linux x86-64. The other five targets have no
committed baseline yet, so the size gate reports on them and gates nothing.

## Built on a patched Blitz

The renderer is [Blitz](https://github.com/DioxusLabs/blitz) at `1efe22d2` plus one commit, taken
from a fork: replaced layout reaches `unreachable!()` when a replaced element carries a custom
widget, which is what stands between `<canvas>` and the compositing seam. Filed as
[blitz#706](https://github.com/DioxusLabs/blitz/issues/706) and offered upstream as
[blitz#719](https://github.com/DioxusLabs/blitz/pull/719); the pin retires when it lands. Every
binary here contains that fork's rendering code, and the third-party notices each package carries
name it.

## Known gaps

- Node-API wrapper finalizers do not run on Windows, so a long-running Windows application retains
  every DOM node it has touched (issue #136).
- `<canvas>` 2D, WebGL/WebGPU, WebRTC, accessibility, IME and cross-platform font fallback are
  out of scope for this release and tracked in the backlog milestone.
- `blitsen doctor` reports what the compatibility profile does not cover; read it before assuming
  an application is inside the profile.

## Checking the release

The only real check is the one that uses the registry rather than the workflow: install `blitsen`
on a machine that has never built it, export an application, and run the artifact. See
`docs/RELEASING.md`.
