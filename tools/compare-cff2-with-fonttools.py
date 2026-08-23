#!/usr/bin/env python3
"""Compare the CFF2 table this engine writes against the one fontTools writes.

Question this answers
---------------------
    When a CFF2 variable font is instanced, do we resolve the same blends into the same
    numbers, keep the same regions, and write a table with the same shape as fontTools?

`compare-with-fonttools.py` compares *drawn outlines*, which is the right check for
`glyf` and not enough for CFF2. A CFF2 instance can draw correctly and still be wrong in
ways that only show up later: a `blend` left in a font whose variation store has been
deleted, a `vsindex` pointing at a subtable that is no longer there, a Private DICT whose
alignment zones were dropped, or a region list that no longer matches the deltas in the
charstrings. So this reaches inside the table and compares the programs themselves.

It prints, per case:

* which tables each side produced;
* the disassembled charstring program for every glyph, side by side;
* the variation store's regions and each subtable's region indices;
* the Private DICT of every entry in the FDArray;
* `hmtx` advances and the `fvar` extents.

Usage
-----
    tools/compare-cff2-with-fonttools.py             # build, compare, print differences
    tools/compare-cff2-with-fonttools.py --verbose   # print the programs even when equal
    tools/compare-cff2-with-fonttools.py --no-build  # use the existing slice binary

Exits 0 when the two agree (allowing for the differences in ACCEPTED), 1 otherwise, and 0
with a message when the fontTools environment cannot be prepared.

What it measured, when this was written
---------------------------------------
On `tests/suite/fixtures/out/cff2-vf.otf` with fontTools 4.62.1, every charstring program
is *identical* for every case: pinned at 400, 500, 700 and 900, and restricted to
400:700 and 400:900. The resolved values agree exactly because both sides round the same
way -- fontTools adds `round(defaultDelta)` to the base value in `instantiateCFF2`, and
`instancer/cff2/charstring.rs` copies that, ties-to-even included.

Three differences are expected and allowlisted:

* **Table size.** fontTools runs `specializeCommands` over every program, which rewrites
  operators into their shortest forms (`rlineto` into `hlineto` and so on). This resolves
  blends in place and leaves the operators alone, so an instance here is a little larger
  and draws the same path. Re-specializing is an optimisation, not a correctness step,
  and it is the part of the pipeline most likely to introduce a difference that draws.
* **`STAT` and name records.** `slice cut` runs the whole Slice pipeline, which prunes
  name records that no longer name anything and filters `STAT`; `instantiateVariableFont`
  on its own does less. Both are compared by `compare-with-fonttools.py` on the `glyf`
  fixture already, so they are out of scope here.
* **An emptied `HVAR`.** This fixture's `HVAR` has no regions and no deltas -- both
  masters have the same advances -- so after re-tenting there is nothing left in it.
  fontTools keeps the shell, because it only deletes `HVAR` when every axis was pinned;
  this drops it, because a variation store with no regions describes no variation. An
  `HVAR` that *does* vary is rebuilt rather than dropped: see
  `instancer::varstore::rebuild`, which is what makes the difference visible here at all.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
VENV = REPO_ROOT / ".fonttools-venv"
FONTTOOLS_VERSION = "4.62.1"
FIXTURE = REPO_ROOT / "tests" / "suite" / "fixtures" / "out" / "cff2-vf.otf"

# Fields whose difference is explained in the module docstring rather than a defect.
ACCEPTED = {
    "cff2_length": "fontTools re-specializes charstring operators; this leaves them alone",
    "tables": "slice cut runs the whole pipeline, which also prunes names and STAT",
    "statValues": "slice cut filters STAT; instantiateVariableFont does not",
    "nameIDs": "slice cut prunes name records the output no longer refers to",
    "hasHVAR": "an HVAR whose store has no regions left describes no variation; we drop it",
}

# (label, axis limits). A number pins; a two-element list restricts.
CASES = [
    ("pin at the default (wght=400)", {"wght": 400}),
    ("pin between the masters (wght=500)", {"wght": 500}),
    ("pin at wght=700, the corpus case", {"wght": 700}),
    ("pin at the far master (wght=900)", {"wght": 900}),
    ("restrict to 400:700, the corpus case", {"wght": [400, 700]}),
    ("restrict to the whole axis 400:900", {"wght": [400, 900]}),
]

BUILD = r"""
import json, sys
from fontTools import ttLib
from fontTools.varLib import instancer

src, dst, limits = sys.argv[1], sys.argv[2], json.loads(sys.argv[3])
limits = {k: (tuple(v) if isinstance(v, list) else v) for k, v in limits.items()}
font = ttLib.TTFont(src)
instancer.instantiateVariableFont(font, limits, inplace=True, optimize=True)
font.save(dst)
"""

# Reaches into the CFF2 table rather than drawing from it; see the module docstring.
PROBE = r"""
import json, sys
from fontTools import ttLib

def describe(path):
    f = ttLib.TTFont(path)
    out = {"tables": sorted(f.keys())}
    if "CFF2" not in f:
        out["cff2"] = None
        return out

    out["cff2_length"] = len(f.reader["CFF2"]) if "CFF2" in f.reader else None

    top = f["CFF2"].cff.topDictIndex[0]
    programs = {}
    for name in f.getGlyphOrder():
        cs = top.CharStrings[name]
        cs.decompile()
        programs[name] = [
            (round(t, 6) if isinstance(t, float) else t) for t in cs.program
        ]
    out["programs"] = programs

    store = getattr(top, "VarStore", None)
    if store is None:
        out["regions"] = None
        out["varData"] = None
    else:
        vs = store.otVarStore
        out["regions"] = [
            [[a.StartCoord, a.PeakCoord, a.EndCoord] for a in r.VarRegionAxis]
            for r in vs.VarRegionList.Region
        ]
        out["varData"] = [list(d.VarRegionIndex) for d in vs.VarData]

    privates = []
    for fd in top.FDArray:
        p = fd.Private
        privates.append(
            {k: v for k, v in sorted(p.rawDict.items()) if k not in ("Subrs",)}
        )
    out["privateDicts"] = privates
    out["globalSubrs"] = len(f["CFF2"].cff.GlobalSubrs)
    out["localSubrs"] = [
        len(getattr(fd.Private, "Subrs", []) or []) for fd in top.FDArray
    ]

    out["advances"] = {n: f["hmtx"][n][0] for n in f.getGlyphOrder()}
    if "fvar" in f:
        out["axes"] = [
            [a.axisTag, a.minValue, a.defaultValue, a.maxValue] for a in f["fvar"].axes
        ]
    else:
        out["axes"] = None
    out["hasHVAR"] = "HVAR" in f
    return out

print(json.dumps(describe(sys.argv[1])))
"""


def ensure_venv() -> Path | None:
    python = VENV / "bin" / "python"
    if python.exists():
        return python
    print(f"setting up {VENV} with fontTools {FONTTOOLS_VERSION}")
    try:
        subprocess.run([sys.executable, "-m", "venv", str(VENV)], check=True)
        subprocess.run(
            [
                str(python),
                "-m",
                "pip",
                "install",
                "-q",
                f"fonttools=={FONTTOOLS_VERSION}",
                "brotli",
            ],
            check=True,
        )
    except subprocess.CalledProcessError as e:
        print(f"could not prepare the fontTools environment: {e}", file=sys.stderr)
        return None
    return python


def run_probe(python: Path, path: Path) -> dict:
    result = subprocess.run(
        [str(python), "-c", PROBE, str(path)], capture_output=True, text=True
    )
    if result.returncode != 0:
        raise SystemExit(f"the probe failed on {path}:\n{result.stderr}")
    return json.loads(result.stdout)


def compare_programs(a: dict, b: dict, verbose: bool) -> int:
    """Print each glyph's program side by side. Returns the number that differ."""
    differing = 0
    for name in sorted(set(a) | set(b)):
        left, right = a.get(name), b.get(name)
        if left == right:
            if verbose:
                print(f"  ok   {name:10} {left}")
            continue
        differing += 1
        print(f"  FAIL {name}")
        print(f"         fontTools: {left}")
        print(f"         ours:      {right}")
    return differing


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--verbose", action="store_true", help="print every field, not just differences"
    )
    parser.add_argument(
        "--no-build", action="store_true", help="use the existing slice binary"
    )
    args = parser.parse_args()

    python = ensure_venv()
    if python is None:
        print("skipping the CFF2 comparison")
        return 0
    if not FIXTURE.exists():
        print(f"{FIXTURE} is missing; run tests/suite/fixtures/build.py")
        return 0

    if not args.no_build:
        subprocess.run(
            ["cargo", "build", "-p", "slice-cli"],
            cwd=REPO_ROOT,
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    slice_cli = REPO_ROOT / "target" / "debug" / "slice"

    workdir = Path(tempfile.mkdtemp(prefix="slice-cff2-vs-fonttools-"))
    failures = 0

    for label, limits in CASES:
        print(f"=== {label} ===")
        reference = workdir / "reference.otf"
        ours = workdir / "ours.otf"

        subprocess.run(
            [str(python), "-c", BUILD, str(FIXTURE), str(reference), json.dumps(limits)],
            check=True,
        )

        axis_args = []
        for tag, value in limits.items():
            if isinstance(value, list):
                axis_args += ["--axis", f"{tag}={value[0]}:{value[1]}"]
            else:
                axis_args += ["--axis", f"{tag}={value}"]
        done = subprocess.run(
            [str(slice_cli), "cut", str(FIXTURE), str(ours), *axis_args],
            capture_output=True,
            text=True,
        )
        if done.returncode != 0:
            print(f"  FAIL slice cut refused the job: {done.stderr.strip()}")
            failures += 1
            print()
            continue

        a = run_probe(python, reference)
        b = run_probe(python, ours)

        for key in sorted(set(a) | set(b)):
            va, vb = a.get(key), b.get(key)
            if key == "programs":
                differing = compare_programs(va or {}, vb or {}, args.verbose)
                failures += differing
                if not differing:
                    print(f"  ok   programs               all {len(va or {})} identical")
                continue
            if va == vb:
                if args.verbose:
                    print(f"  ok   {key:22} {va}")
                continue
            if key in ACCEPTED:
                if args.verbose or key == "tables":
                    print(f"  --   {key} differs ({ACCEPTED[key]})")
                    print(f"         fontTools: {va}")
                    print(f"         ours:      {vb}")
                continue
            print(f"  FAIL {key}\n         fontTools: {va}\n         ours:      {vb}")
            failures += 1
        print()

    if failures:
        print(f"{failures} difference(s) beyond the accepted list", file=sys.stderr)
        return 1
    print("every CFF2 case matches fontTools, apart from the accepted differences")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
