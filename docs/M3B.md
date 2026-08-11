# M3b — compatible adoption proof

**M3b is close, and not yet complete.** The export pipeline works and is gated. The adoption claim
it was declared on — take a real, existing Vite application that we did not write and run it
unchanged — was measured against six applications on 2026-08-11 and **all six failed**. After the
work that measurement prompted, five of the six build and render unmodified. Issue
[#69](https://github.com/krazyjakee/blitsen/issues/69) stays open until all six do.

| Gate | What it proves | Result |
| --- | --- | --- |
| `test:m3b` | the export pipeline, on an application written here | passes |
| `test:third-party` | adoption, on applications written by other people | 5/6 build and render |

| Application | Builds | Renders |
| --- | --- | --- |
| Shadcn Admin (React, Tailwind 4, Radix, TanStack, Recharts) | yes | 364 elements, 16 colours |
| vue3-realworld (Vue 3, vue-router, Pinia) | yes | 29 elements, 16 colours |
| `create-vite react-ts` / `vue-ts` / `svelte-ts` | yes | 50 elements, 16 colours each |
| Wordle+ (Svelte) | **no** | no — a remote `<script src>` stops the document loading |

Wordle+ is refused for precisely the reason it does not render, so the diagnostic and the evidence
agree. A remote script is the one asset class that is genuinely fatal: `resolve_local_script`
rejects any `src` with a scheme, and that error aborts the whole script run, so no script on the
page runs. Whether Blitsen should fetch remote scripts at export time is a product question, not a
runtime gap.

## What the six failures were

Worth recording, because none of them was the export pipeline and one of them was a single line.

- **A subresource we refused blocked painting for the life of the document.** Blitz holds a
  stylesheet as a pending critical resource until its handler completes, and the trait's only
  failure signal is dropping that handler — which never completes it. Shadcn Admin mounted 364
  elements with correct layout and correct computed styles, and painted 1,024,000 pixels of pure
  white, because its `<head>` links a Google Fonts stylesheet. `LocalResources` now answers every
  request, with empty bytes for anything it will not serve.
- **`doctor` refused builds over things that render.** Decorative `filter` and `transform` were
  graded errors on the strength of a capture whose real cause was elsewhere; so were references to
  absent APIs that real bundles feature-detect. Both are warnings now, and the stock `create-vite`
  template went from refused to exported.
- **Missing DOM surface**, found by probing the live bridge rather than one crash at a time:
  `createElementNS`, `createComment`, `link.relList`, element traversal, `getElementsByClassName`,
  the `*AttributeNS` trio, `getComputedStyle`, `matchMedia`, `ResizeObserver`, the `Image`
  constructor, `navigator`, and in-memory `localStorage`.
- **Assets a bundler resolved into string literals** were dropped from the export as unreachable,
  so the stock template shipped without any of its images.

## What `test:m3b` proves, and what it does not

The acceptance application is [`examples/vite-react`](../examples/vite-react): a conventional
Vite 8 + React 19 application with no Blitsen imports, source branches, or Vite base-path override.
Its ordinary `vite build` emits `dist/index.html` with Vite's default root-relative asset URLs.
Blitsen scans that untouched output, normalizes local root-relative HTML/CSS references only in
the staged export, and embeds the three output files with the runtime into one executable.

- `blitsen doctor dist` reports zero compatibility errors and eight portability warnings, all in
  feature-detected paths inside React's own bundle: `MessageChannel` in the scheduler, `FormData`
  file bodies, two device APIs, and a media constructor. None of them is taken at runtime.
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

The weakness is the application. `examples/vite-react` was written here, and its markup carries
`data-react-ready="true"`, `id="count"` and `id="increment"` — the exact selectors
`test/run-m3b.mjs` queries. Its CSS was written against what the renderer already does, it ships
one chunk with no `<link rel="modulepreload">`, no remote asset, no `localStorage`, no SVG, and no
router. The gate therefore measures the export pipeline and the DOM bridge against markup shaped
for them. It is not evidence of adoption, and the milestone should never have been closed on it.

## Third-party result (2026-08-11, Linux x64)

Every application below was cloned at a pinned revision and built with its own unmodified build
command. **No source change, config change, polyfill, or shim was applied to any of them.** No
application-specific selector was added anywhere either: the render check counts elements below
`<body>` and distinct painted colours, so it works on applications whose markup we have never seen.

| Application | Framework | `doctor` | `blitsen build` | Rendered |
| --- | --- | ---: | --- | --- |
| [Shadcn Admin](https://github.com/satnaing/shadcn-admin) `70cfd30` | React 19, Tailwind 4, Radix, TanStack, Recharts | 78 errors, 43 warnings | refused | no — blank |
| [Vue 3 RealWorld](https://github.com/mutoe/vue3-realworld-example-app) `a3b0731` | Vue 3, vue-router, Pinia | 15 errors, 8 warnings | refused | no — document did not load |
| [Wordle+](https://github.com/MikhaD/wordle) `199122b` | Svelte 3 | 55 errors, 2 warnings | refused | no — document did not load |
| `create-vite@9.1.2 --template react-ts` | React 19 | 3 errors, 8 warnings | refused | no — blank |
| `create-vite@9.1.2 --template vue-ts` | Vue 3 | 3 errors, 0 warnings | refused | no — document did not load |
| `create-vite@9.1.2 --template svelte-ts` | Svelte 5 | 3 errors, 1 warning | refused | no — document did not load |

The last three are the floor of the claim: the official Vite starter templates, unedited. All three
are refused by `doctor` for the same three declarations in the template's decorative CSS — one
`filter: invert()` inside a media query and two `transform: perspective(…)` rules on the hero
artwork — so `blitsen build` never reaches the exporter. None of the three renders either.

### What the frames show

`doctor` blocks `build`, so the render evidence stages each `dist` exactly as the exporter would
(the same `planIngest` and root-relative rewrite) and loads it directly. That is the most generous
possible reading: the gate is bypassed and the runtime still produces nothing.

- **Shadcn Admin** — one PNG, 1280×800, 1,024,000 pixels of `#ffffff`. Not a partial dashboard: a
  white page. The post-JavaScript DOM has 30 nodes, all of them the shipped shell (`<head>`, the
  stylesheet, `<body>`, an empty `<div id="root">`). React itself loaded and ran.
- **`create-vite react-ts`** — white except for two 1-pixel vertical grey (`#e5e4e7`) hairlines at
  x≈77 and x≈1202, running the full height: the edges of the template's empty centred `#root`
  container. 1,600 non-white pixels out of 1,024,000. `#root` is empty.
- **Vue 3 RealWorld, Wordle+, `vue-ts`, `svelte-ts`** — no frame at all. The document load throws
  before the first layout, so there is nothing to paint.

For contrast, the same pipeline at the same 1280×800 viewport on `examples/vite-react` mounts 17
elements below `<body>` and paints 16 colours: heading, card, three stat tiles, and the Increment
button, all legible. That is the threshold the check uses — 10 elements and 3 colours.

### Where each one stops

| Application | First blocker | Cause |
| --- | --- | --- |
| Shadcn Admin | `ReferenceError: localStorage is not defined` during React's first render | the theme provider does `useState(() => localStorage.getItem("vite-ui-theme") ?? …)`, so the whole tree throws; the effect right behind it calls `window.matchMedia`, which is also absent |
| `create-vite react-ts` | `document.createElementNS is not a function` | React's DOM renderer creates SVG elements through `createElementNS`; the template's logos are inline SVG |
| Vue 3 RealWorld | `document.createComment is not a function` | Vue's runtime creates a comment anchor for every fragment, so no Vue application can mount |
| `create-vite vue-ts` | `document.createElementNS is not a function` | same as React: the template's inline SVG logos |
| Wordle+ | `script src must be relative to the entrypoint: https://www.googletagmanager.com/…` | the loader refuses any remote `<script src>`; here it is an `async` analytics tag whose own inline follow-up is origin-guarded and would have done nothing |
| `create-vite svelte-ts` | `navigator is not defined` | Svelte 5.56's client runtime reads `navigator.userAgent` unguarded while initializing |

A second, independent blocker sits in front of every code-split Vite build: `link.relList` is
absent, so Vite's own module-preload polyfill decides the host cannot preload and calls
`fetch(link.href)` on each `<link rel="modulepreload">`. The document base is `blitsen://app/`, and
`fetch` rejects it with *fetch supports http and https; blitsen: has no server behind it*. The
in-repo example never trips this, because a single-chunk build emits no preload links at all.
Vue 3 RealWorld emits five and takes the path on every load.

### The absent surface behind those failures

Probed directly through the bridge harness, in the same runtime the export uses:

| Area | Present | Absent |
| --- | --- | --- |
| Node creation | `createElement`, `createTextNode`, `innerHTML` | `createComment`, `createDocumentFragment`, `createElementNS`, `insertAdjacentHTML`, `template.content`, `DOMParser`, `Range` |
| Traversal | `document.querySelector`, `childNodes`, `classList`, `style.setProperty` | `element.querySelector`, `element.children`, `firstElementChild`, `closest`, `dataset` |
| Host | `MutationObserver`, `history`, `crypto.randomUUID`, `fetch` (absolute http(s) only) | `localStorage`, `sessionStorage`, `navigator`, `matchMedia`, `getComputedStyle`, `ResizeObserver`, `IntersectionObserver`, `requestIdleCallback`, `Element.animate`, `attachShadow`, `elementFromPoint`, `XMLHttpRequest`, `canvas.getContext` |

`link.relList` is absent, `location.href` is `blitsen://app/`, `document.baseURI` is undefined, and
`fetch` accepts only absolute http(s) URLs, so an application cannot fetch its own bundled files.

Feature detection does not rescue these. Vue and React do not test for `createComment` or
`createElementNS`; Svelte 5 does not test for `navigator`; Shadcn Admin does not test for
`localStorage`. They are unconditional in every framework's mount path.

### Deviations required

None were made, which is the point: this run records what adoption costs, not a manufactured pass.
Getting any of these six to render would need source edits to third-party applications, so P10 is
not met. The nearest fixture, Shadcn Admin, would need at least a `localStorage` shim, an SVG
namespace shim, and its remote font links removed before it could be judged on layout at all — and
that is only the list of blockers reached so far. Each fix exposes the next one behind it.

Two failures are also worth separating from the runtime gaps, because they are ours to soften:

- **`doctor` is a wall, not a report, and its errors are not all real.** It is a regex scan over
  minified output, so a blocking error is not evidence the application does the thing. Shadcn
  Admin's two `WEB_CANVAS` errors are TanStack Table's `column.getContext()`; its `WEB_XHR`,
  `WEB_COMPONENTS` and `WEB_STORAGE` errors sit behind `typeof XMLHttpRequest < "u"`,
  `typeof ShadowRoot < "u"` and a `try`/`catch` — exactly the feature detection the guidance asks
  for. Vue 3 RealWorld is blocked partly by `opacity: 1`. There is no override flag, so an
  application cannot be exported and looked at until every one of these is gone. Whether inert or
  feature-detected code should block a build is a product decision, not a technical one. (A CSS
  diagnostic also reports one line early, because its pattern anchors on the previous
  declaration's `;`.)
- **Remote assets are treated inconsistently.** A remote `<link rel="stylesheet">` (Shadcn Admin's
  Google Fonts) is an ingest warning that still loads; a remote `<script src>` (Wordle+'s analytics
  tag) is a fatal load error. A browser tolerates both, and the second is what stops Wordle+ from
  being measured at all.

## Repeating this

```sh
bun run --cwd packages/blitsen test:m3b            # the export pipeline gate (CI)
bun run --cwd packages/blitsen test:third-party    # the adoption gate (opt-in, not CI)
```

`test:third-party` needs network, npm, npx and corepack. It clones three repositories at pinned
revisions, scaffolds three templates from a pinned `create-vite`, and runs their real builds, so it
takes several minutes on a cold cache. It is deliberately outside CI —
CI must not depend on cloning other people's repositories — and it currently exits non-zero,
because the claim it measures is currently false. `--only <name>` runs one fixture and
`--work <dir>` reuses checkouts between runs. Frames, `render.json` per fixture, and `summary.json`
land in `target/third-party/`.

This proves the M3b adoption and compatibility boundary, not general browser compatibility. The
published [v0 profile](COMPATIBILITY.md) and `doctor` diagnostics define that boundary; the gap
between that profile and what real Vite output does is what the table above measures. The renderer
side of the same question is tracked in [`BLITZ-GAPS.md`](BLITZ-GAPS.md), and the earlier
whole-application survey is [`spikes/s6`](../spikes/s6/README.md). As with M3, the Phase 1
executable is an internal architecture proof and is not yet cleared for redistribution until the
licensing gate in [`LICENSING.md`](LICENSING.md) is automated.
