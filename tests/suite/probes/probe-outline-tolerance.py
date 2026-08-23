#!/usr/bin/env python3
"""Question: how far can a saved static instance's coordinates be from what a
variation-aware renderer draws for the source at the same location?

Answer used in tests/suite/cases/outlines-containers.json to justify
`outlines_match_source_at` tolerance 0.5 for simple contours.

Run:  tests/suite/probes/probe-outline-tolerance.py
Signal: "coord" is the max |delta| over every point of every glyph between
  TTFont.getGlyphSet(location=L)   (float interpolation, the renderer's view)
and
  instantiateVariableFont(..., L) saved and reloaded   (int16 glyf, the output)
Pass means coord < 0.5 (one int16 rounding) at every sampled location.
"""

import os
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent.parent.parent
VENV_PYTHON = REPO / ".suite-venv" / "bin" / "python"


def reexec_into_venv() -> None:
    """The suite's venv holds the pinned fontTools; a bare python probably does not."""
    try:
        import fontTools  # noqa: F401
    except ImportError:
        if not VENV_PYTHON.exists():
            raise SystemExit(
                "fontTools is not importable and .suite-venv does not exist yet. "
                "Run tests/suite/run.py once; it bootstraps the venv."
            )
        os.execv(str(VENV_PYTHON), [str(VENV_PYTHON), str(Path(__file__).resolve()), *sys.argv[1:]])


reexec_into_venv()

import io
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont
# Decomposing, so a composite glyph is measured by the shape its components
# make rather than reading as empty. Recursive has none, but a probe that
# silently skips composites is a trap for the next font it is pointed at.
from fontTools.pens.recordingPen import DecomposingRecordingPen

SRC = sys.argv[1] if len(sys.argv) > 1 else str(
    REPO / "testdata" / "fonts" / "Recursive-VF.subset.ttf"
)
LOCS = [
    {"MONO": 0, "CASL": 0, "wght": 300, "slnt": 0, "CRSV": 0.5},     # default
    {"MONO": 0, "CASL": 0, "wght": 653.7, "slnt": 0, "CRSV": 0.5},   # fractional
    {"MONO": 0.5, "CASL": 0.5, "wght": 500, "slnt": -7, "CRSV": 0.5},
    {"MONO": 1, "CASL": 0, "wght": 300, "slnt": -15, "CRSV": 0},     # corner
    {"MONO": 0, "CASL": 1, "wght": 1000, "slnt": 0, "CRSV": 1},      # corner
]

def pts(gs, n):
    p = DecomposingRecordingPen(gs); gs[n].draw(p)
    return [c for _, args in p.value for a in args if isinstance(a, tuple) for c in a]

for loc in LOCS:
    src = TTFont(SRC); ref = src.getGlyphSet(location=loc)
    inst = instantiateVariableFont(TTFont(SRC), loc, inplace=False, optimize=True)
    b = io.BytesIO(); inst.save(b); b.seek(0)
    out = TTFont(b).getGlyphSet()
    coord = adv = 0.0
    for g in src.getGlyphOrder():
        for x, y in zip(pts(ref, g), pts(out, g)):
            coord = max(coord, abs(x - y))
        adv = max(adv, abs(ref[g].width - out[g].width))
    print(f"{loc}  coord {coord:.4f}  adv {adv:.4f}")
