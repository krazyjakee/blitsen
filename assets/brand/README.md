# Blitsen brand marks

A reindeer, drawn the way one would appear on a family crest — traditional, formal, flat, no
shield or frame around it. The chosen mark is the beast **rampant**: rearing on its hind legs,
head in profile, antlers thrown back.

## The mark

| File | Use |
| --- | --- |
| `svg/blitsen-rampant.svg` | **Primary.** Everything at 48 px and above. |
| `svg/blitsen-rampant-icon.svg` | The same pose redrawn heavier for small sizes. 16–32 px only. |

The full mark has the better line — finer antlers, more articulated legs — but that detail turns
to mush below about 48 px. The small-size cut thickens the limbs, closes the gap between body and
foreleg, and drops each antler to three short tines, so the silhouette survives. Compare them at
matched sizes in `rampant-sizes.png`.

Use the full mark by default. Reach for the cut only when the render is genuinely tiny.

## Alternates

Not chosen, kept because they are already drawn and traced:

| File | Heraldic term |
| --- | --- |
| `svg/blitsen-rampant-bolt.svg` | rampant, attires of lightning |
| `svg/blitsen-mark.svg` | head affronté, attires of lightning |
| `svg/blitsen-head.svg` | head affronté |
| `svg/blitsen-courant.svg` | courant — springing, in profile |

The lightning attires are a nod to Blitzen, if the plain beast ever needs the pun made explicit.
`contact-sheet.png` shows the whole explored set.

## Colour

Every SVG is a single `<path>` with `fill="currentColor"`, so it inherits the surrounding text
colour and needs no separate dark-mode file:

```html
<span style="color: #111">…inline the SVG here…</span>
```

The rendered PNGs are black on transparency, which means they disappear on a dark background —
inline the SVG there instead, or ask for a light PNG set.

## Sizes

`icons/blitsen-rampant-{16,24,32,48,64,128,256,512,1024}.png`, rendered from the SVG. The 16, 24
and 32 px files come from the small-size cut; 48 px and up come from the full mark.

`icons/blitsen.ico` carries seven entries (16/24/32 from the cut, 48/64/128/256 from the full
mark) so Windows and browser tabs pick the right drawing at each size rather than downsampling one.

## Provenance and regenerating

The marks were drawn by an image model (`gemini-3-pro-image`) at 2048×2048 and traced from
there — `10-rampant-refined.png` for the primary and `20-rampant-icon.png` for the small-size
cut.

**Those raw renders are not in the repository.** They are 14 MB of PNG, and this project keeps
binaries out of its history on purpose. They are gitignored rather than deleted, so they survive
in a working copy that produced them; anywhere else, **the SVGs here are the source of truth**.
Re-cutting an icon size or a colour variant needs only the SVG. Only a change to the drawing
itself — a different tine count, a different pose — needs a new render.

Tracing used potrace (the pure-Python `potracer`), then trim to content and pad to a square
viewBox with a 6% margin. Each trace was verified by rendering the SVG back to a bitmap and
comparing against its source raster — every mark scored an IoU of 0.994 or better, so the vectors
match what Gemini drew rather than approximating it.
