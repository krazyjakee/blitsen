# 0.2.0 — server-sent events, `Intl`, and SVG that paints

Blitsen 0.2.0 closes three absences from the compatibility profile. Two of them were things an
application could not work around at all, and the third was waiting on a fix upstream that has
since landed.

The minor version, not a patch: the web surface an application is checked against has grown, the
renderer's pin has moved to upstream Blitz, and the export is 12 MB larger than 0.1.3's. Nothing
that worked in 0.1.3 stops working.

## Added

- **`EventSource`** ([#236](https://github.com/krazyjakee/blitsen/issues/236)). Server-sent event
  streams, on the worker pool `fetch` and `WebSocket` already share, delivered at the same defined
  point in the frame turn — so a feed cannot re-enter an application part-way through a frame.
  Named events, `lastEventId`, `readyState` and `close()` are all there. Reconnection belongs to
  the transport: a stream whose body ends fires `error`, waits the interval a `retry:` field asked
  for, and reconnects carrying `Last-Event-ID`, while a response that is not a
  `200 text/event-stream` settles at `CLOSED` and is not retried.
- **`Intl`** ([#237](https://github.com/krazyjakee/blitsen/issues/237)), implemented natively over
  CLDR through ICU4X, with the platform's own time-zone database. `NumberFormat` (decimal, percent,
  currency, compact), `DateTimeFormat` — including **named IANA `timeZone` values** and their
  daylight-saving history — `RelativeTimeFormat`, `PluralRules`, `Collator` and `ListFormat`, in
  the document and in workers. `Number.prototype.toLocaleString`, `Date.prototype.toLocale*String`
  and `String.prototype.localeCompare` are now the formatters above rather than the engine's
  locale-blind versions, which returned the invariant form whatever locale they were given.

  The two operations with no fallback an application could write for itself are the point of it:
  converting an instant into a named zone, which needs that zone's history of offsets, and
  formatting a currency, which needs the minor units and placement CLDR gives that code in that
  locale — `$1,234.50`, `¥1,235`, `1.234,50 €`.

  Absent, and feature-detectably so: `formatToParts`/`formatRange`, `Intl.Segmenter`,
  `Intl.DisplayNames`, `Intl.DurationFormat` and `Intl.supportedValuesOf`.
- **SVG** ([#238](https://github.com/krazyjakee/blitsen/issues/238)). An inline `<svg>` subtree,
  `<img src="icon.svg">` and CSS `background-image` all paint — as vectors into the same scene as
  the rest of the frame, so they stay sharp at any window scale. Shapes, paths, `viewBox`,
  transforms, gradients, dash patterns and `currentColor` off the CSS cascade all render, `<text>`
  renders where the host's fonts allow (see below), and mutating a subtree from script repaints it. `filter`, `mask`, SMIL animation, `foreignObject`
  and `<pattern>` fills do not; `blitsen doctor` names those constructs specifically rather than
  warning about every `<svg>`.
- **`os.locale()`** in `blitsen/os`: the language tag and IANA zone the session is configured for —
  the two values an application hands to the formatters above. It was absent because nothing
  consumed it, which stopped being true.

## Changed

- **The renderer pin moved to upstream Blitz, and the fork is retired**
  ([#138](https://github.com/krazyjakee/blitsen/issues/138)). The custom-widget layout fix Blitsen
  carried in a fork is upstream, so the workspace has no `[patch]` section at all: a checkout, CI
  and a release all resolve `blitz*` from upstream.
- **Render-blocking stylesheets now block the cascade as well as the paint**, matching browsers.
  While a `<link rel="stylesheet">` in the head is in flight, no element has a resolved style —
  `getComputedStyle` answers with nothing rather than with a pre-stylesheet value. Anything reading
  computed style before the document's own stylesheets have loaded sees this.
- **Auto margins on an absolutely positioned box now resolve against the used width**, so
  `inset: 0; margin: auto; max-width` centres instead of pinning to the leading edge. That is the
  shape most modal dialogs are centred with.
- **The export is 50.9 MB installed, 19.2 MB compressed**, against 38.8 MB and 15.3 MB in 0.1.3.
  The whole of that is `Intl`'s CLDR data and the SVG stack; every CLDR locale ships and there is
  nothing to configure. [`docs/PRODUCT.md`](PRODUCT.md) records the trade and what could be given
  back if the number matters more than the coverage.

## Known limitations

- An SVG `<pattern>` fill does not merely fail to paint: the renderer marks paints it cannot
  convert with a half-transparent red box drawn at the **frame's top-left corner** rather than over
  the element. It belongs to `anyrender` rather than to Blitz, is gated as gap G16 in
  [`docs/BLITZ-GAPS.md`](BLITZ-GAPS.md), and `doctor` reports it at build time.
- A currency's fraction digits are the currency's: `minimumFractionDigits` and
  `maximumFractionDigits` do not override CLDR for `style: "currency"`. `resolvedOptions()` reports
  the digits actually used, so the disagreement is detectable rather than silent.
- **SVG `<text>` can silently render nothing on a host where HTML text renders fine**: the two use
  different font discovery, and where they disagree the text lays out and paints no glyphs. It
  happens on GitHub's own Linux runner. Icons are unaffected — they are paths — but chart labels
  inside an `<svg>` are not. Gap G17 in [`docs/BLITZ-GAPS.md`](BLITZ-GAPS.md).
- An SVG subtree is re-parsed rather than patched when it changes, so a chart that rewrites its
  paths every frame pays a parse every frame. For per-frame drawing, use `<blitsen-view>`.

## Validation

`cargo test --workspace`, `cargo clippy --all-targets`, the JavaScript suite, the native harness,
the layout conformance corpus, the frame determinism gate, the size gate and the
redistribution/licensing gate all pass. The determinism golden's pixel digests were re-recorded:
DOM and layout digests are byte-identical across the renderer bump and every conformance golden is
unchanged, with the frame-to-frame difference measured at 0.37% of pixels differing by at most
1/255 — rasteriser rounding rather than a rendering change.

As in every release so far, published artifacts are unsigned and are not notarised. See
[`docs/RELEASING.md`](RELEASING.md) for the distribution and signing model.
