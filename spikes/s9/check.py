#!/usr/bin/env python3
"""Decide whether a captured Android frame is *this document* or merely a colour.

A screencap that does not crash proves nothing, and a screencap that is uniformly
one colour proves less than nothing, because it looks like a pass. So every claim
this spike makes about the frame is a count of pixels of an exact RGB that only
one part of the document can produce:

  page      the stylesheet's body background, and the negative control's subject
  css-*     three fills written in style.css and touched by nothing else
  js        a fill written only in app.js -- absent from index.html and style.css,
            so finding it means a JavaScript engine ran and the DOM bridge carried
            the mutation into layout and paint
  strips    two pale bands behind the type specimens, which give the glyph ink a
            known background to be counted against; a font stack that resolved to
            nothing leaves the band empty rather than ambiguous

The two specimen bands are also measured. The same string is set in `sans-serif`
and in `monospace`, so if one face answered every glyph -- or the platform has no
fonts and something drew boxes -- the two ink extents come out equal. Read the
result narrowly: unequal extents say two faces were resolved and their metrics
were used, not that both families resolved to what their names suggest. On the
API 33 image they do not; see README.md.

Usage: check.py FRAME.png [--page RRGGBB] [--js RRGGBB] [--json OUT.json]
"""

import argparse
import json
import sys

import numpy as np
from PIL import Image

# Written once here and once in the stylesheet, on purpose: a checker that read
# its expectations out of the CSS would pass against a frame of anything.
STRIP = (245, 247, 255)
SWATCHES = {
    "css-red": (229, 72, 77),
    "css-green": (48, 164, 108),
    "css-blue": (0, 145, 255),
}
# Every swatch is 78x78 CSS px; at the emulator's 2.75x this is ~46k device px.
# The floor is deliberately far below that -- it is asking "is this fill on the
# screen at all", not "is it exactly the size I predicted".
SWATCH_FLOOR = 5000
TEXT_FLOOR = 2000


def count(pixels, rgb, tolerance=2):
    return int(
        np.all(np.abs(pixels.astype(np.int16) - np.array(rgb, np.int16)) <= tolerance, axis=-1).sum()
    )


def bands(pixels, tolerance=6):
    """The rows of each pale specimen strip, as (top, bottom) pairs."""
    light = np.all(
        np.abs(pixels.astype(np.int16) - np.array(STRIP, np.int16)) <= tolerance, axis=-1
    )
    # More than half the row, so that the heading -- which is the same colour as
    # the strips, being body text on the dark ground -- is not mistaken for one.
    rows = light.sum(axis=1) > pixels.shape[1] // 2
    found, start = [], None
    for y, lit in enumerate(rows):
        if lit and start is None:
            start = y
        elif not lit and start is not None:
            if y - start > 8:
                found.append((start, y))
            start = None
    if start is not None:
        found.append((start, len(rows)))
    return found


def ink(pixels, top, bottom, tolerance=6):
    """Dark pixels inside one strip, and how far across it they run.

    Measured strictly inside the strip's own columns. The page around it is dark
    too, so a run taken across the whole row would come back the width of the
    screen for every specimen and compare nothing.
    """
    band = pixels[top:bottom].astype(np.int16)
    light = np.all(np.abs(band - np.array(STRIP, np.int16)) <= tolerance, axis=-1)
    inside = np.where(light.sum(axis=0) > (bottom - top) // 2)[0]
    if inside.size == 0:
        return {"pixels": 0, "left": None, "right": None, "width": 0}
    # Well in from each rounded corner, whose antialias against the dark page is
    # dark and is not glyph, and counting only columns with a stem's worth of ink
    # in them for the same reason.
    first, last = int(inside[0]) + 20, int(inside[-1]) - 20
    luma = band[:, first : last + 1] @ np.array([0.2126, 0.7152, 0.0722])
    dark = luma < 110
    columns = np.where(dark.sum(axis=0) >= 2)[0]
    if columns.size == 0:
        return {"pixels": 0, "left": None, "right": None, "width": 0}
    return {
        "pixels": int(dark.sum()),
        "left": first + int(columns[0]),
        "right": first + int(columns[-1]),
        "width": int(columns[-1] - columns[0] + 1),
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("frame")
    parser.add_argument("--page", default="0b1020")
    parser.add_argument("--js", default="ff00aa")
    parser.add_argument("--later", help="a second capture, taken seconds after the first")
    parser.add_argument("--specimens", help="write the two type specimens, cropped, here")
    parser.add_argument("--json")
    options = parser.parse_args()

    page = tuple(int(options.page[i : i + 2], 16) for i in (0, 2, 4))
    js = tuple(int(options.js[i : i + 2], 16) for i in (0, 2, 4))

    image = Image.open(options.frame).convert("RGB")
    pixels = np.asarray(image)
    height, width, _ = pixels.shape
    flat = pixels.reshape(-1, 3)
    colours, counts = np.unique(flat, axis=0, return_counts=True)

    checks = []
    tallies = {"page": count(pixels, page), "js": count(pixels, js)}
    for name, rgb in SWATCHES.items():
        tallies[name] = count(pixels, rgb)
    tallies["strip"] = count(pixels, STRIP)
    tallies["text"] = count(pixels, STRIP) # recomputed below against the dark ground

    # Body text is the strip colour on the dark ground, so it cannot be told from
    # the strips by colour alone. Count it only outside the strips.
    strips = bands(pixels)
    mask = np.ones(height, bool)
    for top, bottom in strips:
        mask[top:bottom] = False
    tallies["text"] = count(pixels[mask], STRIP)
    tallies["strip"] = count(pixels[~mask], STRIP)

    checks.append(("frame is not uniform", len(colours) >= 64, f"{len(colours)} distinct colours"))
    checks.append(
        (
            "page background painted",
            tallies["page"] > width * height // 8,
            f"{tallies['page']} px of #{options.page}",
        )
    )
    for name in SWATCHES:
        checks.append(
            (f"{name} fill painted", tallies[name] > SWATCH_FLOOR, f"{tallies[name]} px")
        )
    checks.append(
        (
            "javascript fill painted",
            tallies["js"] > SWATCH_FLOOR,
            f"{tallies['js']} px of #{options.js}, a colour no markup or stylesheet carries",
        )
    )
    checks.append(
        ("body text painted", tallies["text"] > TEXT_FLOOR, f"{tallies['text']} px of glyph")
    )
    checks.append(("two type specimens found", len(strips) == 2, f"{len(strips)} pale bands"))

    inks = [ink(pixels, top, bottom) for top, bottom in strips]
    for index, measured in enumerate(inks):
        checks.append(
            (
                f"specimen {index} has glyphs",
                measured["pixels"] > 400,
                f"{measured['pixels']} dark px spanning {measured['width']} px",
            )
        )
    if len(inks) == 2:
        checks.append(
            (
                "the two families measure differently",
                inks[0]["width"] != inks[1]["width"],
                f"sans {inks[0]['width']} px vs monospace {inks[1]['width']} px",
            )
        )

    if options.specimens and len(strips) == 2 and all(measured["left"] for measured in inks):
        # Cropped to the ink, both on the same left origin and the same scale, so
        # that "two faces" and "one face plus different digits" can be told apart
        # by eye. They cannot be told apart by the width check alone.
        crops = [
            pixels[top:bottom, measured["left"] : measured["right"] + 1]
            for (top, bottom), measured in zip(strips, inks)
        ]
        widest = max(crop.shape[1] for crop in crops)
        tallest = max(crop.shape[0] for crop in crops)
        sheet = np.full((tallest * 2 + 8, widest, 3), STRIP, np.uint8)
        sheet[tallest : tallest + 8] = 0
        for index, crop in enumerate(crops):
            offset = index * (tallest + 8)
            sheet[offset : offset + crop.shape[0], : crop.shape[1]] = crop
        Image.fromarray(sheet).save(options.specimens)

    # Not a check, because it is a property of the device's font configuration
    # rather than of Blitsen, and either answer is a legitimate one. It is
    # reported because it is what keeps the width check above from being read as
    # more than it says.
    shared = None
    if len(inks) == 2 and all(measured["left"] for measured in inks):
        left = max(measured["left"] for measured in inks)
        right = left + min(measured["width"] for measured in inks) * 2 // 3
        first = pixels[strips[0][0] : strips[0][1], left:right].astype(np.int16)
        second = pixels[strips[1][0] : strips[1][1], left:right].astype(np.int16)
        for offset in range(-6, 7):
            top = first[max(0, offset) :]
            bottom = second[max(0, -offset) :]
            rows = min(top.shape[0], bottom.shape[0])
            identical = np.all(top[:rows] == bottom[:rows], axis=-1).mean()
            shared = identical if shared is None else max(shared, identical)

    moved = None
    if options.later:
        later = np.asarray(Image.open(options.later).convert("RGB"))
        moved = int(np.any(later != pixels, axis=-1).sum()) if later.shape == pixels.shape else -1
        checks.append(
            (
                "still painting five seconds later",
                moved > 0,
                f"{moved} px differ from {options.later}",
            )
        )

    order = np.argsort(-counts)[:8]
    print(f"{options.frame}: {width}x{height}, {len(colours)} distinct colours")
    print("  most common colours")
    for index in order:
        red, green, blue = colours[index]
        share = 100.0 * counts[index] / (width * height)
        print(f"    #{red:02x}{green:02x}{blue:02x}  {counts[index]:>9} px  {share:5.2f}%")
    if shared is not None:
        print(
            f"  the two specimens' leading glyphs are {100 * shared:.1f}% pixel-identical"
            " once aligned"
        )
    print("  checks")
    failed = 0
    for label, ok, detail in checks:
        print(f"    {'pass' if ok else 'FAIL'}  {label:<38} {detail}")
        failed += not ok

    if options.json:
        with open(options.json, "w") as handle:
            json.dump(
                {
                    "frame": options.frame,
                    "width": width,
                    "height": height,
                    "distinctColours": len(colours),
                    "page": options.page,
                    "js": options.js,
                    "pixels": tallies,
                    "specimens": inks,
                    "sharedGlyphs": shared,
                    "movedPixels": moved,
                    "checks": [
                        {"check": label, "pass": bool(ok), "detail": detail}
                        for label, ok, detail in checks
                    ],
                },
                handle,
                indent=2,
            )
            handle.write("\n")

    print(f"  {len(checks) - failed}/{len(checks)} passed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
