# Licensing Blitsen and exported applications

Blitsen's own source is available under either the Apache License 2.0 or the MIT
License, at your option. This matches Blitz and the usual Rust ecosystem model.
It is compatible with the permissive licences used by Taffy, wgpu, and winit,
and with Stylo's file-level MPL-2.0 terms. Changes copied into MPL-covered Stylo
files remain subject to the MPL; that does not change Blitsen or an application
into an MPL work.

This document records the project's distribution design, not legal advice.
Anyone shipping a commercial runtime should have the final packaging and notice
flow reviewed by qualified counsel.

## JavaScriptCore is the important exception

WebKit is a per-file mixture of BSD-style and GNU Library/Lesser GPL code. The
exact Bun WebKit revision used by S0 contains GNU Library GPL v2-or-later files
inside JavaScriptCore and WTF, so the JSC archive must be treated as an
LGPL-family library even though most files are BSD-licensed.

This is also Bun's stated interpretation. Bun 1.3.14's `LICENSE.md` says that it
statically links LGPL-2 JavaScriptCore and that static-link recipients must be
given the material needed to modify JSC and relink Bun.

Authoritative references:

- [WebKit licensing](https://webkit.org/licensing-webkit/)
- [WebKit licensing documentation](https://docs.webkit.org/Other/Licensing.html)
- [GNU Library GPL 2.0, especially section 6](https://www.gnu.org/licenses/old-licenses/lgpl-2.0.html)
- [Bun 1.3.14 licence and relinking instructions](https://github.com/oven-sh/bun/blob/bun-v1.3.14/LICENSE.md)
- [the pinned Bun WebKit source](https://github.com/oven-sh/WebKit/tree/447082ab6897278727b44e1ba3c326ae6e1504c3)

The practical result is good but conditional: closed-source applications may
use and distribute JSC without licensing the application under the LGPL, as
long as the distributor satisfies the library notices, source, modification,
reverse-engineering, and replacement/relinking conditions.

## Phase 1: Bun-hosted exports

Bun itself is MIT-licensed, but its binary statically contains JSC and other
third-party libraries. A Phase 1 export therefore must embed and expose:

- Bun's complete `LICENSE.md` and all required third-party notices;
- the exact Bun and patched WebKit revisions used by the export;
- a durable source/download offer and the build instructions needed to relink
  Bun with a modified JSC; and
- terms that do not prohibit modification or reverse engineering for debugging
  such modifications.

The application HTML/CSS/JS is an interpreted payload, not part of the native
link. Blitsen's engineering position is that this keeps the application source
outside the JSC relinking material, but that boundary must be included in the
pre-release legal review. The exporter must make it possible to reattach the
unchanged application payload to a compliant rebuilt runtime.

## Phase 2: dynamically replaceable JSC by default

Production Phase 2 exports will dynamically load JSC. The default JSC shared
library may be carried beside the executable or extracted from a one-file
wrapper, but the runtime must provide a documented way to load a user-supplied,
ABI-compatible replacement. Packaging, signatures, or checksums must not make
that replacement impossible.

This is the clean closed-source path: the application can remain proprietary,
while recipients can replace JSC without relinking or receiving application
objects. Exports must still carry the LGPL text, copyright notices, exact JSC
source and patch offer, and the replacement instructions.

Static JSC linking remains useful for internal spikes such as S0. It is not the
default export architecture. A future static export mode may ship only after it
can automatically provide all of the following:

- complete corresponding JSC source, including Blitsen/Bun patches and build
  scripts;
- relinkable Blitsen runtime object code or source;
- a reproducible command that rebuilds with a modified JSC and reattaches the
  unchanged application payload; and
- the notices and permissions required by section 6, without LTO or signing
  preventing the recipient's modified executable from running.

## Exporter acceptance gate

No `blitsen build` command may claim redistribution compliance until an
automated test extracts the embedded notices/source offer, substitutes a
compatible JSC library (or completes the static relink flow), and launches the
result. Each platform package needs its own audited third-party manifest.
