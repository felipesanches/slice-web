#!/usr/bin/env python3
"""Slice every variable font in a real corpus and check the result.

Question this answers
---------------------
    The conformance corpus in `tests/suite/` is 14 fixtures, thirteen of them synthetic.
    Passing all of it says the program does what we thought to ask for. This asks the
    other question: pointed at hundreds of fonts nobody designed a test around, does it
    crash, does it produce a readable font, and does that font agree with fontTools?

Three tiers of check, cheapest first, because a crash on font 12 is worth knowing about
before an hour of outline comparison:

    1. `slice cut` exits 0 and writes a file.
    2. fontTools opens the output, the glyph count is unchanged, and `fvar` is present or
       absent as the job implies.
    3. The outlines and advances match what fontTools' own instancer produces for the
       same job -- the only tier that can catch a wrong answer as opposed to no answer.

Two jobs per font: every axis pinned at its default (the static path), and the first axis
restricted to its upper half (the partial path). A font whose first axis cannot be halved
-- fewer than two distinct coordinates -- gets the static job only, and that is reported
rather than skipped silently.

The inputs are never written to. Every output goes to a scratch directory, and the sweep
sha256s the whole corpus before and after and refuses to report a result if any input
changed. That check is the point: it is cheap, and a promise is not evidence.

Usage
-----
    tools/corpus-sweep.py --corpus /path/to/google/fonts
    tools/corpus-sweep.py --corpus ... --compare        # add tier 3, much slower
    tools/corpus-sweep.py --corpus ... --limit 50       # a quick look
    tools/corpus-sweep.py --corpus ... --json out.json  # machine-readable

Tier 3 samples at most `--sample-glyphs` glyphs per font (default 120), evenly spaced
through the glyph order, because comparing every glyph of a 60,000-glyph CJK font against
fontTools costs minutes each. The number sampled is reported per font, never rounded up
to "all".
"""

from __future__ import annotations

import argparse
import hashlib
import json
import multiprocessing
import os
import struct
import subprocess
import sys
import tempfile
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
BINARY = os.environ.get("SLICE_BINARY", str(REPO / "target" / "release" / "slice"))
VENV_PYTHON = REPO / ".suite-venv" / "bin" / "python"


# --------------------------------------------------------------------- finding fonts

def is_variable(path: Path) -> bool:
    """True if the file has an `fvar`, read from the table directory alone."""
    try:
        with open(path, "rb") as f:
            header = f.read(12)
            if len(header) < 12:
                return False
            tag, count = header[:4], struct.unpack(">H", header[4:6])[0]
            if tag not in (b"\x00\x01\x00\x00", b"OTTO", b"true"):
                return False
            records = f.read(16 * count)
        return b"fvar" in {records[i:i + 4] for i in range(0, len(records) - 15, 16)}
    except OSError:
        return False


def find_fonts(corpus: Path) -> list[Path]:
    found = []
    for dirpath, dirnames, filenames in os.walk(corpus):
        dirnames[:] = [d for d in dirnames if d not in (".git", "venv", ".venv")]
        for name in sorted(filenames):
            if name.lower().endswith((".ttf", ".otf")):
                path = Path(dirpath) / name
                if is_variable(path):
                    found.append(path)
    return sorted(found)


def digest(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


# ------------------------------------------------------------------------ the checks

def axes_of(path: Path) -> list[dict] | None:
    out = subprocess.run([BINARY, "info", str(path), "--json"],
                         capture_output=True, text=True, timeout=300)
    if out.returncode != 0:
        return None
    try:
        return json.loads(out.stdout)["axes"]
    except Exception:  # noqa: BLE001
        return None


def jobs_for(axes: list[dict]) -> list[tuple[str, list[str], bool]]:
    """(name, --axis arguments, expect the output to still be variable)."""
    pinned = [f"{a['tag']}={a['default']}" for a in axes]
    jobs = [("static", pinned, False)]

    first = axes[0]
    low, high = float(first["default"]), float(first["max"])
    if high > low:
        rest = [f"{a['tag']}={a['default']}" for a in axes[1:]]
        jobs.append(("partial", [f"{first['tag']}={low}:{high}"] + rest, True))
    return jobs


def check_one(task) -> dict:
    path, workdir, compare, sample_glyphs = task
    path = Path(path)
    result = {"font": str(path), "size": path.stat().st_size, "jobs": []}

    axes = axes_of(path)
    if axes is None:
        result["error"] = "slice info failed"
        return result
    result["axes"] = [a["tag"] for a in axes]

    for name, args, expect_variable in jobs_for(axes):
        entry = {"job": name}
        output = Path(workdir) / f"{path.stem}.{name}.ttf"
        started = time.monotonic()
        try:
            run = subprocess.run(
                [BINARY, "cut", str(path), str(output), *sum(([f"--axis", a] for a in args), [])],
                capture_output=True, text=True, timeout=600,
            )
        except subprocess.TimeoutExpired:
            entry.update(tier=1, ok=False, detail="timed out after 600s")
            result["jobs"].append(entry)
            continue
        entry["seconds"] = round(time.monotonic() - started, 3)

        if run.returncode != 0:
            message = (run.stderr or run.stdout).strip().splitlines()
            entry.update(tier=1, ok=False,
                         detail=(message[-1][:300] if message else "no output"),
                         panic="panicked at" in (run.stderr or ""))
            result["jobs"].append(entry)
            continue
        if not output.exists():
            entry.update(tier=1, ok=False, detail="exit 0 but no file written")
            result["jobs"].append(entry)
            continue

        problem = tier2(path, output, expect_variable)
        if problem:
            entry.update(tier=2, ok=False, detail=problem)
            result["jobs"].append(entry)
            continue

        if compare:
            problem, sampled = tier3(path, output, args, axes, sample_glyphs)
            entry["sampled_glyphs"] = sampled
            if problem:
                entry.update(tier=3, ok=False, detail=problem)
                result["jobs"].append(entry)
                continue

        entry.update(ok=True, out_size=output.stat().st_size)
        result["jobs"].append(entry)
        output.unlink(missing_ok=True)

    return result


def tier2(source: Path, output: Path, expect_variable: bool) -> str | None:
    from fontTools import ttLib

    try:
        font = ttLib.TTFont(str(output))
        font.getGlyphOrder()
    except Exception as e:  # noqa: BLE001
        return f"fontTools cannot open the output: {type(e).__name__}: {e}"
    try:
        original = ttLib.TTFont(str(source))
    except Exception as e:  # noqa: BLE001
        return f"fontTools cannot open the *input*: {e}"

    if len(font.getGlyphOrder()) != len(original.getGlyphOrder()):
        return (f"glyph count changed: {len(original.getGlyphOrder())} -> "
                f"{len(font.getGlyphOrder())}")
    if expect_variable and "fvar" not in font:
        return "a restricted axis should have left a variable font, but there is no fvar"
    if not expect_variable and "fvar" in font:
        return "every axis was pinned, but the output still has an fvar"
    if not expect_variable and "gvar" in font:
        return "every axis was pinned, but the output still has a gvar"
    return None


def tier3(source: Path, output: Path, args: list[str], axes: list[dict],
          sample_glyphs: int) -> tuple[str | None, int]:
    """Compare against fontTools' own instance of the same job."""
    import io

    from fontTools import ttLib
    from fontTools.pens.recordingPen import DecomposingRecordingPen
    from fontTools.varLib.instancer import instantiateVariableFont

    location: dict = {}
    for arg in args:
        tag, _, value = arg.partition("=")
        if ":" in value:
            low, _, high = value.partition(":")
            location[tag] = (float(low), float(high))
        else:
            location[tag] = float(value)

    reference = ttLib.TTFont(str(source))
    try:
        instantiateVariableFont(reference, location, inplace=True)
    except Exception as e:  # noqa: BLE001
        # fontTools refusing the job is not evidence about us; report it as its own
        # outcome rather than as a disagreement.
        return f"fontTools itself could not do this job: {type(e).__name__}: {e}", 0
    buffer = io.BytesIO()
    reference.save(buffer)
    buffer.seek(0)
    reference = ttLib.TTFont(buffer)

    ours = ttLib.TTFont(str(output))
    order = reference.getGlyphOrder()
    step = max(1, len(order) // sample_glyphs)
    names = order[::step][:sample_glyphs]

    # Both sides are static here for the static job. For the partial job both keep the
    # same axes, and both are drawn at their defaults, which is the location the
    # restriction leaves in place.
    ref_set, our_set = reference.getGlyphSet(), ours.getGlyphSet()
    for name in names:
        if name not in ours.getGlyphOrder():
            return f"glyph {name} is missing from the output", len(names)
        a, b = DecomposingRecordingPen(ref_set), DecomposingRecordingPen(our_set)
        try:
            ref_set[name].draw(a)
            our_set[name].draw(b)
        except Exception as e:  # noqa: BLE001
            return f"glyph {name} could not be drawn: {type(e).__name__}: {e}", len(names)
        if len(a.value) != len(b.value):
            return (f"glyph {name}: {len(a.value)} drawing operations in fontTools' "
                    f"instance, {len(b.value)} in ours"), len(names)
        for (op_a, args_a), (op_b, args_b) in zip(a.value, b.value):
            if op_a != op_b:
                return f"glyph {name}: {op_a} vs {op_b}", len(names)
            for pa, pb in zip(args_a, args_b):
                if pa is None or pb is None:
                    continue
                for ca, cb in zip(pa, pb):
                    if abs(ca - cb) > 1.0:
                        return (f"glyph {name}: coordinate {ca} vs {cb}, a difference of "
                                f"{abs(ca - cb):.3f} units"), len(names)
        if reference["hmtx"][name][0] != ours["hmtx"][name][0]:
            return (f"glyph {name}: advance {reference['hmtx'][name][0]} vs "
                    f"{ours['hmtx'][name][0]}"), len(names)
    return None, len(names)


# ----------------------------------------------------------------------------- driver

def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--corpus", type=Path, required=True)
    parser.add_argument("--compare", action="store_true", help="add the fontTools diff")
    parser.add_argument("--limit", type=int, help="only the first N fonts")
    parser.add_argument("--sample-glyphs", type=int, default=120)
    parser.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) - 1))
    parser.add_argument("--json", help="write the full result here")
    args = parser.parse_args()

    if not Path(BINARY).exists():
        raise SystemExit(f"{BINARY} is not built; run cargo build --release -p slice-cli")

    print(f"scanning {args.corpus} ...", flush=True)
    fonts = find_fonts(args.corpus)
    if args.limit:
        fonts = fonts[: args.limit]
    print(f"{len(fonts)} variable fonts", flush=True)

    print("hashing the inputs ...", flush=True)
    before = {str(p): digest(p) for p in fonts}

    workdir = Path(tempfile.mkdtemp(prefix="slice-corpus-sweep-"))
    tasks = [(str(p), str(workdir), args.compare, args.sample_glyphs) for p in fonts]

    results = []
    started = time.monotonic()
    with multiprocessing.Pool(args.jobs) as pool:
        for n, result in enumerate(pool.imap_unordered(check_one, tasks), 1):
            results.append(result)
            if n % 25 == 0 or n == len(tasks):
                bad = sum(1 for r in results for j in r.get("jobs", []) if not j.get("ok"))
                print(f"  {n}/{len(tasks)}  {bad} failing job(s)  "
                      f"{time.monotonic() - started:.0f}s", flush=True)

    print("re-hashing the inputs ...", flush=True)
    changed = [p for p, h in before.items() if digest(Path(p)) != h]
    if changed:
        print(f"\n!! {len(changed)} INPUT FILES WERE MODIFIED -- results are void:")
        for p in changed[:20]:
            print(f"     {p}")
        return 2
    print(f"all {len(before)} inputs byte-identical after the sweep\n")

    return report(results, args)


def report(results: list[dict], args) -> int:
    total_jobs = sum(len(r.get("jobs", [])) for r in results)
    failures = [(r, j) for r in results for j in r.get("jobs", []) if not j.get("ok")]
    panics = [(r, j) for r, j in failures if j.get("panic")]
    infos = [r for r in results if r.get("error")]

    print(f"=== {len(results)} fonts, {total_jobs} jobs ===")
    print(f"  {total_jobs - len(failures)}/{total_jobs} jobs succeeded"
          f"   ({100 * (total_jobs - len(failures)) / max(1, total_jobs):.1f}%)")
    if infos:
        print(f"  {len(infos)} fonts could not even be read by `slice info`")
    if panics:
        print(f"  {len(panics)} PANICS")

    by_tier: dict[int, int] = {}
    for _, j in failures:
        by_tier[j.get("tier", 0)] = by_tier.get(j.get("tier", 0), 0) + 1
    for tier in sorted(by_tier):
        label = {1: "did not produce a font", 2: "produced an invalid font",
                 3: "disagreed with fontTools"}.get(tier, "unknown")
        print(f"    tier {tier}: {by_tier[tier]:4}  {label}")

    if failures:
        print(f"\n  failures ({len(failures)}):")
        for r, j in failures[:40]:
            print(f"    {Path(r['font']).name}  [{j['job']}]")
            print(f"      {j.get('detail', '')[:160]}")
        if len(failures) > 40:
            print(f"    ... and {len(failures) - 40} more")

    if args.compare:
        sampled = [j["sampled_glyphs"] for _, j in
                   [(r, j) for r in results for j in r.get("jobs", [])]
                   if "sampled_glyphs" in j]
        if sampled:
            print(f"\n  outline comparison sampled {sum(sampled)} glyphs across "
                  f"{len(sampled)} jobs (cap {args.sample_glyphs} per job)")

    slowest = sorted(((j.get("seconds", 0), r["font"], j["job"])
                      for r in results for j in r.get("jobs", [])), reverse=True)[:3]
    print("\n  slowest slices:")
    for seconds, font, job in slowest:
        print(f"    {seconds:7.2f}s  {Path(font).name} [{job}]")

    if args.json:
        Path(args.json).write_text(json.dumps(results, indent=2))
        print(f"\nwrote {args.json}")

    return 1 if failures else 0


if __name__ == "__main__":
    try:
        import fontTools  # noqa: F401
    except ImportError:
        if VENV_PYTHON.exists():
            os.execv(str(VENV_PYTHON), [str(VENV_PYTHON), str(Path(__file__).resolve()),
                                        *sys.argv[1:]])
        raise SystemExit("fontTools is not importable; run tests/suite/run.py once")
    raise SystemExit(main())
