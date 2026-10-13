"""Slim the bundled UI fonts down to what Shoji draws.

Usage: python -I scripts/build-fonts.py <dir with the source .ttf files>

Sources are the variable fonts from https://github.com/google/fonts (all SIL OFL 1.1):
  ofl/fraunces/Fraunces[SOFT,WONK,opsz,wght].ttf         -> Fraunces.ttf
  ofl/literata/Literata[opsz,wght].ttf                   -> Literata.ttf
  ofl/newsreader/Newsreader[opsz,wght].ttf               -> Newsreader.ttf
  ofl/instrumentsans/InstrumentSans[wdth,wght].ttf       -> InstrumentSans.ttf

Each font is pinned to the axis ranges the UI uses, subset to Latin, and written as
WOFF2 into assets/fonts/. Requires `pip install fonttools brotli`.
"""

import sys
from pathlib import Path

from fontTools import subset
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont

# Basic Latin, Latin-1, common punctuation, arrows and the euro, enough for every UI string.
UNICODES = (
    "U+0020-007E,U+00A0-00FF,U+0131,U+0152-0153,U+02C6,U+02DA,U+02DC,U+2013-2014,"
    "U+2018-201E,U+2022,U+2026,U+2030,U+2039-203A,U+20AC,U+2122,U+2190-2193,U+2212,U+2715"
)
FEATURES = ["kern", "liga", "calt", "tnum", "lnum", "onum", "case", "ss01", "ss02"]

# name -> axis limits (a number pins the axis, a tuple keeps a range)
FONTS = {
    # Softest, non-wonky Fraunces: the warm display face of the Kinari profile.
    "Fraunces": {"SOFT": 100, "WONK": 0, "opsz": (9, 144), "wght": (300, 600)},
    "Literata": {"opsz": (7, 72), "wght": (300, 700)},
    "Newsreader": {"opsz": (6, 72), "wght": (300, 700)},
    "InstrumentSans": {"wdth": 100, "wght": (400, 700)},
}


def main(src: Path, out: Path) -> None:
    out.mkdir(parents=True, exist_ok=True)
    for name, limits in FONTS.items():
        # Subset first: instancing leaves gvar lazily loaded, which the subsetter trips on.
        font = TTFont(src / f"{name}.ttf", lazy=False)
        options = subset.Options()
        options.flavor = "woff2"
        options.layout_features = FEATURES
        options.name_IDs = [0, 1, 2, 3, 4, 5, 6, 13, 14]
        options.notdef_outline = True
        subsetter = subset.Subsetter(options)
        subsetter.populate(unicodes=subset.parse_unicodes(UNICODES))
        subsetter.subset(font)
        font = instantiateVariableFont(font, limits, updateFontNames=False)
        target = out / f"{name}.woff2"
        font.flavor = "woff2"
        font.save(target)
        print(f"{target.name}: {target.stat().st_size // 1024} KB")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    root = Path(__file__).resolve().parent.parent
    main(Path(sys.argv[1]), root / "assets" / "fonts")
