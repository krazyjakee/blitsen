# S0 — JSC + Blitz single-binary size

Status: Linux x64 measured; initial macOS arm64 and Windows x64 measurements are
defined in `.github/workflows/s0-size.yml` but have not run because GitHub rejected
the jobs at scheduling time for an account billing/spending-limit problem.

## Result

The measured Linux x64 stripped+LTO executable is **52,480,904 bytes (52.48 MB,
50.05 MiB)** installed and **24,076,701 bytes (24.08 MB, 22.96 MiB)** after
`gzip -9`. It executes both the JavaScriptCore C API (`6 * 7 == 42`) and a real
Blitz HTML/layout/CPU-paint path to a 320×180 RGBA buffer.

This misses the Phase 2 bare-app estimate's 50 MB upper bound. It even exceeds
50 MiB (52,428,800 bytes) by 52,104 bytes, before the window/event-loop host,
DOM bridge, module loader, native API, app bundle, wgpu renderer, icons, or other
production runtime services are present. The 25–50 MB headline therefore does
not survive this measurement. The viable positioning is the documented fallback:
**still far below Electron**, with a measured platform budget to be set after the
host exists.

## Linux x64 measurements

| Variant | Installed | gzip -9 |
|---|---:|---:|
| debug | 736,676,312 B (702.55 MiB) | 189,660,248 B (180.87 MiB) |
| release | 67,997,072 B (64.85 MiB) | 26,706,135 B (25.47 MiB) |
| release + strip | 55,274,296 B (52.71 MiB) | 24,851,040 B (23.70 MiB) |
| release + strip + LTO | **52,480,904 B (50.05 MiB)** | **24,076,701 B (22.96 MiB)** |

The machine-readable source is [`results/linux-x64.tsv`](results/linux-x64.tsv).
GNU `strip` was used for the strip row. The production profile uses one codegen
unit, Rust LTO, and Cargo stripping; the JSC LTO archive contains LLVM bitcode.

## What dominates

Controlled feature builds against the same regular JSC archive give the clearest
split:

| Release + strip feature set | Installed | gzip -9 |
|---|---:|---:|
| JSC only | 37,980,984 B (36.22 MiB) | 17,933,416 B (17.10 MiB) |
| Blitz render path only | 18,620,152 B (17.76 MiB) | 7,174,043 B (6.84 MiB) |
| Combined | 55,274,296 B (52.71 MiB) | 24,851,040 B (23.70 MiB) |

The rows are not additive because they share Rust/system support. JSC is the
largest component: the JSC-only build is 68.7% of the combined stripped size.
`cargo-bloat --release --crates` attributes 21.8 MiB, or 62.5% of `.text`, to
native `[Unknown]` symbols (the statically linked JSC/WTF/ICU/mimalloc objects),
then 4.4 MiB (12.6%) to Stylo. Large named native symbols include JSC combined
code, DFG and Wasm parsers. On the Rust side, Stylo, Blitz DOM, font shaping,
SVG/image decoding, and Vello CPU are the material groups.

## Acquisition and pins

- Blitz: `1efe22d2524d71ede5b94592204c21f0de644219`
- Bun WebKit/JSC: `447082ab6897278727b44e1ba3c326ae6e1504c3`
- Bun mimalloc: `1803341d6241d8fa4b3f65fa68cb13a32ad92f04`
- anyrender patch: `dcd219746ff13a5832beab552f3f7f494d1bd84d`
- resvg/usvg patch: `3289a9b0c3d3352692bf5acdf5f6e6949cdb57b5`

The official Bun WebKit prebuilt is the workable standalone JSC acquisition
route. The Linux regular archive SHA-256 is
`a8d331147a731457300aad592b7b26735eb6fcbc5228e4e3c25b67021cc512b7`;
the LTO archive SHA-256 is
`07ecb44fbd5ea68a17956a9ee0a7c03557b7896d41b3161affd7e9410865627f`.
The archives statically supply JavaScriptCore, WTF, bmalloc and ICU. The ELF has
no dynamic JavaScriptCore or ICU dependency, though it still uses ordinary Linux
system libraries such as libc, libstdc++, fontconfig and freetype; “single binary”
does not mean a fully static Linux executable.

## Reproduce

On Ubuntu/Linux x64 with Rust, clang, LLD, GNU binutils, curl, git and gzip:

```sh
./spikes/s0/run-linux.sh
```

The script downloads about 1.1 GB of pinned regular and LTO WebKit archives and
keeps them in `spikes/s0/.cache`. Override `BLITSEN_S0_CACHE` and
`BLITSEN_S0_MEASURE` to put those elsewhere. The recorded run used Rust 1.97.1,
Cargo 1.97.1, clang/LLD 18.1.3 and GNU strip 2.42 on x86_64 Linux.

The executable prints:

```text
jsc=42 rgba_bytes=230400 checksum=21320261
```

The global JSC context deliberately lives for the process lifetime. Calling the
public `JSGlobalContextRelease` on Bun's JSC build without Bun's host glue asserts
during atom-table teardown. A production host must either preserve this lifetime
model or resolve the missing teardown initialization before supporting runtime
restart in-process.

## Initial-platform measurements

| Platform | Stripped + LTO installed | gzip -9 | State |
|---|---:|---:|---|
| Linux x64 | 52,480,904 B | 24,076,701 B | measured locally |
| macOS arm64 | pending | pending | workflow could not be scheduled |
| Windows x64 | pending | pending | workflow could not be scheduled |

The Windows result remains especially important because PRODUCT P5 makes it the
priority target for size claims. Do not infer either pending number from archive
or Linux sizes.
