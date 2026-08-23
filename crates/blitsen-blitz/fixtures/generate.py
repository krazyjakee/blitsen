"""Regenerates the subresource fixtures. Needs `fonttools` and `brotli`.

Pass ``--world-only`` after the output directory to write only
``block-world.ttf``. That keeps adding text coverage from rewriting the older
fixtures with a different fonttools version.

The fonts are built rather than borrowed so they carry no licence. The four
`block-*` corpus faces use solid rectangular glyphs with controlled metrics;
the small `block-world` face below adds only the outlines and shaping features
needed by the text tests.

A block glyph makes a rendered run unmistakable in a pixel assertion — no
system fallback paints a filled rectangle — so a test can prove the *web*
font was used rather than merely that some font was.

The first three faces are distinguished only by block height. Their internal
metadata is deliberately identical and deliberately wrong: same family, same
"Regular" style, same weight 400, none of which is the family or the weight
the CSS declares. Only the `@font-face` descriptors can tell them apart, so a
test that selects one by `font-weight` or `font-style` is testing the
descriptor rather than the font's own `name` table.

The fourth, `block-ascii`, covers printable ASCII rather than capitals alone,
and adds a blank space that still advances so runs can wrap. It exists for the
conformance corpus, whose cases are built from real framework markup and are
therefore not written in capitals: pinning that font makes every text metric
in the case come out of a committed file rather than off the host.

`block-world` is deliberately not a useful fallback font. It has one CJK
character, one emoji outline and Arabic beh plus its joining forms, solely so
tests can prove fallback, emoji clustering and complex shaping without making
the runtime or repository carry a general-purpose font.
"""

import sys

from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen

UPM = 1000
CAPITALS = range(0x41, 0x5B)
PRINTABLE = range(0x21, 0x7F)
# Fraction of the em box each face fills and the codepoints it covers, keyed by
# the file stem it is written to.
FACES = {
    "regular": (UPM, CAPITALS),
    "bold": (UPM // 2, CAPITALS),
    "italic": (UPM // 4, CAPITALS),
    "ascii": (UPM, PRINTABLE),
}


def build(top, codepoints):
    pen = TTGlyphPen(None)
    pen.moveTo((0, 0))
    pen.lineTo((UPM, 0))
    pen.lineTo((UPM, top))
    pen.lineTo((0, top))
    pen.closePath()

    spaced = codepoints is PRINTABLE
    glyphs = [".notdef", "block"] + (["space"] if spaced else [])
    cmap = {codepoint: "block" for codepoint in codepoints}
    if spaced:
        cmap[0x20] = "space"

    fb = FontBuilder(UPM, isTTF=True)
    fb.setupGlyphOrder(glyphs)
    fb.setupCharacterMap(cmap)
    fb.setupGlyf({name: (pen if name == "block" else TTGlyphPen(None)).glyph() for name in glyphs})
    fb.setupHorizontalMetrics({name: (UPM, 0) for name in glyphs})
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


def rectangle(left, bottom, right, top):
    pen = TTGlyphPen(None)
    pen.moveTo((left, bottom))
    pen.lineTo((right, bottom))
    pen.lineTo((right, top))
    pen.lineTo((left, top))
    pen.closePath()
    return pen.glyph()


def build_world():
    """A tiny owned font for deterministic fallback and shaping assertions."""
    glyphs = [
        ".notdef",
        "beh",
        "beh.init",
        "beh.medi",
        "beh.fina",
        "beh.isol",
        "cjk",
        "emoji",
    ]
    outlines = {
        # A visible missing-glyph box. It is intentionally a glyph rather than
        # a promise that every platform's own last-resort face looks this way.
        ".notdef": rectangle(100, 100, 900, 900),
        "beh": rectangle(100, 0, 900, 300),
        "beh.init": rectangle(100, 0, 900, 450),
        "beh.medi": rectangle(100, 0, 900, 600),
        "beh.fina": rectangle(100, 0, 900, 750),
        "beh.isol": rectangle(100, 0, 900, 900),
        "cjk": rectangle(0, 0, 1000, 1000),
        # An outline glyph, not a colour-font table. The test must not imply
        # COLR/CBDT/SVG or ZWJ-sequence support that this fixture cannot prove.
        "emoji": rectangle(150, 150, 850, 850),
    }
    fb = FontBuilder(UPM, isTTF=True)
    fb.setupGlyphOrder(glyphs)
    fb.setupCharacterMap({0x0628: "beh", 0x4E2D: "cjk", 0x1F642: "emoji"})
    fb.setupGlyf(outlines)
    fb.setupHorizontalMetrics({name: (UPM, 0) for name in glyphs})
    fb.setupHorizontalHeader(ascent=UPM, descent=0)
    fb.setupNameTable(
        {
            "familyName": "Blitsen World Fixture",
            "styleName": "Regular",
            "uniqueFontIdentifier": "Blitsen World Fixture 1",
            "fullName": "Blitsen World Fixture",
            "psName": "BlitsenWorldFixture-Regular",
            "version": "1.0",
        }
    )
    fb.setupOS2(
        usWeightClass=400,
        fsSelection=0x40,
        sTypoAscender=UPM,
        sTypoDescender=0,
        usWinAscent=UPM,
        usWinDescent=0,
    )
    fb.setupPost(italicAngle=0)
    # HarfRust assigns these features from Unicode joining state. Distinct
    # output glyphs let the test prove shaping happened rather than merely
    # proving that three Arabic codepoints had cmap entries.
    fb.addOpenTypeFeatures(
        """
        languagesystem DFLT dflt;
        languagesystem arab dflt;
        feature init { sub beh by beh.init; } init;
        feature medi { sub beh by beh.medi; } medi;
        feature fina { sub beh by beh.fina; } fina;
        feature isol { sub beh by beh.isol; } isol;
        """
    )
    # Keep the committed fixture reproducible. FontTools otherwise stamps the
    # `head` table at save time, changing the binary on every regeneration.
    fb.font.recalcTimestamp = False
    fb.font["head"].created = 2082844800  # 1970-01-01 in the OpenType epoch.
    fb.font["head"].modified = 2082844800
    return fb


out = sys.argv[1]
world_only = sys.argv[2:] == ["--world-only"]
if sys.argv[2:] not in ([], ["--world-only"]):
    raise SystemExit("usage: generate.py <output-directory> [--world-only]")

if not world_only:
    for stem, (top, codepoints) in FACES.items():
        fb = build(top, codepoints)
        fb.save(f"{out}/block-{stem}.ttf")
        if stem == "regular":
            fb.font.flavor = "woff2"
            fb.save(f"{out}/block-{stem}.woff2")

build_world().save(f"{out}/block-world.ttf")

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


if not world_only:
    with open(f"{out}/swatch.png", "wb") as png:
        png.write(
            b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", WIDTH, HEIGHT, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(rows, 9))
            + chunk(b"IEND", b"")
        )
