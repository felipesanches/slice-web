#!/usr/bin/env python3
"""Run one corpus case against the original Slice.

The original is a PyQt5 application, but its engine needs no display and no
`QApplication`: `DesignAxisModel`, `FontNameModel`, `FontBitFlagModel` and
`InstanceWorker.run()` can all be driven directly. This does exactly that, reproducing
the sequence `MainWindow.btn_clicked_slice` performs, so that what is scored is the
program's real behaviour rather than a paraphrase of it.

Fidelity matters more than convenience here, in two places that are themselves under
test:

* After `load_font`, the original clears all six bit-flag checkboxes
  (`__main__.py:730-735`). So a case that does not mention bits is run with every bit
  **off**, which is what the user would be submitting if they never touched that panel.
* `FontNameModel.load_font` fills nameIDs 1, 2, 3, 4 and 6 only. nameIDs 16, 17, 21 and
  22 stay empty, and `edit_name_table` deletes empty optional records. A case that does
  not mention names inherits exactly that.

Reproducing those faithfully is the whole point: they are claims D5 and E4, and a runner
that quietly "fixed" them would hide the defects the corpus exists to find.

Usage: original.py <case.json> <fixture.ttf> <output-path>
Prints a JSON result: {"ok": true, "path": ...} or {"ok": false, "error": ...}
"""

from __future__ import annotations

import contextlib
import io
import json
import os
import sys
import traceback

# The original's package lives here.
SLICE_SRC = os.environ.get("SLICE_ORIGINAL_SRC", "/home/fsanches/compartilhado/Slice/src")
sys.path.insert(0, SLICE_SRC)

# Editor row order, from FontNameModel.__init__.
NAME_ROWS = [1, 2, 3, 4, 6, 16, 17, 21, 22]


def run(case: dict, fixture: str, output: str) -> dict:
    from slice.models import DesignAxisModel, FontBitFlagModel, FontModel, FontNameModel
    from slice.instanceworker import InstanceWorker

    given = case.get("input", {})

    # --- load_font (__main__.py:707) -------------------------------------------------
    try:
        font_model = FontModel(fixture)
        if not font_model.is_variable_font():
            return {
                "ok": False,
                "error": (
                    "The font is missing the OpenType fvar table and is not recognized "
                    "as a variable font."
                ),
            }
    except Exception as e:  # noqa: BLE001
        return {"ok": False, "error": f"could not load the font: {e}"}

    axis_model = DesignAxisModel()
    name_model = FontNameModel()
    try:
        name_model.load_font(font_model)
        axis_model.load_font(font_model)
    except Exception as e:  # noqa: BLE001
        return {"ok": False, "error": f"could not read the font's tables: {e}"}

    # --- the user fills in the Axis Editor -------------------------------------------
    tags = list(axis_model._v_header)
    for tag, text in given.get("axes", {}).items():
        if tag not in tags:
            return {"ok": False, "error": f"the font has no {tag} axis"}
        axis_model._data[tags.index(tag)][1] = text

    # --- the user fills in the Name Editor -------------------------------------------
    for name_id, text in given.get("names", {}).items():
        name_id = int(name_id)
        if name_id not in NAME_ROWS:
            return {"ok": False, "error": f"nameID {name_id} is not an editable row"}
        name_model._data[NAME_ROWS.index(name_id)][0] = text

    # --- the Bit Flag Editor ----------------------------------------------------------
    # Every box starts unchecked, because load_font cleared them. This is claim A5/E4.
    bits = given.get("bits", {})
    os2_bits = {f"bit{b}": False for b in (0, 5, 6, 8)}
    head_bits = {f"bit{b}": False for b in (0, 1)}
    for bit, on in bits.get("fsSelection", {}).items():
        key = f"bit{int(bit)}"
        if key not in os2_bits:
            return {"ok": False, "error": f"fsSelection bit {bit} is not in the editor"}
        os2_bits[key] = bool(on)
    for bit, on in bits.get("macStyle", {}).items():
        key = f"bit{int(bit)}"
        if key not in head_bits:
            return {"ok": False, "error": f"macStyle bit {bit} is not in the editor"}
        head_bits[key] = bool(on)

    # --- btn_clicked_slice (__main__.py:559) ------------------------------------------

    # The parse-and-validate step, which raises ValueError on a malformed entry.
    try:
        values_present = axis_model.instance_data_validates_missing_data()
    except ValueError as e:
        return {"ok": False, "error": str(e)}
    except Exception as e:  # noqa: BLE001
        return {"ok": False, "error": f"{type(e).__name__}: {e}"}

    if not values_present:
        return {
            "ok": False,
            "error": (
                "You requested the same design space that is supported in the font path "
                "that you are processing. Please define at least one axis location or "
                "restricted axis range."
            ),
        }

    if given.get("remove_overlaps"):
        # Claim G14: the original has no such option. Reported as a refusal so the
        # scorer can tell "this program does not have the feature" apart from "this
        # program has the feature and got it wrong".
        return {"ok": False, "error": "the original Slice cannot remove overlaps"}

    bit_model = FontBitFlagModel(os2_bits, head_bits)
    worker = InstanceWorker(output, font_model, axis_model, name_model, bit_model)

    # InstanceWorker.run swallows every exception into an error signal and prints a
    # debugging report to stdout. Capture both.
    errors: list[str] = []
    worker.signals.error.connect(errors.append)
    noise = io.StringIO()
    try:
        with contextlib.redirect_stdout(noise), contextlib.redirect_stderr(noise):
            worker.run()
    except Exception as e:  # noqa: BLE001
        return {"ok": False, "error": f"{type(e).__name__}: {e}"}

    if errors:
        return {"ok": False, "error": errors[0]}
    if not os.path.exists(output):
        return {"ok": False, "error": "the worker reported success but wrote no file"}
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
