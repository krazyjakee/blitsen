# Blitz rendering gaps — standing list

Blitsen renders through [Blitz](https://github.com/DioxusLabs/blitz), pinned by revision. This is
the live list of rendering divergences we know about, with status. It supersedes the frozen
catalogue in [`spikes/s6/README.md`](../spikes/s6/README.md), which was a dated snapshot from one
run and is kept only as the evidence behind the initial triage.

**Scope.** A gap belongs here when Blitz renders something differently from a browser, or not at
all. Things Blitsen has simply not implemented — `fetch`, images, web fonts — are capability tiers,
not Blitz gaps; those live in [`COMPATIBILITY.md`](COMPATIBILITY.md).

Status values: **open** (reproduced, unfiled), **filed** (upstream issue exists), **fixed**,
**withdrawn** (turned out not to be a Blitz gap).

| # | Gap | Status | Evidence | Notes |
| --- | --- | --- | --- | --- |
| G1 | Digits absent from some text runs while letters and punctuation render | filed | `spikes/s6/results/blitz-snapshot/react.png` | [blitz#688](https://github.com/DioxusLabs/blitz/issues/688); see below |
| G2 | A property named by `transition` keeps its pre-stylesheet value for good | filed | `crates/blitsen-blitz/tests/reductions/transition-stale-style.html` | [blitz#689](https://github.com/DioxusLabs/blitz/issues/689); the hidden overlays were a symptom |
| G3 | `absolute`/`fixed` insets and auto margins resolve against the wrong box | filed | `crates/blitsen-blitz/tests/conformance/cases/defect-*.html` | [blitz#690](https://github.com/DioxusLabs/blitz/issues/690), [blitz#691](https://github.com/DioxusLabs/blitz/issues/691); gated as known-failing cases |
| G4 | Form controls and anchors fall back to native/default styling | open | `spikes/s6/results/blitz-snapshot/{react,vue}.png` | `appearance`, inherited anchor colour, Tailwind 4 reset, control UA CSS |
| G5 | SVG icons missing or geometrically wrong; chart fills incorrect | filed upstream | `spikes/s6/results/blitz-snapshot/react.png` | [blitz#448](https://github.com/DioxusLabs/blitz/issues/448) |
| G6 | Accumulating line-height, antialiasing and vertical spacing differences | open, low priority | `spikes/s6/results/metrics.tsv` | Needs a tolerance corpus before it can be actioned |
| G7 | `blitz-dom`'s `svg` feature does not compile at the pinned revision | filed | Build failure, reproducible | [blitz#687](https://github.com/DioxusLabs/blitz/issues/687); see below |

Broad Tailwind/renderer work has an upstream collection in
[blitz#389](https://github.com/DioxusLabs/blitz/issues/389).

## G1 — the digits gap, corrected

The s6 catalogue classified this as a "missing numeric glyphs" text-paint bug. Two explanations
have since been ruled out, and the entry is narrower than it first appeared:

- **It is not Blitsen's missing-fonts bug.** Blitsen shipped for months with `blitz-dom`'s
  `system-fonts` feature off, so Parley had no font sources and *no* glyph rendered. That was a
  Blitsen dependency-feature bug, fixed separately. The s6 spike is unaffected by it: `run.sh`
  builds **upstream Blitz's own `screenshot` example** from a clean checkout, with Blitz's own
  feature defaults. The two are unrelated, and fixing one did not fix the other.
- **It is not a numeric font-feature bug.** `font-variant-numeric: tabular-nums`, `slashed-zero`
  and `font-feature-settings: 'tnum'` all render digits correctly in Blitsen's renderer against
  Noto Sans.

What the evidence actually shows: in the same screenshot, the chart's axis labels (`$6000`,
`$4500`, `$1500`, `$0`) render their digits correctly, while the summary-card values on the same
page lose theirs. So digit rendering is not globally broken — something about the specific font
resolution on those elements is. The fixture's font stack is the obvious next suspect, which makes
this adjacent to font fallback ([#104](https://github.com/krazyjakee/blitsen/issues/104)).

Filed as [blitz#688](https://github.com/DioxusLabs/blitz/issues/688) with the evidence and the
ruled-out explanations, explicitly as an unreduced report. The affected elements are the ones
Tailwind 4 / Shadcn styles with the app's own font stack, while the working axis labels come from
Recharts — a fallback-chain difference between those two paths is the most likely candidate.

**Next step:** a minimal reproduction isolating the card elements' resolved font stack.

## G2 — the overlays were a symptom; the defect is `transition`

The s6 catalogue read this as `visibility: hidden` and `opacity: 0` being ignored. They are not:
both suppress paint correctly, on their own and through positioned descendants, at the pinned
revision. What the Wordle+ capture actually shows is narrower and worse.

Wordle+'s modal overlay carries `transition: all 0.2s ease` alongside `visibility: hidden`,
`opacity: 0` and its backdrop colour. When the stylesheet finishes loading *after* the first style
resolution — which is what happens for a `<link>`, and not what happens for a `<style>` element —
every property named by that `transition` keeps the value it had before the sheet applied, and keeps
it permanently. `visibility` stays `visible`, so the closed modals paint; the backdrop stays
transparent, so no dark overlay appears behind them; the keyboard keys lose their background for the
same reason. Resolving the document at a later timestamp does not settle it.

The reduction is [`tests/reductions/transition-stale-style.html`](../crates/blitsen-blitz/tests/reductions/transition-stale-style.html)
and its stylesheet: two paragraphs the sheet hides, one of which also declares `transition`. Blitz
paints that one. The same CSS inlined in a `<style>` element renders correctly, and so does a
`transition: color` that does not name `visibility` — the staleness is scoped to the properties the
transition covers.

It reproduces in upstream Blitz's own `screenshot` example at the pinned revision, which is how the
s6 images were made. It is **not** reachable through Blitsen's renderer today, because Blitsen has
no net provider and its bundles inline their CSS; that is also why it cannot be a conformance case.

## G3 — not stacking contexts: two containing-block defects

The s6 catalogue put this down to overlays escaping their stacking context. Stacking is fine:
`z-index` order and `transform` both behave. Two containing-block defects explain the capture
instead, and both are now gated as known-failing conformance cases (see
[CONFORMANCE.md](CONFORMANCE.md)):

- **The initial containing block is the root element's box, not the viewport.** An absolutely
  positioned box with no positioned ancestor resolves its insets against its parent block, and
  `position: fixed` does the same, so on a document shorter than the window both stop at the
  content's height instead of reaching the bottom of the viewport.
  [`defect-initial-containing-block.html`](../crates/blitsen-blitz/tests/conformance/cases/defect-initial-containing-block.html).
- **Auto margins are resolved from the declared width, not the used one.** With both insets given,
  `margin: auto` and a width that `max-width` clamps, the free space is computed against the
  unclamped width, which leaves no margin and pins the box to the leading edge instead of centring
  it. [`defect-absolute-auto-margins.html`](../crates/blitsen-blitz/tests/conformance/cases/defect-absolute-auto-margins.html).

Together they are why Wordle+'s modals sit at the left edge and stop short of the viewport. Both
reproduce in upstream Blitz's `screenshot` example as well as in Blitsen's renderer, and the numbers
in both cases were confirmed against Chromium at the same viewport.

## G7 — `blitz-dom`'s `svg` feature does not compile

Enabling `blitz-dom`'s `svg` feature at the pinned revision fails:

```
error[E0432]: unresolved import `usvg::svgtypes`
error[E0599]: no method named `intrinsic_dimensions` found for struct `std::sync::Arc<usvg::Tree>`
```

The feature has drifted behind its `usvg` dependency. Blitz's own meta-crate never enables
`blitz-dom/svg` — it takes `blitz-dom` with default features off — so upstream has not hit it.
Blitsen therefore selects `blitz-dom`'s features explicitly, with `svg` omitted, and keeps
`blitz-paint`'s defaults off because its default `svg` feature turns `blitz-dom/svg` back on.

Filed as [blitz#687](https://github.com/DioxusLabs/blitz/issues/687), with a suggestion that a
`--all-features` build of `blitz-dom` in CI would catch this class of drift.

## Keeping this list honest

- Every row needs evidence that can be re-examined — a committed image, a test, or a build failure
  someone else can reproduce. "Looked wrong once" is not a row.
- A row moves to **filed** only with an issue number, and to **withdrawn** with the reason.
- A row with a reduction says what the reduction shows, not what the first screenshot suggested.
  Three rows have now changed description under reduction — G1, G2 and G3 — and each time the
  original wording would have sent a maintainer looking for a bug that does not exist.
- Re-running the s6 corpus needs Chromium, ImageMagick, npm, Corepack, Python 3 and network
  access, so it is not a CI gate, and it stays manual. The gated corpus is the golden-image one
  in [CONFORMANCE.md](CONFORMANCE.md), which asks the other question: not whether Blitsen matches
  a browser, but whether it matches itself on every platform. A gap belongs here when a browser
  disagrees with Blitz; a conformance case belongs there when Blitsen has to keep agreeing with
  itself.
