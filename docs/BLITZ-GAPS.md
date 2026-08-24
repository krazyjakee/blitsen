# Blitz rendering gaps — standing list

Blitsen renders through [Blitz](https://github.com/DioxusLabs/blitz), at the exact revision declared
by the workspace [`Cargo.toml`](../Cargo.toml). This is the live list of rendering divergences we
know about, with status. It supersedes the frozen
catalogue in [`spikes/s6/README.md`](../spikes/s6/README.md), which was a dated snapshot from one
run and is kept only as the evidence behind the initial triage.

**Scope.** A gap belongs here when the renderer draws something differently from a browser, or not
at all. Things Blitsen has simply not implemented — `<video>`, WebGL, navigation — are capability
tiers, not gaps; those live in [`COMPATIBILITY.md`](COMPATIBILITY.md). (`fetch`, images, web fonts
and `<canvas>` were in that sentence once and are implemented now.)

**"Blitz" here is four layers, and a row has to say which one it sits in**, because that decides
where it gets filed: stylo's cascade, `blitz-dom`'s layout and DOM, `blitz-paint`'s scene building,
and the [anyrender](https://github.com/DioxusLabs/anyrender) backend that rasterises the scene —
`anyrender_vello` for the window, `anyrender_vello_cpu` for headless renders and the tests. G9 is
the first row that turned out to live in the backend rather than in Blitz, and it looked exactly
like a Blitz gap until it was reduced.

Status values: **open** (reproduced, unfiled), **filed** (upstream issue exists), **fixed**,
**withdrawn** (turned out not to be a gap in the renderer).

| # | Gap | Status | Evidence | Notes |
| --- | --- | --- | --- | --- |
| G1 | Digits absent from some text runs while letters and punctuation render | filed | `spikes/s6/results/blitz-snapshot/react.png` | [blitz#688](https://github.com/DioxusLabs/blitz/issues/688); see below |
| G2 | A property named by `transition` keeps its pre-stylesheet value for good | filed | `crates/blitsen-blitz/tests/reductions/transition-stale-style.html` | [blitz#689](https://github.com/DioxusLabs/blitz/issues/689); the hidden overlays were a symptom. Now reachable through Blitsen, which it was not when this was written; see below |
| G3 | `absolute`/`fixed` insets resolve against the wrong box | filed, half fixed | `crates/blitsen-blitz/tests/conformance/cases/defect-initial-containing-block.html` | [blitz#690](https://github.com/DioxusLabs/blitz/issues/690), [blitz#691](https://github.com/DioxusLabs/blitz/issues/691). The auto-margin half is **fixed** — Taffy's fix arrived with the pin, and its case is now an ordinary oracle. The initial containing block is still wrong and still gated |
| G4 | Form controls and anchors fall back to native/default styling | open | `crates/blitsen-blitz/src/tests/ua.rs`; `spikes/s6/results/blitz-snapshot/{react,vue}.png` | Blitz ships a trimmed `html.css` and no `forms.css` or `ua.css` at all. The half of that baseline this engine can honour now ships in `crates/blitsen-blitz/src/ua.rs`; the controls with no widget behind them do not. See below |
| G5 | SVG paints nothing in Blitsen's build | fixed | `crates/blitsen-blitz/src/tests/svg.rs` | The `svg` feature it needed compiles upstream now (G7), so Blitsen enables it: inline `<svg>`, `<img src="*.svg">` and `background-image` all paint, as vectors. What is left unpainted is G16. See below |
| G6 | Accumulating line-height, antialiasing and vertical spacing differences | open, low priority | `spikes/s6/results/metrics.tsv` | Needs a tolerance corpus before it can be actioned |
| G7 | `blitz-dom`'s `svg` feature does not compile at the pinned revision | fixed | The build, which now compiles it | [blitz#687](https://github.com/DioxusLabs/blitz/issues/687); upstream re-adopted `svg` as a default feature and moved to usvg 0.48, and the pin moved past it. See below |
| G8 | `@keyframes` animations never move | withdrawn | `crates/blitsen-blitz/src/lib.rs::tests::animations_stand_still_until_the_host_supplies_a_clock` | Not a Blitz gap: Blitz animates keyframes correctly. Blitsen was resolving every frame at time zero; see below |
| G9 | `filter` and `backdrop-filter` paint nothing at all | open | `crates/blitsen-blitz/tests/conformance/cases/defect-filter-ignored.html` | Not Blitz: `blitz-paint` builds the filter layer and both anyrender backends drop it. `clip-path` and `mask-image` do work. See below |
| G10 | Changing checkedness does not invalidate the cascade, so a `:checked` rule never repaints | open | `blitz-dom`'s `stylo.rs`, `document.rs` and `events/pointer.rs` at the pin; measured in pixels | Blitz's own click on its own checkbox has it too; see below |
| G11 | A subresource whose handler never completes blocks painting for the life of the document | open | `crates/blitsen-blitz/tests/conformance/cases/unservable-subresource.html` | `NetProvider`'s only failure signal is dropping the handler, which is also the thing that blocks. Worked around in `resources.rs`; see below |
| G12 | Blitz has no `[hidden]` user-agent rule | withdrawn | `docs/COMPATIBILITY.md`, "Rendered text, box reads and scrolling" | It has the rule. The probe that said otherwise put an author `display: block` on the element, which correctly wins the cascade |
| G13 | Link-ness never reaches `ElementState`, so the style-sharing cache hands one anchor's style to another | open | `crates/blitsen-blitz/src/tests/ua.rs::only_an_anchor_with_an_href_is_painted_as_a_link` | stylo's cascade layer: `:any-link` and `:link` are matched ad hoc in `blitz-dom`'s `stylo.rs`, so two sibling anchors of opposite kinds are sharing candidates. Same shape as G10. See below |

| G14 | A replaced element panics in layout the moment it carries a custom widget | fixed | `crates/blitsen-blitz/src/tests/canvas.rs::canvas_survives_carrying_a_custom_widget` | [blitz#706](https://github.com/DioxusLabs/blitz/issues/706), fixed by [blitz#719](https://github.com/DioxusLabs/blitz/pull/719). The fork that carried the patch is retired and the workspace has no `[patch]` section left (#138). See below |
| G17 | SVG `<text>` is resolved against a font database that can be empty on a host where HTML text renders | open | `crates/blitsen-blitz/src/tests/svg.rs::text_inside_an_svg_is_painted_as_glyphs_wherever_the_host_has_one`; GitHub's Linux runner | `blitz-dom` hands usvg a `fontdb` built by `load_system_fonts()`, while HTML text is shaped by Parley through the platform's own font discovery. The two disagree, and where they do the text vanishes silently. See below |
| G16 | An SVG paint the renderer cannot convert marks the frame's corner rather than the element | open | `crates/blitsen-blitz/src/tests/svg.rs::an_unsupported_pattern_fill_marks_the_frame_corner_rather_than_the_element` | Not Blitz: `anyrender_svg`'s error handler fills the node's bounding box under the *identity* transform. A `<pattern>` fill anywhere leaves a half-transparent red box at 0,0. See below |
| G15 | `pointer-events` accepts only `auto` and `none`, so `all` is dropped and the element inherits | open | `crates/blitsen-blitz/src/tests/stylesheets.rs::an_element_that_declares_it_takes_hits_inside_one_that_does_not_is_hit` | stylo's cascade layer: the other nine values are `#[cfg(feature = "gecko")]`, which needs Gecko's bindings and cannot be enabled by an embedder. Worked around in `pointer_events.rs`; see below |
| G18 | An opacity on an image paint aborts the process rather than drawing | open | `packages/blitsen/test/native-harness/canvas.mjs`, the half-transparent `drawImage` | anyrender backend: `vello_common`'s encoder reaches `unimplemented!("Applying opacity to image commands")`, which is a panic across the native boundary. Worked around in `canvas/wire.rs`; see below |
| G19 | A compose function applies outside its own layer's clip | fixed | `crates/blitsen-blitz/src/tests/canvas.rs::a_destructive_composite_erases_the_canvas_and_not_the_page_behind_it` | anyrender backend: a `Copy` or `Clear` layer clipped to a rectangle cleared the whole surface. Gone at the `111ed2f7` pin; the encoder already scoped its layers correctly and needed no change. See below |

Broad Tailwind/renderer work has an upstream collection in
[blitz#389](https://github.com/DioxusLabs/blitz/issues/389).

**Where these surface for an application author.** Three of these rows are what the `doctor`
diagnostics in [`COMPATIBILITY.md`](COMPATIBILITY.md#diagnostic-severity) are describing:
`CSS_TRANSITION` is G2, `CSS_FIXED` is G3, `CSS_EFFECT` is G9. All three are warnings rather than
errors, because a degraded page beats a refused build.

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
s6 images were made.

**The last paragraph of this entry used to say it was unreachable through Blitsen, because Blitsen
had no net provider. That is no longer true.** The runtime installs `blitz::net::Provider`
(`crates/blitsen-host/src/native_window.rs`), which loads a `<link>` stylesheet off the frame thread, and the
dev server reloads linked sheets rather than inlining them. So the precondition — a sheet that
lands *after* the first style resolution — is now something an ordinary application can hit. What
has not been done is measuring whether it then reproduces: the renderer tests use
`LocalResources`, which answers inside `fetch` and so hands the sheet over before the first
resolution, which is why this is still a reduction and not a conformance case. **Next step:**
resolve a document with a slow-arriving sheet through the runtime's provider. If it reproduces,
this becomes a gated case; if it does not, the entry needs the reason why not.

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
- **Auto margins were resolved from the declared width, not the used one** — and that half is
  **fixed**. With both insets given, `margin: auto` and a width that `max-width` clamped, the free
  space was computed against the unclamped width, which left no margin and pinned the box to the
  leading edge instead of centring it. The fix is Taffy's and arrived with the Blitz pin; the case
  is now an ordinary oracle with a golden image rather than a known failure, and lives at
  [`absolute-auto-margins.html`](../crates/blitsen-blitz/tests/conformance/cases/absolute-auto-margins.html).

The initial containing block is what is left of the row, and it is why Wordle+'s modals stop short
of the viewport. It reproduces in upstream Blitz's `screenshot` example as well as in Blitsen's
renderer, and the numbers in both cases were confirmed against Chromium at the same viewport.

## G4 — the missing sheet is `forms.css`, and half of it we can ship ourselves

Blitz's `packages/blitz-dom/assets/default.css` is Gecko's `html.css` with the internal
pseudo-classes trimmed out, plus a fifty-line form-control shim. Gecko's other two user-agent
sheets — `forms.css` (965 lines) and `ua.css` (567) — have no counterpart. That is not a decoration
gap: it is the part of the baseline no application writes down, because every browser supplies it.
Compared against the sheets extracted from a local Firefox build, and confirmed by headless renders
of the controls:

- **No `cursor` declaration exists anywhere in the sheet.** Every element resolves `cursor: auto`,
  so `BaseDocument::get_cursor` falls through to its text-hit fallback and a button hovers with the
  text caret. Gecko sets `cursor: default` and `user-select: none` on buttons (`forms.css`), and
  `:any-link { cursor: pointer }` (`ua.css`).
- **`:disabled` never appears in a rule**, so a disabled control paints identically to a live one.
  The pseudo-class itself matches fine.
- **`fieldset` and `legend` have no rules**, so a fieldset is inline and borderless and its legend
  runs into the contents on one line.
- **Every `<a>` is coloured and underlined**, including one with no `href`.
- **`<select>`, `<meter>` and `<progress>` paint nothing at all** — no box, no border, no text — and
  `input[type=range|color|number]` collapse to a four-pixel bordered box with no widget and no value.
- **`::placeholder` renders nothing** and `:placeholder-shown` is hardcoded to `false`
  (`stylo.rs`); **`:focus-visible` is hardcoded to `false` too**, so nothing but `input` and
  `textarea` — which use `:focus` — can show a focus ring.
- **`<table border="1">` draws no borders.** The `border` attribute is not one of the presentation
  attributes `stylo.rs` maps, and the sheet's rules for it are keyed on Gecko's
  `:-moz-table-border-nonzero`, which is not implemented — so they can never match. (The `[frame]`
  and `[rules]` rules beside them are ordinary attribute selectors and do work.) `cellpadding` and
  `cellspacing` are not mapped either.
- **A closed `<details>` still shows bare text children**, because the rule that hides them can only
  reach elements.

The first four, plus `textarea`'s `white-space: pre-wrap`, are pure cascade and now ship in
`crates/blitsen-blitz/src/ua.rs`, appended after Blitz's own sheet and below anything an author
writes. The rest need a widget or engine support before a rule would mean anything, and are left
alone deliberately: a UA rule for a control nobody paints is a lie in a stylesheet.

## G5 — SVG paints, and what it does not paint is one list

The s6 catalogue described icons that were missing or geometrically wrong, and chart fills that were
the wrong colour. Both descriptions were of a build, not of SVG: upstream's `screenshot` example
built `blitz-paint` with its default `svg` feature and got the wrong geometry, and Blitsen built
without it — because it did not compile (G7) — and got a correctly-sized box with nothing at all
inside it.

Both are now history. The feature compiles upstream, the pin has moved past the fix, and Blitsen
turns it on in both halves: `blitz-dom/svg` parses an `<svg>` subtree and an SVG subresource with
usvg, and `blitz-paint/svg` paints the parsed tree through `anyrender_svg` into the same scene as
the rest of the frame. So an SVG is *vector* output rather than a rasterised image, and stays sharp
at any window scale. `<img src="icon.svg">` and `background-image: url(icon.svg)` work for the same
reason and take the intrinsic size the file declares.

What paints and what does not is measured rather than asserted, in
[`src/tests/svg.rs`](../crates/blitsen-blitz/src/tests/svg.rs): shapes, paths, `viewBox`,
transforms, fills and strokes including `currentColor` off the CSS cascade, dash patterns, linear
and radial gradients, `opacity`, a single-path `clipPath` and `<text>` all paint; `filter`, `mask`,
SMIL animation, `foreignObject`, multi-path `clipPath` and `<pattern>` fills do not. The list is in
[COMPATIBILITY.md](COMPATIBILITY.md#svg), and `doctor`'s `HTML_SVG` warning now names those
constructs rather than every `<svg>` element.

One more thing worth knowing, and it is G16 rather than this row: a `<pattern>` fill does not merely
fail to paint, it marks the frame.

## G7 — `blitz-dom`'s `svg` feature did not compile

Enabling `blitz-dom`'s `svg` feature at the old pin failed:

```
error[E0432]: unresolved import `usvg::svgtypes`
error[E0599]: no method named `intrinsic_dimensions` found for struct `std::sync::Arc<usvg::Tree>`
```

The feature had drifted behind its `usvg` dependency, and upstream had not hit it because the
meta-crate never enabled it. Filed as [blitz#687](https://github.com/DioxusLabs/blitz/issues/687)
and **fixed**: upstream moved to usvg 0.48, took `svgtypes` as a dependency of its own, and made
`svg` a *default* feature of both `blitz-dom` and `blitz-paint`. Blitsen still names its features
explicitly rather than taking `default` — the list is a decision this workspace records — but `svg`
is now in the list, which is what closed G5.

## G8 — keyframes animate; the clock was ours

Asked before implementing the CSSOM stylesheet API ([#117](https://github.com/krazyjakee/blitsen/issues/117)):
a `@keyframes` rule a framework inserts from JavaScript is worse than a missing API if it parses,
cascades, and never runs, because the application believes its transition is playing. So the
question came first, and it was measured in pixels rather than read out of the source.

**Blitz animates `@keyframes`.** A 40×40 box with
`animation: slide 2s linear both` over `@keyframes slide { from { margin-left: 0px } to
{ margin-left: 200px } }`, resolved at 0.0, 0.5, 1.0, 1.5 and 2.0 seconds and painted each time,
has its inked bounds at x = 0, 50, 100, 150 and 200. `opacity` and `transform` interpolate the same
way — at 0% of a fade the box paints nothing at all. Stylo's animation machinery is wired up in
`blitz-dom`'s `stylo.rs`, and `BaseDocument::resolve` takes the sample time as its only argument.

**What did not move was Blitsen.** Every `flush_layout` called `resolve(0.0)`, so the same document
painted through Blitsen's own boundary stood at x = 0 for as many frames as it was given. Nothing
in Blitsen ever advanced the animation clock, and no test noticed, because every animation started
at zero and stayed at its first keyframe — which looks exactly like a document that has settled.

Fixed in Blitsen rather than upstream: `DomBackend::set_animation_time` takes the frame's
timestamp, the bridge sets it from the same value it hands `requestAnimationFrame`, and a document
with animation left to run reports itself as owing a frame so the loop keeps turning. The clock
only moves forward, and it comes from the host rather than a wall clock, so replay and the recorded
frame goldens stay deterministic. See "The animation clock" in [COMPATIBILITY.md](COMPATIBILITY.md).

Reproduction, both halves, in `crates/blitsen-blitz/src/lib.rs`:
`animations_stand_still_until_the_host_supplies_a_clock` and
`an_inserted_keyframes_rule_animates_with_the_frame_clock`.

## G9 — the filter gap is not Blitz's; it is the backend's

`defect-filter-ignored.html` gates the observable half: `filter: invert(1)` over black paints
black, so a page that needed the filter to be legible is not. The case calls it a Blitz defect,
which is one layer off. Reduced, it lands somewhere else entirely.

What is measured, at the pin, painted through `anyrender_vello_cpu` (the headless and test path;
the window's `anyrender_vello` behaves the same by inspection):

- `filter: opacity(0)` paints at full opacity, and `invert(1)` over black stays black.
- `filter: blur(6px)` bleeds nothing outside the border box, and `drop-shadow` draws nothing.
- `backdrop-filter: invert(1)` over a black box leaves it black.
- `clip-path: inset(0 0 0 50%)` **works**: the left half is transparent, the right half painted.
- `mask-image: linear-gradient(to right, transparent 0 50%, #000 50% 100%)` **works**: the left
  half is masked away and the right half is opaque.

So the four properties `doctor` groups under `CSS_EFFECT` are not one behaviour. Two of them
render. **Narrowing that rule to `filter` and `backdrop-filter` is a follow-up**, and until it
happens the diagnostic over-warns on `clip-path` and `mask`.

`blitz-paint` is not where the effect is lost. It converts stylo's filter list and hands it to
`Scene::push_layer` (`convert_filters`, `packages/blitz-paint/src/render.rs`) — the layer, the
expansion rect and the backdrop filter are all built. It is the backend that drops it:

- `anyrender_vello` 0.13 names both parameters `_filter` and `_backdrop_filter` in `push_layer`
  and ignores them. There is no feature to turn on. That is the **window** renderer.
- `anyrender_vello_cpu` 0.15 keeps `filter` only behind a `filters` feature that is not in its
  defaults — so Blitsen, which takes the crate with defaults, compiles it out — and drops
  `backdrop_filter` outright. Even with the feature on, the conversion is suppressed whenever
  `multithreading` is enabled.

Enabling `filters` locally does not fix it, and makes it worse: everything that converts —
`blur`, `drop-shadow`, `grayscale`, `sepia`, `hue-rotate` — panics inside the pinned `vello_cpu`
(`unreachable!()` in `fine/mod.rs`, a `PushZeroClip` command reaching fine rasterisation), and
everything built as a component transfer stays ignored, because the backend converts that variant
to `None` — `invert` and `opacity` measured, `brightness` and `contrast` by the same path. So there
is no configuration of the pinned stack where a CSS filter paints. That was measured by enabling
the feature, running the probes, and reverting; nothing in the tree carries it.

**Next step:** file against anyrender rather than Blitz — `anyrender_vello` has no filter support
at all, and `anyrender_vello_cpu`'s is gated off and panics when ungated — and correct the header
of `defect-filter-ignored.html`, which attributes it to Blitz.

## G10 — `:checked` matches; nothing tells the cascade it changed

A component library that styles its own checkbox, radio or switch through `:checked` renders the
unchecked state forever. The selector is not the problem — the invalidation is.

Measured: a document giving `#flag` a black background, with a sibling rule
`#box:checked ~ #flag` that makes it red. After `set_form_checked(box, true)` and a layout flush,
the middle of `#flag` is still `#000000ff`. Touching an unrelated attribute on the same element
afterwards — which does force a restyle — repaints it `#ff0000ff`. So stylo evaluates the rule
correctly and matches the flag; it is simply never asked to re-evaluate it.

The reason is in `blitz-dom` at the pin. Checkedness lives in
`SpecialElementData::CheckboxInput(bool)`, and `:checked` matches straight off that
(`NonTSPseudoClass::Checked` in `packages/blitz-dom/src/stylo.rs`). Every path that writes it
flips the bool in place with no stylo snapshot and no restyle hint: `BaseDocument::toggle_checkbox`
and `toggle_radio` in `document.rs`, and the click handler in `events/pointer.rs`. Attribute writes
go through the mutator, which *does* snapshot — which is why the unrelated poke above worked, and
why this looks like a scripting-only problem when it is not. **A real click on Blitz's own
checkbox has the same gap.**

Blitsen could force a restyle when it writes checkedness from JavaScript, the way an attribute
write already does; that would cover the scripted path and not the clicked one, which is Blitz's
own handler. Fixing it properly means snapshotting the element the way an attribute change does.

**Next step:** a committed reduction — this is currently a measurement plus source, and the rules
below ask for a test — and an upstream issue.

## G11 — a subresource that never answers blocks the page forever

Blitz holds a stylesheet as a pending critical resource until its `NetHandler` completes, and the
only failure signal the trait has is *dropping* the handler — which never completes it. So a
provider that stays silent about what it will not serve does not degrade the page, it blanks it,
for the life of the document.

That is a whole application, not a corner: shadcn-admin mounted 364 elements correctly and
rendered 1,024,000 pixels of pure white, because its `<head>` links a Google Fonts stylesheet the
local provider refuses. `unservable-subresource.html` gates it with two unservable links — one
remote, one a local file that does not exist — and a box below them that has to paint anyway.

Blitsen works around it in `crates/blitsen-blitz/src/resources.rs` by answering everything it will
not serve with an empty body, which an empty stylesheet contributes nothing to and a broken image
fails to decode from. `ResourceLog::stop` completes every abandoned handler for the same reason,
so `window.stop()` cancels loading instead of blanking the document. The cost is that an empty
body is now indistinguishable from a failure, which is why the tracker grades one as the other.

**Next step:** file it. The fix upstream is a failure signal on the trait — a provider should be
able to say "I will not serve this" without lying with an empty body, and a dropped handler should
release the resource rather than pin it.

## G13 — an anchor can be handed the style of the anchor next to it

Writing G4's "an `<a>` with no `href` is not a link" rule as `a:not(:any-link)` produced a result
that depended on document order: whichever anchor came first won, and its sibling of the opposite
kind resolved the same colour and decoration. Two anchors, one with `href` and one without, in a
document with no author styles: put the link first and the bare anchor is blue and underlined; put
the bare anchor first and the *link* is black with no underline.

The rule is fine; the sharing is not. `blitz-dom`'s `stylo.rs` matches `:any-link` and `:link` ad
hoc off the element's tag and `href` attribute, and never records link-ness in `ElementState`.
stylo's style-sharing cache keys on element state and revalidates the rest through selectors it
knows are unreliable — attribute selectors among them — so two sibling `<a>`s with the same tag and
no attribute dependency look interchangeable to it, and one gets the other's computed style. Adding
*any* attribute-dependent rule for anchors to the document makes the correct result reappear, which
is what makes this so easy to mistake for a cascade bug in your own sheet.

This is G10's shape again — state that the cascade cannot see because it never enters
`ElementState` — one step further along: G10 loses an invalidation, this loses the sharing check.

Blitsen's baseline sheet sidesteps it by keying on the attribute (`a:not([href])`), which is a
revalidation selector, and the test asserts both document orders so a change upstream cannot
quietly reintroduce it.

**Next step:** an upstream issue, with the two-anchor reduction.

## G14 — the custom-widget seam and the replaced elements are mutually exclusive

`set_custom_widget` writes `SpecialElementData::CustomWidget` into the one slot replaced layout
reads. `blitz-dom`'s `layout/mod.rs` matches `Image`, then `Canvas | SubDocument | None`, then
`_ => unreachable!()` — so a `<canvas>`, `<img>`, `<svg>`, `<video>`, `<embed>` or `<iframe>`
carrying a widget reaches the last arm and takes the process down on the next layout.

This is the one gap so far with no workaround available on Blitsen's side. `<blitsen-view>` only
avoids it by not being a replaced element: it is an unknown tag given a replaced element's box by
user-agent CSS, which is why `viewport.rs` carries that sheet. Tag membership in
`is_replaced_element` is not something CSS can opt out of, and the alternative paint path —
`SpecialElementData::Canvas` with a `custom_paint_source_id` — is inert at the pin, because
`blitz-paint` never reads `canvas_data`.

What makes it worth patching rather than deferring: everything else `<canvas>` needs from Blitz is
already correct. `<canvas>` is in `is_replaced_element`, and `layout/mod.rs` already implements the
spec sizing — intrinsic size from the `width`/`height` content attributes, defaulting to 300×150,
with an intrinsic aspect ratio that only `<canvas>` gets. Measured through Blitsen: a
`<canvas width="200" height="100">` lays out at 200×100, and adding `style="width: 50px"` gives
50×25.

Filed as [blitz#706](https://github.com/DioxusLabs/blitz/issues/706) and **fixed upstream** by
[blitz#719](https://github.com/DioxusLabs/blitz/pull/719), which took the shape the fork carried and
went further: `Widget` gained `intrinsic_size()` and `aspect_ratio()`, so a widget can size the
element it is attached to, and the sizing body it shares with `<canvas>` is factored into
`default_object_size`. The pin has moved past it, the fork is retired, and the workspace has no
`[patch]` section at all any more — which closes [#138](https://github.com/krazyjakee/blitsen/issues/138)
as well. What that is worth beyond the panic: a checkout, CI and a release all resolve `blitz*`
from upstream, so nobody has to trust a second repository for the renderer to build.

## G15 — `pointer-events: all` is not an invalid value, but the cascade drops it as one

`stylo`'s `PointerEvents` enum has eleven variants and nine of them are `#[cfg(feature = "gecko")]`.
Without that feature — which needs Gecko's bindings, so no embedder has it — the property parses
`auto` and `none` and nothing else, and every other value is an invalid declaration.

An invalid declaration is dropped, and `pointer-events` inherits, so the element keeps whatever its
ancestor had. That inverts the author's meaning in the one pattern the property exists for: a
container that takes no hits holding elements that declare that they do. React Flow ships exactly
that — `.react-flow__nodes { pointer-events: none }` around `.react-flow__node { pointer-events:
all }`, and `.react-flow__handle { pointer-events: none }` overridden by
`.connectionindicator { pointer-events: all }` — so on the [React Flow example](../examples/reactflow)
*nothing on the canvas was hit-testable at all*: not `elementFromPoint`, not a click, not the
cursor. It is the second half of issue #128, and the reason the first half alone did not fix it.

Worked around in `crates/blitsen-blitz/src/pointer_events.rs`, which rewrites the nine dropped
values to `auto` as CSS enters the document — the parsed tree, a `<style>` element's text, a `style`
attribute, one CSSOM property, and a linked sheet's bytes. All nine mean "this element takes hits";
only `none` does not. The cost is a readback: `getComputedStyle(el).pointerEvents` reports `auto`
where a browser reports the author's keyword. Rewriting the cascade's input rather than special-
casing the hit test is deliberate — a rule the hit test honoured but the cascade denied would be a
divergence between two answers about the same element, which is worse than one honest answer.

## G17 — SVG text and HTML text do not look for fonts in the same place

`blitz-dom` builds one font database for usvg and hands it every SVG it parses:

```rust
pub(crate) static FONT_DB: LazyLock<Arc<fontdb::Database>> = LazyLock::new(|| {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    Arc::new(db)
});
```

HTML text does not use it. That goes through Parley, which discovers fonts the platform's own way —
fontconfig on Linux, Core Text on macOS, DirectWrite on Windows. `fontdb::load_system_fonts` scans a
fixed list of directories instead, and the two do not always agree.

Where they disagree the failure is silent and total: the `<text>` element lays out, contributes no
ink, and everything around it paints normally. This is not hypothetical — it is how it was found.
The layout conformance corpus renders HTML text on GitHub's Linux runner, and on the same runner in
the same job an `<svg><text>` rendered nothing at all, while both render here. A machine with fonts
by any ordinary definition can still be a machine where SVG text disappears.

Two things follow. An icon set is unaffected — icons are paths — but a *chart* is not, because axis
labels are text, and an application that draws its labels inside the SVG can lose them on a machine
its author never tested. And a test cannot assert SVG glyphs unconditionally, which is why
`svg.rs`'s text case gates the part that is true everywhere — the shapes beside the text still paint
and nothing panics — and says on stderr which of the two hosts it ran on.

**Next step:** file against Blitz. The fix is upstream's to choose: seed that database from the same
discovery Parley uses, or give usvg a fallback face so a resolvable family always exists.

## G16 — an SVG paint that cannot be converted marks the corner of the frame

`anyrender_svg` hands any node whose paint it cannot convert to an error handler, and the default
handler — the one `blitz-paint` uses — fills that node's bounding box with red at half alpha:

```rust
pub(crate) fn default_error_handler<S: PaintScene>(scene: &mut S, node: &usvg::Node) {
    let bb = node.bounding_box();
    scene.fill(Fill::NonZero, Affine::IDENTITY, RED.multiply_alpha(0.5), None, &rect);
}
```

`Affine::IDENTITY` is the bug. Every other paint in that renderer is drawn under the node's own
transform composed with the element's; the mark is drawn under neither, so it lands at the bounding
box's *local* coordinates — which for a typical `<rect x="0" y="0">` is the top-left corner of the
whole frame. The element that could not be painted gets nothing, and something entirely unrelated
gets a red wash.

`<pattern>` is the paint that reaches it in practice, because `to_brush` returns `None` for
`usvg::Paint::Pattern`. Measured in
[`src/tests/svg.rs`](../crates/blitsen-blitz/src/tests/svg.rs): a patterned rect 100px to the right
of a blue tile paints nothing where the rect is, and turns the tile from `#2563eb` into `#933275` —
red at half alpha over blue — at the origin.

Two separate things are wrong and both are worth saying upstream: the mark is misplaced, and a
missing feature that *silently* draws nothing is friendlier than one that stains the page. Neither
is Blitz's: this is `anyrender_svg`, the same layer G9 lives in. **Next step:** file against
anyrender, and until then `doctor`'s `HTML_SVG` names `<pattern>` explicitly so a build that has
one is told before a user sees it.

## Keeping this list honest

- Every row needs evidence that can be re-examined — a committed image, a test, a build failure, or
  a named source line at the pinned revision. "Looked wrong once" is not a row.
- A row moves to **filed** only with an issue number, and to **withdrawn** with the reason.
- A row with a reduction says what the reduction shows, not what the first screenshot suggested.
  Four rows have now changed description under reduction — G1, G2, G3 and G5 — and each time the
  original wording would have sent a maintainer looking for a bug that does not exist. Three have
  since been fixed upstream and say so rather than being deleted: G5, G7 and G14. A fixed row is
  evidence that the filing worked, and the pin bump that closed them is the reason to keep reading
  the rest.
- **A row names the layer it lives in.** Three rows have now moved between layers: G8 was Blitsen's
  clock, not Blitz's animations; G9 is the anyrender backend, not `blitz-paint`, which builds the
  filter layer correctly; G12 was a probe that styled the element it was probing. Two more —
  the hit-testing bug that walked DOM parents instead of layout parents, and `getClientRects` —
  were Blitsen's own and never became rows. A row filed at the wrong layer is worse than no row.
- Reduce until the property is isolated, not until the page looks wrong. `CSS_EFFECT` grouped four
  properties on one screenshot; two of them render correctly, and only measuring each one alone
  showed which.
- Re-running the s6 corpus needs Chromium, ImageMagick, npm, Corepack, Python 3 and network
  access, so it is not a CI gate, and it stays manual. The gated corpus is the golden-image one
  in [CONFORMANCE.md](CONFORMANCE.md), which asks the other question: not whether Blitsen matches
  a browser, but whether it matches itself on every platform. A gap belongs here when a browser
  disagrees with Blitz; a conformance case belongs there when Blitsen has to keep agreeing with
  itself.

## G18 — an opacity on an image paint is a panic, not a slow path

`peniko::ImageSampler` carries an `alpha`, and every path through Blitsen that could set it now
sets `1.0` instead. Not as a simplification: `vello_common`'s paint encoder reaches

```rust
if sampler.alpha != 1.0 {
    unimplemented!("Applying opacity to image commands");
}
```

which is a panic, and a panic raised from inside a paint is not a wrong picture — it is the
process. It was found by the canvas demo the first time it drew a pattern at `globalAlpha = 0.35`.

The encoding is what carries the fix rather than a check at the call site. `canvas::wire`'s image
paint has no alpha field at all and `canvas::record` writes `1.0` literally, so there is no
argument to get wrong: an opacity on an image is a compositing layer the caller opens, which the
backend does implement. That costs one layer per drawn image at less than full opacity, and
nothing at all at full opacity — which is what a canvas does almost all of the time.

## G19 — a destructive compose function was not bounded by its layer — fixed

`push_layer` takes a blend mode *and* a clip, which reads as "compose these two things inside this
shape". At the `1efe22d2` pin the backend composed them everywhere: a layer with `Compose::Copy`
and a 4×4 clip replaced the destination inside that rectangle **and cleared the rest of the
surface**. It was measured directly — fill the canvas red, push a `Copy` layer clipped to a small
square, draw into it, pop: the square was right and everything outside it was transparent.

**It is fixed at the `111ed2f7` pin this workspace uses**, and the fix cost nothing on this side,
because the encoder had never depended on the defect. A composite operation other than
`source-over` is opened as a layer over the *whole canvas* — that is the scope the specification
gives it, `source-in` clearing the canvas everywhere the drawn shape is not — so the five
destructive compose functions (`copy`, `source-in`, `source-out`, `destination-in`,
`destination-atop`) erase exactly what they should whether or not the clip is honoured. What the
defect used to do was make the *same* picture out of a narrower layer, which is why nothing broke
when it stopped.

The two operations that must be bounded still are. `clearRect` and `putImageData` erase a rectangle
and nothing else, and both go through `destination-out` — whose result for an absent source is the
destination unchanged — rather than `Clear` or `Copy`, which would take the canvas with them. That
was the right encoding under the defect and it is still the right encoding without it. A
full-canvas `clearRect` does not reach a layer at all: it is recorded as "forget everything", which
is cheaper than any of this.

What remains is a destructive composite operation used *inside* a `clip()`, which erases within the
clip's layer rather than across the canvas a browser would clear. That is a scoping question this
encoder has not answered rather than a backend defect, and COMPATIBILITY.md states it under the
canvas surface.
