# M3b — compatible adoption proof

M3b is complete on the currently supported Linux x64 development target. The acceptance
application is [`examples/vite-react`](../examples/vite-react): a conventional Vite 8 + React 19
application with no Blitsen imports, source branches, or Vite base-path override.

Its ordinary `vite build` emits `dist/index.html` with Vite's default root-relative asset URLs.
Blitsen scans that untouched output, normalizes local root-relative HTML/CSS references only in
the staged export, and embeds the three output files with the runtime into one executable.

## Acceptance evidence

- `blitsen doctor dist` reports zero compatibility errors. It currently emits one portability
  warning for the guarded `fetch` reference in Vite's module-preload bootstrap; that code path is
  not taken by this output and `fetch` exists in the Phase 1 host.
- The production React module mounts after the host event loop advances. Vite's bootstrap uses a
  functional `MutationObserver`; React sees stable DOM wrappers plus standard node identity fields.
- The standalone gate runs the exported executable with an empty `PATH`, asserts the React tree
  mounted, dispatches a bubbling click through React's delegated listener, and observes the state
  render change from `0` to `1`.
- The optimized Linux x64 acceptance artifact measured 132,364,416 bytes installed. This is a
  Phase 1 Bun-hosted measurement, not a production size target.
- The gate exposed and fixed a compiled-host-only identity failure: weak native wrappers could be
  reclaimed while their connected DOM nodes remained. Each document context now strongly interns
  wrappers, preserving React's listener and fiber properties across garbage collection.

Repeat the gate with:

```sh
bun run --cwd packages/blitsen test:m3b
```

This proves the M3b adoption and compatibility boundary, not general browser compatibility. The
published [v0 profile](COMPATIBILITY.md) and `doctor` diagnostics define that boundary. As with M3,
the Phase 1 executable is an internal architecture proof and is not yet cleared for redistribution
until the licensing gate in [`LICENSING.md`](LICENSING.md) is automated.
