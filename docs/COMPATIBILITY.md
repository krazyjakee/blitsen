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
build repeats every diagnostic and **fails on any error** — an export that cannot run is not worth
producing — while warnings let feature-detected fallback paths through. The scan is static and
conservative: it finds references, not only executed paths.

## Strict v0 surface

| Area | In profile |
| --- | --- |
| Application shape | One built `index.html` plus the local files reachable from it; root-relative HTML/CSS asset URLs are normalized while ingesting without changing `dist` |
| JavaScript | ES modules already emitted by the application's bundler |
| Framework DOM | Stable node identity, standard node type/name/owner fields, `MutationObserver`, creation/insertion/removal, text and attributes |
| Selection and collections | `querySelector`, `querySelectorAll`, `getElementById`, static `NodeList`, `classList` |
| Events | Capture/target/bubble listeners, click, mouse, wheel, keyboard, focus, resize and lifecycle events |
| Scheduling | `requestAnimationFrame`, timers and microtasks |
| CSS | Static block, flex and grid layout; bounded absolute positioning; spacing, borders, backgrounds, colors and system typography |

The M3b acceptance app intentionally uses the normal Vite default output, including
root-relative `/assets/...` references and Vite's module-preload bootstrap. It contains no
Blitsen imports or runtime branches.

## Asset URLs

There is no web server behind an exported application, so a URL that assumes a server root has to
be resolved at build time. **Blitsen rewrites server-root URLs while ingesting, in its own staging
copy — your `dist` directory is never modified.**

| You wrote | Blitsen does |
| --- | --- |
| `href="./assets/app.css"` | Nothing; relative URLs already work. |
| `src="/assets/app.js"` (default `base`) | Rewrites to the equivalent document-relative path. |
| `src="/app/assets/app.js"` (custom `base: "/app/"`) | Drops the base prefix that does not exist in the output, then rewrites. |
| `url("/assets/hero.png")` in CSS, and `@import` | Same rewrite, applied transitively. |
| `<a href="/settings">` | Nothing; anchors are navigation, not subresources. |
| `src="https://cdn…"` or `//cdn…` | Fails the build. A self-contained export cannot fetch it. |

**Only HTML and CSS are rewritten.** JavaScript is left byte-identical, because a path assembled
at runtime cannot be safely edited by a regular expression. In practice:

- `new URL('./x.png', import.meta.url)` and a relative `import()` **work** — the export preserves
  your directory layout, so relative resolution lands on the same file it did on a server.
- `new URL('/assets/x.png', …)`, `fetch('/data.json')`, and any specifier built from a variable or
  template literal **do not work**, and are not diagnosed. Configure your bundler with a relative
  base (Vite: `base: './'`) if your application computes asset URLs from a server root.

## Unreferenced files

Ingest walks the output directory from `index.html` and collects only what it can reach.
Whatever is left over is listed at the end of the build and dropped, because an unreferenced file
is pure export size. Keep some of it with a repeatable glob (`*` stops at `/`, `**` does not):

```sh
npx blitsen build dist --include 'assets/*.wasm' --include 'locales/**'
```

That is also the escape hatch for a file only a runtime-computed URL reaches.

## Where assets live

`--assets embedded` (the default) puts every asset inside the executable and unpacks them into a
private temporary directory at launch — one file to ship, nothing to install. `--assets
side-loaded` writes them to `<outfile>.assets/` beside the executable instead, which is the right
choice when assets must stay patchable after shipping or are large enough that carrying them in
the binary is wasteful. Each asset is content-hashed with SHA-256 either way, and repeating a
build from the same input directory, output path and working directory produces a byte-identical
executable.

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
