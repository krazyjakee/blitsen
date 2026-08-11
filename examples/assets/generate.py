"""Builds the binary assets this example loads. Needs `fonttools` and `brotli`.

Everything here is generated rather than borrowed so the example carries no
third-party font or image licence. `Pixelboard` is a 5x7 bitmap alphabet
promoted to outlines — legible, obviously a display face, and about 3 KB as
WOFF2, which is what a real `@font-face` payload looks like.
"""

import struct
import sys
import zlib

from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen

UPM = 1000
COLUMNS, ROWS = 5, 7
CELL = UPM // 8  # 125 units, so a capital is 875 tall and one em wide with space.
ADVANCE = CELL * (COLUMNS + 1)

# Row-major, top row first. A space is drawn by the empty glyph below.
ALPHABET = {
    "A": ".###. #...# #...# ##### #...# #...# #...#",
    "B": "####. #...# ####. #...# #...# #...# ####.",
    "C": ".#### #.... #.... #.... #.... #.... .####",
    "D": "####. #...# #...# #...# #...# #...# ####.",
    "E": "##### #.... ####. #.... #.... #.... #####",
    "F": "##### #.... ####. #.... #.... #.... #....",
    "G": ".#### #.... #.... #.### #...# #...# .####",
    "H": "#...# #...# #...# ##### #...# #...# #...#",
    "I": "##### ..#.. ..#.. ..#.. ..#.. ..#.. #####",
    "J": "....# ....# ....# ....# #...# #...# .###.",
    "K": "#...# #..#. #.#.. ##... #.#.. #..#. #...#",
    "L": "#.... #.... #.... #.... #.... #.... #####",
    "M": "#...# ##.## #.#.# #.#.# #...# #...# #...#",
    "N": "#...# ##..# #.#.# #..## #...# #...# #...#",
    "O": ".###. #...# #...# #...# #...# #...# .###.",
    "P": "####. #...# #...# ####. #.... #.... #....",
    "Q": ".###. #...# #...# #...# #.#.# #..#. .##.#",
    "R": "####. #...# #...# ####. #.#.. #..#. #...#",
    "S": ".#### #.... #.... .###. ....# ....# ####.",
    "T": "##### ..#.. ..#.. ..#.. ..#.. ..#.. ..#..",
    "U": "#...# #...# #...# #...# #...# #...# .###.",
    "V": "#...# #...# #...# #...# #...# .#.#. ..#..",
    "W": "#...# #...# #...# #.#.# #.#.# ##.## #...#",
    "X": "#...# #...# .#.#. ..#.. .#.#. #...# #...#",
    "Y": "#...# #...# .#.#. ..#.. ..#.. ..#.. ..#..",
    "Z": "##### ....# ...#. ..#.. .#... #.... #####",
    "0": ".###. #...# #..## #.#.# ##..# #...# .###.",
    "1": "..#.. .##.. ..#.. ..#.. ..#.. ..#.. .###.",
    "2": ".###. #...# ....# ..##. .#... #.... #####",
    "3": "####. ....# ....# .###. ....# ....# ####.",
    "4": "...#. ..##. .#.#. #..#. ##### ...#. ...#.",
    "5": "##### #.... ####. ....# ....# #...# .###.",
    "6": ".###. #.... #.... ####. #...# #...# .###.",
    "7": "##### ....# ...#. ..#.. .#... .#... .#...",
    "8": ".###. #...# #...# .###. #...# #...# .###.",
    "9": ".###. #...# #...# .#### ....# ....# .###.",
}
PUNCTUATION = {".": "..... ..... ..... ..... ..... .##.. .##..", " ": " " * 7}


def outline(bitmap):
    """One contour per horizontal run of lit cells."""
    pen = TTGlyphPen(None)
    for row, cells in enumerate(bitmap.split()):
        start = None
        for column in range(COLUMNS + 1):
            lit = column < COLUMNS and cells[column] == "#"
            if lit and start is None:
                start = column
            elif not lit and start is not None:
                left, right = start * CELL, column * CELL
                bottom, top = (ROWS - 1 - row) * CELL, (ROWS - row) * CELL
                pen.moveTo((left, bottom))
                pen.lineTo((left, top))
                pen.lineTo((right, top))
                pen.lineTo((right, bottom))
                pen.closePath()
                start = None
    return pen.glyph()


def name_of(character):
    return f"u{ord(character):04X}"


glyphs = {**ALPHABET, **PUNCTUATION}
order = [".notdef", *(name_of(character) for character in glyphs)]

fb = FontBuilder(UPM, isTTF=True)
fb.setupGlyphOrder(order)
fb.setupCharacterMap({ord(character): name_of(character) for character in glyphs})
fb.setupGlyf(
    {
        ".notdef": TTGlyphPen(None).glyph(),
        **{name_of(character): outline(bitmap) for character, bitmap in glyphs.items()},
    }
)
fb.setupHorizontalMetrics({name: (ADVANCE, 0) for name in order})
fb.setupHorizontalHeader(ascent=ROWS * CELL, descent=-CELL)
fb.setupNameTable(
    {
        "familyName": "Pixelboard",
        "styleName": "Regular",
        "uniqueFontIdentifier": "Blitsen Pixelboard Regular",
        "fullName": "Pixelboard Regular",
        "psName": "Pixelboard-Regular",
        "version": "1.0",
        "copyright": "Generated for the Blitsen assets example. Public domain.",
    }
)
fb.setupOS2(sTypoAscender=ROWS * CELL, sTypoDescender=-CELL, usWinAscent=ROWS * CELL, usWinDescent=CELL)
fb.setupPost()

out = sys.argv[1] if len(sys.argv) > 1 else "."
fb.font.flavor = "woff2"
fb.save(f"{out}/pixelboard.woff2")


def png(path, width, height, shade):
    rows = b"".join(
        b"\x00" + b"".join(bytes(shade(x, y)) for x in range(width)) for y in range(height)
    )

    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data))

    with open(path, "wb") as file:
        file.write(
            b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(rows, 9))
            + chunk(b"IEND", b"")
        )


# 320x180: a wide photo-shaped gradient, so the page can size it from its
# intrinsic ratio alone.
png(
    f"{out}/gradient.png",
    320,
    180,
    lambda x, y: (30 + x // 2, 40 + y // 2, 200 - x // 3, 255),
)
# 64x64 tile, deliberately square, repeated as a CSS background.
png(
    f"{out}/tile.png",
    64,
    64,
    lambda x, y: (24, 33, 47, 255) if (x // 8 + y // 8) % 2 else (31, 43, 61, 255),
)
