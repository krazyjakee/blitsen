# Layout conformance corpus

Product requirement **P6** is byte-identical layout across platforms for the test corpus under
pinned inputs—not a claim that different operating-system fonts have identical metrics. This
corpus is the gate on that claim: static documents rendered headlessly to PNG, compared against
committed golden images, run on Linux, macOS and Windows in CI.

```sh
bun run --cwd packages/blitsen test:conformance   # or: cargo test -p blitsen-blitz --test conformance
```

It also runs inside `cargo test --workspace`, so a renderer change cannot land without it.

Everything lives in [`crates/blitsen-blitz/tests/conformance/`](../crates/blitsen-blitz/tests/conformance):
`conformance.rs` is the harness, `cases/` holds the documents, `goldens/` holds their images.

## Two tiers, because they answer different questions

**The declared checks are a correctness oracle.** Each case file opens with a comment containing
the geometry and colours it is *meant* to produce, and the reasoning that produced them. Those
numbers were computed from the CSS the case declares — not read back from a run — so a case that
passes says the layout is right, not merely unchanged. This is the part that stops a corpus
becoming a record of current bugs.

**The golden PNG is a change detector.** It covers the whole frame, including everything nobody
thought to assert. That matters more than it sounds: the three renderer bugs found while building
this corpus' surroundings — `blitz-dom`'s `system-fonts` feature off so no glyph ever rendered, its
`image` crate carrying no codecs so no image ever decoded, and `background-image` painting a frame
late — were all invisible to DOM-level assertions and all visible in a frame.

Record with:

```sh
bun run --cwd packages/blitsen conformance:record
```

Recording **refuses to run while any declared check fails**, so a golden can never lock in a layout
the corpus itself says is wrong.

## Cases that document a Blitz defect

A reduction of a renderer bug fails by definition, and the two tiers above have no room for one: it
cannot pass its checks, and a golden of its frame would be the recording this corpus exists to
avoid. So a third kind of case says `@defect <what>` and marks the individual checks a browser
satisfies and Blitz does not with `@!`. Those checks are asserted to *fail*.

The gate therefore stays green while the defect stands, and the run prints what still diverges. The
moment the defect is fixed the case fails, with the message that it looks fixed and the `!` should
go — because a known-failure that quietly starts passing is worth nothing, and the point is to close
the entry in [BLITZ-GAPS.md](BLITZ-GAPS.md) rather than let it rot. The numbers on a `@!` line are
still derived rather than recorded, and are checked against a browser; the two cases here were
verified against Chromium at the same viewport.

Such a case carries no golden image, and every check it does not mark still gates normally — which
is what keeps it a reduction rather than a blank cheque.

## Case format

A case is one self-contained HTML document. The leading comment is the expectation header; only
`@`-prefixed lines are read, so the rest of it is free for the arithmetic behind the numbers.

| Directive | Meaning |
| --- | --- |
| `@size <w> <h>` | Viewport, default `400 200` |
| `@oracle` | The declared checks were derived from the CSS, not recorded from a run |
| `@host-fonts` | Depends on the host's installed fonts; carries no golden image |
| `@defect <what>` | Documents a Blitz defect; carries no golden image |
| `@!<check>` | A browser satisfies this check and Blitz does not, so it must fail |
| `@box <selector> <x> <y> <w> <h>` | Border box of the one element the selector matches; a component written `-` is not checked |
| `@pixel <x> <y> <#rrggbbaa>` | Exact colour at a point |
| `@ink <x> <y> <w> <h> <#rrggbbaa> >=\|<= <fraction>` | Fraction of a rectangle painted exactly that colour |

Relative URLs resolve against [`crates/blitsen-blitz/fixtures/`](../crates/blitsen-blitz/fixtures),
which holds the pinned fonts and the 8x4 red/blue image the image cases probe.

## How portability is handled

Two different problems, two different answers.

**Fonts.** Text metrics decide every box below the first line of text, and a CI runner has
different fonts from a laptop. So the corpus ships the `block-regular`, `block-bold`,
`block-italic` and `block-ascii` faces, built by
[`fixtures/generate.py`](../crates/blitsen-blitz/fixtures/generate.py). Their glyphs are solid
rectangles one em wide with an ascent of exactly one em and no descent. Every metric a case depends
on therefore comes out of a committed file, and — because no system fallback paints a filled
rectangle — the frame also says *which* font was used rather than merely that one was.

That is also the product policy outside the corpus: system fonts are available, but an application
that needs portable coverage or metrics supplies its own `@font-face` files. Blitsen bundles no
universal fallback. The `host-typography` case deliberately proves only that a discovered host face
paints and carries neither a golden nor metric assertions.

**Rasterization.** Antialiasing is the CPU's, and vello_cpu selects SIMD kernels at runtime. So the
harness renders a fixed fixture, digests it, and compares golden images only where that fingerprint
matches the one in `goldens/rasterizer.json`. The fixture uses the pinned font, so a fingerprint
mismatch means the rasterizer disagreed, not that the fonts did. Where it does not match, the
declared checks still gate — which is the P6 claim itself — and the run says so instead of passing
quietly. This is the same two-tier arrangement as the frame determinism gate; see
[M3.md](M3.md).

Failures write `<case>.golden.png`, `<case>.actual.png` and `<case>.diff.png` (magenta where the two
disagree) into `target/conformance-divergence`, which CI uploads as an artifact.

Every golden here was recorded on Linux x86-64, and the CI job is the first in the workflow to
build anything on macOS or Windows. Whether those runners share this fingerprint is therefore an
open question the job exists to answer, and its first runs there are more likely to surface
toolchain work than conformance failures. `fail-fast: false` keeps the Linux result readable
meanwhile.

## What is in the corpus

Eleven of the twelve cases are correctness oracles. `react-vite` is a change detector, and says so.
The remaining `defect-*` case is an oracle too — its numbers came from the CSS and were confirmed
against a browser — but what it gates is that Blitz still gets it wrong. `absolute-auto-margins`
was the second of those until the Blitz pin moved past the Taffy fix for it, and is now an ordinary
oracle with a golden of the centred box.

| Case | Covers | Kind |
| --- | --- | --- |
| `block-flow` | Margins, padding, borders, sibling margin collapsing | Oracle |
| `flex-row` | `gap`, padding, `align-items: center`, `flex: 1` free space | Oracle |
| `flex-column` | Column direction, `justify-content: space-between`, cross-axis stretch | Oracle |
| `grid` | `1fr`/`2fr`/fixed tracks, asymmetric `gap`, a spanning item, auto placement | Oracle |
| `absolute` | Insets against a positioned ancestor, including a both-edges stretch | Oracle |
| `borders` | Per-side widths and colours, `box-sizing: border-box`, `border-radius` | Oracle |
| `text` | `@font-face`, font size, `line-height`, weight/style descriptor selection | Oracle |
| `images` | `<img>` intrinsic ratio, `background-image`, `background-size`/`position`/`repeat` | Oracle |
| `host-typography` | Text painting through the host's own fonts | Oracle, no golden |
| `react-vite` | Real React output: centred shell, flex card row, borders, radii, wrapping | Change detector |
| `absolute-auto-margins` | Auto margins against a `max-width`-clamped used width | Oracle |
| `defect-initial-containing-block` | `absolute`/`fixed` insets resolved against the viewport | Known defect |

Every golden image in this repository was rendered on the machine that recorded it and then looked
at. They are not recordings of whatever the renderer happened to do.

`host-typography` is the only case that leaves the pinned font behind, because the bug it guards
against can only happen there: with no font sources, a generic-family run paints nothing at all.
Nothing about it is portable, so it carries no golden and asserts only what is true of any correct
rendering — that black is painted where the run is.

## Real framework output

`react-vite` is the React tree that the committed `vite build` bundle in
[`examples/vite-react`](../examples/vite-react) builds at runtime, captured once by
[`capture.mjs`](../crates/blitsen-blitz/tests/conformance/capture.mjs) after the bundle's own
JavaScript ran, and committed:

```sh
bun run --cwd packages/blitsen conformance:capture
```

Capturing needs the whole runtime; rendering the result needs only the renderer, which is what keeps
the corpus runnable in CI with no bundler, no JavaScript engine and no network. The markup is
verbatim and the app's own stylesheet is inlined unchanged. One rule is added: every element is
pinned to the corpus font. Without it the case would not be portable at all — and that is worth
stating plainly, because it is the honest limit of a golden-image gate. **Real framework output
cannot be held to a byte-exact cross-platform golden while it uses the host's fonts.** What this
case gates is the layout that markup produces given fixed text metrics, which is where a renderer
regression would show; what it cannot gate is how the same page looks on a machine with different
fonts installed.

Only its horizontal geometry is declared, because that follows from the stylesheet alone. Heights
follow from the pinned font, so recording them as expectations would be circular, and they are left
to the golden image.

## What is still manual

- **The s6 spike stays manual.** [`spikes/s6/`](../spikes/s6) compares Blitz against Chromium on
  three pinned upstream apps (React/Shadcn, Vue/Conduit, Svelte/Wordle) and is the evidence behind
  [BLITZ-GAPS.md](BLITZ-GAPS.md). It needs Chromium, ImageMagick, npm, Corepack, Python 3 and
  network, and only its result images are committed — not the applications — so it cannot be made
  CI-runnable as it stands. It answers a different question anyway: *does Blitsen match a browser*,
  where this corpus asks *does Blitsen match itself, everywhere*. Both are needed; only the second
  can be a gate.
- **Reductions that need the network.** A case is parsed from a string and laid out once, with no
  net provider, so a bug that only appears when a subresource arrives late cannot be one. Gap G2 is
  exactly that, and its reduction lives in
  [`tests/reductions/`](../crates/blitsen-blitz/tests/reductions) with the command that shows it.
- **Vue and Svelte have no case.** The repository builds one real framework bundle, React through
  Vite, and that is the one captured. Adding another means adding another buildable example, not
  another corpus entry.

## WPT seeding did not happen

Issue #61 asks for a targeted subset of [WPT](https://github.com/web-platform-tests/wpt) reftests
alongside the framework cases. It was not done, for three reasons worth recording rather than
working around:

1. **Vendoring.** WPT is third-party BSD-3-Clause source. Committing a subset needs an attribution
   entry in [LICENSING.md](LICENSING.md), and that is a licensing decision to make deliberately
   rather than as a side effect of a test corpus.
2. **The comparison model is different.** A reftest asserts that two documents render identically,
   not that one document matches a stored image. That is a genuinely better idiom — it is portable
   by construction and needs no golden at all — but it is a second render path the harness does not
   have. A `@ref <file>` directive is the obvious seam.
3. **Most of the subset would need adapting anyway.** WPT's layout reftests are written against
   Ahem, whose glyphs are solid em blocks — which is precisely what `block-*.ttf` here is, arrived
   at independently for the same reason. Adapting a reftest therefore means retargeting its font
   and re-deriving its expectations, at which point the value is in the case selection rather than
   in the import.

The honest summary: this corpus covers the CSS Blitsen actually claims in
[COMPATIBILITY.md](COMPATIBILITY.md) — block, flex and grid layout, bounded absolute positioning,
spacing, borders, backgrounds, colours and typography — with cases whose expectations were derived
rather than recorded. It is not a WPT subset and does not claim to be.
