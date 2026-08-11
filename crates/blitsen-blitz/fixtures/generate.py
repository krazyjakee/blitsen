"""Regenerates the subresource fixtures. Needs `fonttools` and `brotli`.

The fonts are built rather than borrowed so they carry no licence and no
surprises: every glyph is a solid em block.

A block glyph makes a rendered run unmistakable in a pixel assertion — no
system fallback paints a filled rectangle — so a test can prove the *web*
font was used rather than merely that some font was.

The three faces are distinguished only by block height. Their internal
metadata is deliberately identical and deliberately wrong: same family, same
"Regular" style, same weight 400, none of which is the family or the weight
the CSS declares. Only the `@font-face` descriptors can tell them apart, so a
test that selects one by `font-weight` or `font-style` is testing the
descriptor rather than the font's own `name` table.
"""

import sys

from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen

UPM = 1000
# Fraction of the em box each face fills, and the file stem it is written to.
FACES = {"regular": UPM, "bold": UPM // 2, "italic": UPM // 4}


def build(top):
    pen = TTGlyphPen(None)
    pen.moveTo((0, 0))
    pen.lineTo((UPM, 0))
    pen.lineTo((UPM, top))
    pen.lineTo((0, top))
    pen.closePath()

    fb = FontBuilder(UPM, isTTF=True)
    fb.setupGlyphOrder([".notdef", "block"])
    fb.setupCharacterMap({codepoint: "block" for codepoint in range(0x41, 0x5B)})
    fb.setupGlyf({".notdef": TTGlyphPen(None).glyph(), "block": pen.glyph()})
    fb.setupHorizontalMetrics({".notdef": (UPM, 0), "block": (UPM, 0)})
    fb.setupHorizontalHeader(ascent=UPM, descent=0)
    fb.setupNameTable(
        {
            "familyName": "Blitsen Block Source",
            "styleName": "Regular",
            "uniqueFontIdentifier": f"Blitsen Block Source {top}",
            "fullName": "Blitsen Block Source",
            "psName": f"BlitsenBlockSource-{top}",
            "version": "1.0",
        }
    )
    fb.setupOS2(
        usWeightClass=400,
        fsSelection=0x40,  # REGULAR
        sTypoAscender=UPM,
        sTypoDescender=0,
        usWinAscent=UPM,
        usWinDescent=0,
    )
    fb.setupPost(italicAngle=0)
    return fb


out = sys.argv[1]
for stem, top in FACES.items():
    fb = build(top)
    fb.save(f"{out}/block-{stem}.ttf")
    if stem == "regular":
        fb.font.flavor = "woff2"
        fb.save(f"{out}/block-{stem}.woff2")

# An 8x4 PNG, opaque red on the left half and opaque blue on the right. Small
# enough to inline as a `data:` URL in a test, asymmetric enough that a pixel
# assertion catches a flipped or mis-scaled decode.
import struct
import zlib

WIDTH, HEIGHT = 8, 4
RED, BLUE = (220, 20, 20, 255), (20, 40, 220, 255)
rows = b"".join(
    b"\x00" + b"".join(bytes(RED if x < WIDTH // 2 else BLUE) for x in range(WIDTH))
    for _ in range(HEIGHT)
)


def chunk(tag, data):
    return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data))


with open(f"{out}/swatch.png", "wb") as png:
    png.write(
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", WIDTH, HEIGHT, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(rows, 9))
        + chunk(b"IEND", b"")
    )
