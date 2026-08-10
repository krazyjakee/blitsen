# v0 compatibility profile

Blitsen v0 accepts built static applications that stay within the surface below. The profile is
deliberately narrower than “works in a browser”: it describes what the current runtime and Blitz
renderer can support consistently enough to make an adoption claim.

Run the check against build output, not source:

```sh
npx blitsen doctor dist
npx blitsen doctor dist --json
```

`doctor` exits non-zero for profile errors. Warnings identify APIs supplied by the Phase 1 Bun
host or renderer features that are not yet portable to the production-shaped Phase 2 host. A
build repeats every diagnostic but remains available so feature-detected fallback paths can still
be exported. The scan is static and conservative: it finds references, not only executed paths.

## Strict v0 surface

| Area | In profile |
| --- | --- |
| Application shape | One built `index.html` plus local files; root-relative HTML/CSS asset URLs are normalized while ingesting without changing `dist` |
| JavaScript | ES modules already emitted by the application's bundler |
| Framework DOM | Stable node identity, standard node type/name/owner fields, `MutationObserver`, creation/insertion/removal, text and attributes |
| Selection and collections | `querySelector`, `querySelectorAll`, `getElementById`, static `NodeList`, `classList` |
| Events | Capture/target/bubble listeners, click, mouse, wheel, keyboard, focus, resize and lifecycle events |
| Scheduling | `requestAnimationFrame`, timers and microtasks |
| CSS | Static block, flex and grid layout; bounded absolute positioning; spacing, borders, backgrounds, colors and system typography |

The M3b acceptance app intentionally uses the normal Vite default output, including
root-relative `/assets/...` references and Vite's module-preload bootstrap. It contains no
Blitsen imports or runtime branches.

## Diagnosed outside the profile

- Canvas, WebGL and WebGPU.
- Browser storage, workers, service workers and browser navigation.
- Composited CSS layers (`opacity`, `visibility`), transforms, fixed/sticky positioning, filters,
  masks and clipping effects.
- Remote subresources in what must be a self-contained export.
- Images, media, SVG, web fonts, `fetch` and browser stream APIs produce portability warnings at
  their current implementation tier.

The scanner cannot prove visual equivalence or determine that an unsupported reference is dead
code. Treat a zero-error report as the build-time gate and retain visual/interaction acceptance tests
for the application itself. See the earlier [S6 renderer evidence](../spikes/s6/README.md) for why
this boundary exists.
