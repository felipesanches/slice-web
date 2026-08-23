#!/usr/bin/env python3
"""Question: would `partial.advances-still-vary-across-a-restricted-range` actually catch
the bug it exists to catch?

A new test that passes proves nothing on its own. This builds the defect the case is meant
to find -- a partial instance whose advance widths have been frozen at the default master
-- and checks that the case's own assertion fails on it and passes on correct output.

The defect is constructed the way it would really happen. A `glyf` font carries its advance
variation in two places: `HVAR`, and the four phantom points `gvar` appends to every glyph.
An implementation that drops one without rescaling the other leaves a font whose widths are
right at its default location and constant everywhere else. So: take a correct partial
instance, keep every outline delta, and null out only the phantom points.

Expected, on `two-axis` restricted to wdth 100:200 (H is 372 units wide at wdth=50, 600 at
100, 960 at 200):

    correct output     -> PASS  3 locations agree
    advances frozen    -> FAIL  at {'wdth': 150}: glyph H: advance 780 vs 600

Note which location catches it. The frozen font passes at wdth=100, because that is the
axis default and the frozen widths are the default's widths. Only the interior and far-end
samples separate "the widths still interpolate" from "the widths are stuck". That is the
argument for `advances_match_source_across` existing at all, rather than the corpus making
do with the single-location `advances_match_source_at` it had.

Run:  tests/suite/probes/probe-advance-freeze.py
"""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile
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

sys.path.insert(0, str(REPO / "tests" / "suite"))

from fontTools import ttLib  # noqa: E402

from checker import Checker  # noqa: E402

SOURCE = REPO / "tests" / "suite" / "fixtures" / "out" / "two-axis.ttf"
BINARY = os.environ.get("SLICE_BINARY", str(REPO / "target" / "debug" / "slice"))
LOCATIONS = [
    {"wdth": 100, "wght": 400},  # the axis default: a frozen font still passes here
    {"wdth": 150, "wght": 400},
    {"wdth": 200, "wght": 400},
]


def freeze_advances(source: str, target: str) -> None:
    """Keep every outline delta; null out the phantom points that carry the advances."""
    font = ttLib.TTFont(source)
    glyf = font["glyf"]
    for name, variations in font["gvar"].variations.items():
        glyph = glyf[name]
        points = len(glyph.coordinates) if glyph.numberOfContours > 0 else 0
        for variation in variations:
            variation.coordinates = variation.coordinates[:points] + [None] * 4
    font.save(target)


def main() -> int:
    if not Path(BINARY).exists():
        raise SystemExit(f"{BINARY} is not built; run cargo build -p slice-cli")

    work = Path(tempfile.mkdtemp(prefix="slice-advance-freeze-"))
    good, frozen = work / "good.ttf", work / "frozen.ttf"

    subprocess.run(
        [BINARY, "cut", str(SOURCE), str(good), "--axis", "wdth=100:200", "--axis", "wght=400"],
        check=True, capture_output=True,
    )
    freeze_advances(str(good), str(frozen))

    print(f"source {SOURCE.name}, restricted to wdth 100:200\n")
    spec = {"locations": LOCATIONS, "tolerance": 1.0}
    failed_as_expected = False
    for label, path in (("correct output", good), ("advances frozen", frozen)):
        ok, detail = Checker(str(path), str(SOURCE)).advances_match_source_across(spec)
        print(f"  {label:18} -> {'PASS' if ok else 'FAIL'}  {detail}")
        if label == "advances frozen" and not ok:
            failed_as_expected = True

    # Sampling only the default is what the corpus did before this case existed.
    ok, detail = Checker(str(frozen), str(SOURCE)).advances_match_source_at(
        {"location": LOCATIONS[0], "tolerance": 1.0}
    )
    print(f"\n  the same frozen font, sampled only at the default location:"
          f" {'PASS' if ok else 'FAIL'}  {detail}")
    print("  ^^ which is why the case samples more than one.")

    if not failed_as_expected:
        print("\nThe case does NOT discriminate: it passed a font it should have caught.")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
