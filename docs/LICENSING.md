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

## The engine is no longer the hard part

Blitsen hosts **QuickJS-ng, statically linked, under the MIT licence**. That is
a deliberate change from JavaScriptCore and the reason for it was mostly this
document: the JSC path was viable but expensive, and the expense was legal
machinery rather than engineering.

What the swap removed:

| Under JavaScriptCore | Under QuickJS-ng |
|---|---|
| LGPL-family library (WebKit is a per-file BSD/LGPL mixture) | MIT |
| Engine **must** be dynamically loaded and replaceable | Statically linked; nothing ships beside the executable |
| Complete corresponding source and a durable offer | Copyright notice and permission text |
| A reproducible relink flow the recipient can run | — |
| Packaging must not defeat library replacement | — |
| 32 MB library alongside every export | — |

MIT requires that the copyright notice and permission notice travel with the
software. That is a notice-embedding problem, which is tractable and automatable
in a way that "ship a relinkable runtime and a build system" was not.

The history is kept rather than deleted: [`JSC.md`](JSC.md) records the
acquisition decision and why it was made, and [`spikes/s8`](../spikes/s8/README.md)
records the measurements that superseded it.

## What an export must carry

Every dependency's terms still apply — the engine stopped being special, it did
not stop existing. An export links Blitz, Stylo, Vello, wgpu, winit, Taffy,
QuickJS-ng and their transitive dependencies, and the notices for all of them
must travel with the artifact.

Two of those deserve naming:

- **Stylo is MPL-2.0**, file-level. Distributing it in binary form requires
  making the covered source available. This is now the most demanding term in
  the tree, which was not true while JSC was in it.
- **QuickJS-ng is MIT**, requiring the copyright and permission notice.

The application's own HTML, CSS and JavaScript is an interpreted payload read
out of a bundle section at startup. It is not part of the native link and no
dependency's terms reach it.

## Phase 1 exports still carry Bun's terms

An application that carries a `.node` addon still links the Bun host, because
Node-API is Bun's to provide (TECH.md §12). Bun itself is MIT, but its binary
statically contains JSC, so a Phase 1 export inherits the whole LGPL flow this
document used to be about: Bun's complete `LICENSE.md`, the exact Bun and
WebKit revisions, a durable source offer, relink instructions, and terms that
permit modification and reverse engineering for debugging.

This is now a second reason to want the `.node` escape hatch to be rare, beyond
the 95 MB it costs. It is the only path that still needs the relinking flow.

## Exporter acceptance gate

No `blitsen build` command may claim redistribution compliance until an
automated test extracts the embedded notices from a built artifact and checks
them against the dependency tree that produced it. Each platform package needs
its own audited third-party manifest.

That gate is unchanged in principle and much cheaper in practice: for a default
export it is now "are the notices present and complete", with no substitution of
an engine library and no relink flow to complete. Until it exists and passes,
`blitsen build` says so on every run, and it is not implemented yet (#121).
