#!/usr/bin/env python3
"""Ask fontTools what it does, so a claim about the reference implementation is measured.

Questions this answers
----------------------
    1. What does fontTools accept in an axis limit, and what value does it read?
    2. What does its instancer produce for a given job -- which tables survive, what the
       axes become, which container comes out?
    3. What is actually stored in a font's `glyf` and `maxp` -- per-glyph bounding boxes,
       composite-ness, instruction sizes?

Every one of those has been used to settle a disagreement in this project, and each time
the answer went into a commit message or into `docs/adjudication.md`. This is the script
those numbers came from.

Why fontTools is the arbiter at all: the original Slice hands its whole job to
`instantiateVariableFont`, so fontTools' behaviour *is* the original's behaviour for
anything past the editors. And its `--axis` syntax is what users of variable fonts have
learned, so a limit string cannot reasonably mean one thing there and another here.

Usage
-----
    # question 1 -- the numbers quoted in `axes: read range bounds the way fontTools
    # reads them` and in Part 1 of docs/adjudication.md
    tests/suite/probe-fonttools.py limits 'wght=300:1e3' 'wght=.5:900' 'wght=1e3'

    # question 2 -- what a CFF2 font becomes when pinned, and what container comes out
    tests/suite/probe-fonttools.py instance tests/suite/fixtures/out/cff2-vf.otf wght=700
    tests/suite/probe-fonttools.py instance testdata/fonts/Recursive-VF.subset.ttf \
        wght=400 --save-as /tmp/o.woff

    # question 3 -- the composite bbox table in `partial: fill in composite bounding
    # boxes`, and the malformed maxp in the `hinted` fixture
    tests/suite/probe-fonttools.py glyf tests/suite/fixtures/out/composites.ttf
    tests/suite/probe-fonttools.py glyf tests/suite/fixtures/out/hinted.ttf

`limits` takes fontTools' own `tag=value` spelling, not Slice's, because it is fontTools
being asked. A range is `min:max`, and a value beginning with `-` needs a `--` separator
first or argparse claims it as a flag.
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
from pathlib import Path

SUITE = Path(__file__).resolve().parent
REPO = SUITE.parent.parent
VENV_PYTHON = REPO / ".suite-venv" / "bin" / "python"


def reexec_into_venv() -> None:
    """The suite's venv holds the pinned fontTools; a bare python probably does not."""
    try:
        import fontTools  # noqa: F401
    except ImportError:
        if not VENV_PYTHON.exists():
            raise SystemExit(
                "fontTools is not importable and .suite-venv does not exist yet.\n"
                "Run tests/suite/run.py once; it bootstraps the venv."
            )
        os.execv(str(VENV_PYTHON), [str(VENV_PYTHON), str(Path(__file__).resolve()), *sys.argv[1:]])


reexec_into_venv()

import fontTools  # noqa: E402
from fontTools import ttLib  # noqa: E402
from fontTools.varLib.instancer import instantiateVariableFont, parseLimits  # noqa: E402


def command_limits(args) -> int:
    print(f"fontTools {fontTools.version}, parseLimits\n")
    for text in args.limits:
        try:
            parsed = parseLimits([text])
        except Exception as e:  # noqa: BLE001
            print(f"  {text!r:24} REFUSED  {type(e).__name__}: {e}")
            continue
        for tag, value in parsed.items():
            print(f"  {text!r:24} {tag} -> {value}")
    print(
        "\nA 3-tuple is (min, default, max); a `None` in the middle means the default "
        "was not restated\nand stays where the font puts it."
    )
    return 0


def command_instance(args) -> int:
    font = ttLib.TTFont(args.font)
    before = sorted(font.keys())
    location: dict = {}
    for pair in args.axes:
        tag, _, value = pair.partition("=")
        if ":" in value:
            low, _, high = value.partition(":")
            location[tag] = (float(low), float(high))
        else:
            location[tag] = float(value)

    print(f"fontTools {fontTools.version}")
    print(f"input   {Path(args.font).name}")
    print(f"        flavor={font.flavor} sfntVersion={font.sfntVersion}")
    print(f"        tables {' '.join(before)}")
    if "fvar" in font:
        for a in font["fvar"].axes:
            print(f"        {a.axisTag} {a.minValue} / {a.defaultValue} / {a.maxValue}")
    print(f"\njob     {location}\n")

    instantiateVariableFont(font, location, inplace=True)

    # Save and reload before reporting. The instancer leaves interpolated coordinates as
    # Python floats in memory and only rounds when a table is compiled, so anything read
    # straight out of the in-memory font is not what a file would hold.
    target = Path(args.save_as) if args.save_as else Path("/dev/null")
    if args.save_as:
        font.save(str(target))
        font = ttLib.TTFont(str(target))
    else:
        import io

        buffer = io.BytesIO()
        font.save(buffer)
        buffer.seek(0)
        font = ttLib.TTFont(buffer)

    after = sorted(font.keys())
    print(f"output  flavor={font.flavor} sfntVersion={font.sfntVersion!r}")
    if args.save_as:
        print(f"        written to {target} ({target.stat().st_size} bytes)")
    print(f"        tables {' '.join(after)}")
    gone = [t for t in before if t not in after]
    added = [t for t in after if t not in before]
    if gone:
        print(f"        dropped {' '.join(gone)}")
    if added:
        print(f"        added   {' '.join(added)}")
    if "fvar" in font:
        for a in font["fvar"].axes:
            print(f"        {a.axisTag} {a.minValue} / {a.defaultValue} / {a.maxValue}")
    else:
        print("        no fvar: this is a static font")
    return 0


def command_glyf(args) -> int:
    font = ttLib.TTFont(args.font)
    print(f"{Path(args.font).name}\n")
    if "glyf" not in font:
        print("  no glyf table; outlines are CFF")
    else:
        glyf = font["glyf"]
        print(f"  {'glyph':16} {'kind':10} {'bbox':28} program")
        longest = 0
        for name in font.getGlyphOrder():
            g = glyf[name]
            kind = "composite" if g.isComposite() else f"{g.numberOfContours} contour"
            box = (
                f"({g.xMin}, {g.yMin}, {g.xMax}, {g.yMax})"
                if hasattr(g, "xMin")
                else "(none)"
            )
            program = g.program.getBytecode() if hasattr(g, "program") else b""
            longest = max(longest, len(program))
            print(f"  {name:16} {kind:10} {box:28} {len(program)} bytes")
        print(f"\n  head bbox ({font['head'].xMin}, {font['head'].yMin}, "
              f"{font['head'].xMax}, {font['head'].yMax})")
        maxp = font["maxp"]
        declared = getattr(maxp, "maxSizeOfInstructions", None)
        print(f"  maxp.maxSizeOfInstructions {declared}, longest glyph program {longest}")
        if declared is not None and declared < longest:
            print("  ^^ malformed: the specification requires this to cover the longest "
                  "program")
    for table in ("prep", "fpgm", "cvt "):
        if table in font:
            size = (
                len(font[table].program.getBytecode())
                if hasattr(font[table], "program")
                else len(font[table].values)
            )
            print(f"  {table!r} present, {size}")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    sub = parser.add_subparsers(dest="command", required=True)

    limits = sub.add_parser("limits", help="what does fontTools read an axis limit as?")
    limits.add_argument("limits", nargs="+", metavar="TAG=VALUE")

    instance = sub.add_parser("instance", help="what does its instancer produce?")
    instance.add_argument("font")
    instance.add_argument("axes", nargs="+", metavar="TAG=VALUE")
    instance.add_argument("--save-as", help="write the result here, to check the container")

    glyf = sub.add_parser("glyf", help="what is actually stored in glyf and maxp?")
    glyf.add_argument("font")

    args = parser.parse_args()
    if args.command == "limits":
        return command_limits(args)
    if args.command == "instance":
        return command_instance(args)
    return command_glyf(args)


if __name__ == "__main__":
    raise SystemExit(main())
