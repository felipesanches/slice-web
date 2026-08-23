#!/usr/bin/env python3
"""Run one corpus case against this implementation, through `slice-cli`.

The command line is the engine's front door for anything that is not a browser, and it
accepts exactly the same inputs the interface does: the Axis Editor's text, name records
by ID, bit flags by number, overlap removal, and an output path whose extension chooses
the container. So a case can be handed to it unchanged.

One asymmetry with the original's runner is deliberate and is itself under test. Here,
"the user did not touch the Bit Flag Editor" means no `--fs-selection` or `--mac-style`
argument at all, and the engine keeps whatever the font had. In the original it means all
six boxes unchecked, because loading a font clears them. Both runners reproduce their own
program honestly; the corpus decides which is right.

Usage: ours.py <case.json> <fixture.ttf> <output-path>
Prints a JSON result: {"ok": true, "path": ...} or {"ok": false, "error": ...}
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import traceback

# runners/ -> suite/ -> tests/ -> repository root
REPO_ROOT = os.path.dirname(
    os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
)
DEFAULT_BINARY = os.path.join(REPO_ROOT, "target", "debug", "slice")


def run(case: dict, fixture: str, output: str) -> dict:
    binary = os.environ.get("SLICE_BINARY", DEFAULT_BINARY)
    if not os.path.exists(binary):
        return {"ok": False, "error": f"{binary} is not built; run cargo build -p slice-cli"}

    given = case.get("input", {})

    if given.get("load_font") is False:
        return {"ok": False, "error": "Requires a font path"}

    if given.get("action") == "load_only":
        completed = subprocess.run(
            [binary, "info", "--json", fixture], capture_output=True, text=True, timeout=120
        )
        if completed.returncode != 0:
            return {"ok": False, "error": (completed.stderr or "").strip()}
        try:
            info = json.loads(completed.stdout)
        except Exception as e:  # noqa: BLE001
            return {"ok": False, "error": f"info --json produced no JSON: {e}"}
        return {"ok": True, "editor": {"axes": info.get("axes", [])}}

    argv = [binary, "cut", fixture, output]

    for tag, text in given.get("axes", {}).items():
        # Passed through exactly as typed, including whitespace and anything malformed:
        # what the parser does with it is the thing being tested.
        argv += ["--axis", f"{tag}={text}"]

    for name_id, text in given.get("names", {}).items():
        argv += ["--name", f"{name_id}={text}"]

    bits = given.get("bits", {})
    for bit, on in bits.get("fsSelection", {}).items():
        argv += ["--fs-selection", f"{bit}={'on' if on else 'off'}"]
    for bit, on in bits.get("macStyle", {}).items():
        argv += ["--mac-style", f"{bit}={'on' if on else 'off'}"]

    if given.get("remove_overlaps"):
        argv += ["--remove-overlaps"]

    try:
        completed = subprocess.run(argv, capture_output=True, text=True, timeout=300)
    except subprocess.TimeoutExpired:
        return {"ok": False, "error": "timed out after 300s"}

    if completed.returncode != 0:
        message = (completed.stderr or completed.stdout or "").strip()
        # The CLI prefixes failures with "error: "; the corpus matches on substrings, so
        # strip it to keep messages comparable between the two programs.
        if message.startswith("error: "):
            message = message[len("error: "):]
        return {"ok": False, "error": message or f"exited with status {completed.returncode}"}

    if not os.path.exists(output):
        return {"ok": False, "error": "reported success but wrote no file"}
    return {"ok": True, "path": output}


def main() -> int:
    case_path, fixture, output = sys.argv[1], sys.argv[2], sys.argv[3]
    with open(case_path) as f:
        case = json.load(f)
    try:
        result = run(case, fixture, output)
    except Exception:  # noqa: BLE001
        result = {"ok": False, "error": f"runner crashed: {traceback.format_exc()}"}
    print(json.dumps(result))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
