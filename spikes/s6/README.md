# S6 — real Vite application compatibility

Date: 2026-08-10

Platform: Linux x86_64 (Linux Mint 22.3)

Viewport: 1440 × 800 CSS px at device scale 2

Blitz: [`1efe22d`](https://github.com/DioxusLabs/blitz/commit/1efe22d2524d71ede5b94592204c21f0de644219), `0.3.0-beta.1`

Reference renderer: Chromium 151.0.7922.108

## Decision

**The drop-in premise does not survive unchanged.** All three unmodified Vite builds render a
blank page in Blitz's plain HTML frontend because React, Vue, and Svelte create their application
DOM by running JavaScript. This is an ingest/runtime gap, not a CSS failure. S6 cannot measure a
client-rendered framework app "with Blitz alone" from raw build output.

A second pass serialised each live DOM after Chromium ran the unmodified bundle, then handed that
DOM and the build's original CSS to Blitz. This isolates the renderer from the absent JS bridge.
That pass shows useful modern layout coverage, but none of the three apps is screenshot-compatible:

- Vue/Conduit is structurally close. Grid, block flow, spacing, borders, tags, and the responsive
  two-column content area survive. Anchor inheritance, font weight, SVG/icon paint, and form/button
  appearance visibly diverge.
- React/Shadcn Admin preserves the page skeleton, responsive sidebar, card grid, spacing, and most
  colors. Default control/anchor styles leak through, SVG icon/chart paint differs, and numeric
  glyphs vanish from prominent HTML and SVG text. Its large-scale structure is the closest in the
  sample, but missing values make the dashboard unusable.
- Svelte/Wordle+ fails. Elements hidden with `visibility`/`opacity`, absolutely positioned faces,
  and transformed overlays are painted together; settings, statistics, tutorial, board, and toast
  content overlap. The base flex/grid geometry is present, but the result is not usable.

The viable product claim is therefore narrower: Blitsen can consume ordinary Vite builds only once
its JS runtime/DOM bridge executes the entry module, and applications must target a documented CSS
compatibility profile until the renderer gaps below land upstream.

## Fixtures

These are existing applications, pinned rather than copied into this repository:

| Framework | Application | Revision | Licence | Why selected |
| --- | --- | --- | --- | --- |
| React | [Shadcn Admin](https://github.com/satnaing/shadcn-admin) | [`70cfd30`](https://github.com/satnaing/shadcn-admin/commit/70cfd3098f219f09a3c6941b2d1fabe4665dfa3d) | MIT | A full responsive dashboard using React 19, Tailwind 4, Radix, TanStack, Recharts, and SVG icon sets |
| Vue | [Vue 3 RealWorld](https://github.com/mutoe/vue3-realworld-example-app) | [`a3b0731`](https://github.com/mutoe/vue3-realworld-example-app/commit/a3b07312d4c416c3976a3012e64cf39053060708) | MIT | A routed Vue 3/Pinia application with real feed content, controls, tags, and responsive layout |
| Svelte | [Wordle+](https://github.com/MikhaD/wordle) | [`199122b`](https://github.com/MikhaD/wordle/commit/199122be1f3ed71f5cf4abd5748debd91ee540a0) | GPL-3.0 | A shipped Svelte game with grid/flex layout, overlays, transforms, transitions, controls, and CSS variables |

The Svelte capture seeds the application's own persisted settings to light theme and closes its
first-run tutorial. No source or generated CSS is changed.

## Method

1. Build each pinned application with its own unmodified `vite build` command.
2. Serve each `dist` directory locally.
3. Capture Chromium after its bundle settles. Also serialise the resulting DOM.
4. Render the untouched `index.html` with Blitz's upstream CPU screenshot example.
5. Render the serialised DOM with the same original CSS and assets using the same Blitz path.
6. Compare the first 1440 × 800 CSS-pixel viewport. Blitz computes Vue's document as taller than
   the viewport, so only that viewport is used in the metric.

Run [`run.sh`](./run.sh) to repeat the build and capture. It requires Bun, Cargo, Chromium,
ImageMagick, Node/npm, Corepack, Python 3, and network access. Exact source revisions are constants
in the script.

RMSE records pixel error (lower is closer); NCC records image correlation (1 is identical). These
numbers make runs comparable, but are not a compatibility percentage. Large white regions can make
a blank image's RMSE look deceptively good, so raw-build blank renders are deliberately not scored.

| Application | Normalised RMSE | NCC | Outcome |
| --- | ---: | ---: | --- |
| React/Shadcn Admin | 0.125972 | 0.898129 | Structure close; numeric content missing plus control/reset and SVG differences |
| Vue/Conduit | 0.108616 | 0.359666 | Structurally close; still visibly non-equivalent |
| Svelte/Wordle+ | 0.173018 | 0.191305 | Failed overlay/visibility/transform composition |

## Evidence

| | Chromium | Blitz after DOM serialisation | Pixel diff |
| --- | --- | --- | --- |
| React | [reference](./results/chromium/react.png) | [Blitz](./results/blitz-snapshot/react.png) | [diff](./results/diff/react.png) |
| Vue | [reference](./results/chromium/vue.png) | [Blitz](./results/blitz-snapshot/vue.png) | [diff](./results/diff/vue.png) |
| Svelte | [reference](./results/chromium/svelte.png) | [Blitz](./results/blitz-snapshot/svelte.png) | [diff](./results/diff/svelte.png) |

The [raw Blitz renders](./results/blitz-raw/) are three visually identical blank pages. They are
kept as the direct evidence for the unmodified-build result.

## Divergence catalogue

| Application | Visible divergence | Classification | Likely work |
| --- | --- | --- | --- |
| All raw builds | Empty framework mount element; module script is not run | Out of scope for Blitz alone; required Blitsen runtime work | JS engine, module loader, DOM bridge (the core product) |
| React | Digits disappear from revenue, counters, percentages, sales values, and SVG chart labels while punctuation remains | Text paint/layout bug | Minimal reproduction and upstream Blitz/Parley fix; project-blocking for dashboards |
| React | Search input, download button, skip link, and navigation links use native/default styling or blue anchors | CSS/reset and form-control gap | Investigate `appearance`, inherited anchor color, Tailwind 4 reset rules, and control UA CSS |
| React | Several SVG icons are missing/geometrically different and chart bars use the wrong fill | Paint/parser gap | Track with [Blitz SVG support #448](https://github.com/DioxusLabs/blitz/issues/448) |
| React | Fine typography, icon weight, shadows, and a few card offsets differ | Font/paint/layout bugs | Golden reductions upstream; lower priority than control/SVG gaps |
| Vue | Article and navigation anchors are blue/bolder instead of inheriting surrounding styles | CSS cascade/reset gap | Reduce inherited color/font-weight handling against compiled Vue CSS |
| Vue | Favourite controls lose green styling and heart icons; borders and native control chrome differ | Form-control/SVG gap | UA control styling plus SVG support |
| Vue | Minor line-height, text antialiasing, and vertical spacing differences accumulate | Font/layout bug | Tolerance corpus and focused upstream cases |
| Svelte | `visibility:hidden`/`opacity:0` overlays still paint | Paint/invalidation gap | Upstream blocker; closed settings, stats, tutorial, and toast layers must not render |
| Svelte | Absolute faces and fixed/transformed overlays overlap and escape their intended stacking/layout | Layout/paint gap | Focused transform, positioning, clipping, and stacking-context reductions |
| Svelte | Grid tiles and controls lose sizing/alignment under the overlaid content | Layout bug | Verify grid intrinsic sizing after the visibility/stacking fix |

The broad Tailwind/renderer work already has an upstream collection in
[DioxusLabs/blitz#389](https://github.com/DioxusLabs/blitz/issues/389). This run adds four concrete
work streams: missing numeric glyphs, visibility/opacity paint suppression, positioned and
transformed stacking, and reset/control/SVG fidelity. The first two are blockers rather than polish.

## Answer to S6

Blitz already gets enough block, flex, grid, responsive, border, spacing, and variable-driven CSS
right to recognise all three real applications and to preserve the large-scale structure of two.
It does **not** yet get enough right to promise faithful rendering of arbitrary framework output.
The Svelte result and React's missing numeric content are hard failures. Combined with the
guaranteed blank raw SPA output, the current “drop in any existing build” premise must be re-scoped
before it is published.
