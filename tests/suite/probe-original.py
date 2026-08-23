#!/usr/bin/env python3
"""Ask the original Slice what it does with a given input, and report what came out.

Question this answers
---------------------
    When the corpus and the original disagree, what does the original actually do —
    refuse, accept, or accept and quietly produce something else?

Adjudicating a failing case needs that answer as a measurement, not as a reading of the
source. This drives the original through the same runner the corpus uses and reports the
result in the terms the disagreement is usually about: did it refuse, what did it say,
and what do the interesting fields look like in whatever it wrote.

The tables of measured behaviour in `docs/original-behaviour.md` (entries B10 and B11)
came from this. Re-run it to check them.

Usage
-----
    # what does it accept in an axis cell?
    # values beginning with '-' need a -- separator, or argparse claims them as flags
    tests/suite/probe-original.py axis wght -- 400 5000 -9999 inf -inf nan 1e3 '300:700[500]'

    # what does it do to a font when the user touches nothing but the axes?
    tests/suite/probe-original.py roundtrip --axis wght=400

    # both programs, side by side
    tests/suite/probe-original.py roundtrip --axis wght=400 --both

By default it probes `testdata/fonts/Recursive-VF.subset.ttf`, whose `wght` axis is
300 / 300 / 1000. `--font` points it elsewhere. `--enrich` first adds nameIDs 16, 17, 21
and 22 to the font, which is what makes the name-stripping defect visible: the stock
fixture has none, so nothing appears to be lost.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

SUITE = Path(__file__).resolve().parent
REPO = SUITE.parent.parent
VENV_PYTHON = REPO / ".suite-venv" / "bin" / "python"
DEFAULT_FONT = REPO / "testdata" / "fonts" / "Recursive-VF.subset.ttf"

# Enough to pin every axis of the default fixture, so a probe of one axis yields a
# static font and the report is not muddied by the others surviving.
OTHER_AXES = {"MONO": "0", "CASL": "0", "slnt": "0", "CRSV": "0.5"}


def python() -> str:
    if VENV_PYTHON.exists():
        return str(VENV_PYTHON)
    return sys.executable


def enrich(source: Path, target: Path) -> None:
    """Add the name records the stock fixture lacks, so their loss is observable."""
    script = """
import sys
from fontTools import ttLib
f = ttLib.TTFont(sys.argv[1])
n = f["name"]
n.setName("Probe Typographic Family", 16, 3, 1, 0x409)
n.setName("Probe Typographic Subfamily", 17, 3, 1, 0x409)
n.setName("Probe WWS Family", 21, 3, 1, 0x409)
n.setName("Probe WWS Subfamily", 22, 3, 1, 0x409)
f.save(sys.argv[2])
"""
    subprocess.run([python(), "-c", script, str(source), str(target)], check=True)


DESCRIBE = """
import sys, json
from fontTools import ttLib
import math
f = ttLib.TTFont(sys.argv[1])
out = {}
out["variable"] = "fvar" in f
out["axes"] = [[a.axisTag, a.minValue, a.defaultValue, a.maxValue]
               for a in f["fvar"].axes] if "fvar" in f else []
out["usWeightClass"] = f["OS/2"].usWeightClass if "OS/2" in f else None
out["fsSelection"] = f["OS/2"].fsSelection if "OS/2" in f else None
out["macStyle"] = f["head"].macStyle
out["italicAngle"] = float(f["post"].italicAngle)
out["names"] = {}
for i in (1, 2, 3, 4, 6, 16, 17, 21, 22):
    r = f["name"].getName(i, 3, 1, 0x409)
    out["names"][i] = r.toUnicode() if r else None
nonfinite = []
if "glyf" in f:
    for name in f.getGlyphOrder():
        g = f["glyf"][name]
        if g.numberOfContours > 0:
            for x, y in g.coordinates:
                if not (math.isfinite(x) and math.isfinite(y)):
                    nonfinite.append(name)
                    break
out["nonFiniteGlyphs"] = nonfinite
print(json.dumps(out))
"""


def describe(path: str) -> dict:
    completed = subprocess.run(
        [python(), "-c", DESCRIBE, path], capture_output=True, text=True
    )
    if completed.returncode != 0:
        return {"error": completed.stderr.strip()[-200:]}
    return json.loads(completed.stdout)


def run_once(runner: str, axes: dict, font: Path, workdir: Path) -> dict:
    case = {"id": "probe", "input": {"axes": axes}}
    case_file = workdir / "case.json"
    case_file.write_text(json.dumps(case))
    output = workdir / f"{runner}.ttf"
    if output.exists():
        output.unlink()
    completed = subprocess.run(
        [python(), str(SUITE / "runners" / f"{runner}.py"),
         str(case_file), str(font), str(output)],
        capture_output=True, text=True,
    )
    try:
        result = json.loads(completed.stdout.strip().splitlines()[-1])
    except Exception:  # noqa: BLE001
        return {"ok": False, "error": f"runner produced nothing: {completed.stderr[:200]}"}
    if result.get("ok"):
        result["describe"] = describe(result["path"])
    return result


def command_axis(args, font: Path, workdir: Path) -> int:
    print(f"probing the {args.tag} axis of {font.name}")
    info = describe(str(font))
    for tag, low, default, high in info["axes"]:
        if tag == args.tag:
            print(f"  the axis is {low} / {default} / {high}\n")
    runners = ["original", "ours"] if args.both else ["original"]
    for value in args.values:
        axes = dict(OTHER_AXES) if args.pin_others else {}
        axes.pop(args.tag, None)
        axes[args.tag] = value
        for runner in runners:
            result = run_once(runner, axes, font, workdir)
            label = f"{value!r}" + (f" [{runner}]" if args.both else "")
            if not result.get("ok"):
                print(f"  {label:34} REFUSED  {result.get('error', '')[:70]}")
            else:
                d = result["describe"]
                kind = "variable" if d["variable"] else "static"
                extra = ""
                if d["nonFiniteGlyphs"]:
                    extra = f"  !! non-finite coords in {d['nonFiniteGlyphs']}"
                print(f"  {label:34} accepted {kind}, usWeightClass={d['usWeightClass']}{extra}")
    return 0


def command_roundtrip(args, font: Path, workdir: Path) -> int:
    axes = {}
    for pair in args.axis:
        tag, _, value = pair.partition("=")
        axes[tag] = value
    print(f"input: {font.name}, axes {axes}, both editors otherwise untouched\n")
    before = describe(str(font))
    rows = [("input", before)]
    for runner in (["original", "ours"] if args.both else ["original"]):
        result = run_once(runner, axes, font, workdir)
        if not result.get("ok"):
            print(f"{runner}: REFUSED — {result.get('error')}")
            continue
        rows.append((runner, result["describe"]))

    fields = ["usWeightClass", "fsSelection", "macStyle", "italicAngle"]
    width = max(len(label) for label, _ in rows) + 2
    # 18, not 16: a 16-bit binary string exactly fills a 16-wide column and the
    # neighbouring numbers run into it.
    print(f"{'':{width}}" + "".join(f"{f:>18}" for f in fields))
    for label, d in rows:
        cells = []
        for f in fields:
            v = d.get(f)
            cells.append(f"{v:016b}"[-16:] if f in ("fsSelection", "macStyle") and v is not None
                         else str(v))
        print(f"{label:{width}}" + "".join(f"{c:>18}" for c in cells))

    print()
    print(f"{'':{width}}" + "".join(f"{'name ' + str(i):>14}" for i in (16, 17, 21, 22)))
    for label, d in rows:
        cells = ["present" if d["names"].get(str(i), d["names"].get(i)) else "GONE"
                 for i in (16, 17, 21, 22)]
        print(f"{label:{width}}" + "".join(f"{c:>14}" for c in cells))
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--font", type=Path, default=DEFAULT_FONT)
    parser.add_argument("--enrich", action="store_true",
                        help="add nameIDs 16/17/21/22 first, so their loss is visible")
    parser.add_argument("--both", action="store_true", help="probe this implementation too")
    sub = parser.add_subparsers(dest="command", required=True)

    axis = sub.add_parser("axis", help="what does it accept in one axis cell?")
    axis.add_argument("tag")
    axis.add_argument("values", nargs="+")
    axis.add_argument("--no-pin-others", dest="pin_others", action="store_false",
                      help="leave the other axes blank instead of pinning them")

    trip = sub.add_parser("roundtrip", help="what does a slice do to a font's metadata?")
    trip.add_argument("--axis", action="append", default=[], metavar="TAG=VALUE")

    args = parser.parse_args()
    workdir = Path(tempfile.mkdtemp(prefix="slice-probe-"))
    font = args.font
    if args.enrich:
        enriched = workdir / "enriched.ttf"
        enrich(font, enriched)
        font = enriched

    if args.command == "axis":
        return command_axis(args, font, workdir)
    return command_roundtrip(args, font, workdir)


if __name__ == "__main__":
    raise SystemExit(main())
