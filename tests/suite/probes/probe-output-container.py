#!/usr/bin/env python3
"""Question: does the original Slice choose its output container from the file
extension the user types, as docs/original-behaviour.md H1 claims?

Answer: NO. TTFont.save() never looks at the filename; it passes self.flavor to
SFNTWriter (ttFont.py:411) and self.flavor was copied from the reader on open
(ttFont.py:318). The output container therefore always equals the INPUT
container. Evidence for the container.* cases in
tests/suite/cases/outlines-containers.json that the original is expected to fail.

Run:  tests/suite/probes/probe-output-container.py
Signal: "result flavor" is None for a bare sfnt, 'woff'/'woff2' otherwise.
Expected (fontTools 4.62.1, reproducing the original's save path):
  Recursive-VF.subset.ttf   -> o.woff   flavor None   <- an sfnt named .woff
  Recursive-VF.subset.woff  -> o.ttf    flavor woff   <- a WOFF named .ttf
  Recursive-VF.subset.woff2 -> o.ttf    flavor woff2
  Recursive-VF.subset.ttf   -> o.woff2  flavor None
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

import tempfile
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont

D = (Path(sys.argv[1]) if len(sys.argv) > 1 else REPO / "testdata" / "fonts")
D = str(D).rstrip("/") + "/"
LOC = {"MONO": 0, "CASL": 0, "wght": 650, "slnt": 0, "CRSV": 0.5}
tmp = tempfile.mkdtemp()
for src, ext in [("Recursive-VF.subset.ttf", ".woff"),
                 ("Recursive-VF.subset.woff", ".ttf"),
                 ("Recursive-VF.subset.woff2", ".ttf"),
                 ("Recursive-VF.subset.ttf", ".woff2")]:
    f = TTFont(D + src)
    instantiateVariableFont(f, LOC, inplace=True, optimize=True)
    out = os.path.join(tmp, "o" + ext)
    f.save(out)
    print(f"{src:28s} -> {ext:7s} result flavor: {TTFont(out).flavor}  "
          f"size {os.path.getsize(out)}")
