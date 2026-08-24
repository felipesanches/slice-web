---
layout: "default"
title: "What the original Slice does"
description: "A numbered map of the original program's behaviour"
---

A behavioural map of [Slice](https://github.com/source-foundry/Slice) 0.7.1 by Source
Foundry, read out of its source rather than its documentation. Every entry is a numbered,
testable claim; the test corpus in `tests/suite/` references these IDs, and every claim
carries at least one case.

Read this before reading the corpus. It is the answer to "what is the thing we are
trying to be compatible with", and several entries record behaviour that turns out to be
wrong — noted as such, with the evidence, rather than quietly reproduced.

## How to read an entry

Each claim has an **ID**, a statement, where it lives in the original, and a **class**:

| class | meaning |
|---|---|
| `spec` | the OpenType specification requires this |
| `fonttools` | behaviour inherited from `fontTools`, which the original delegates to |
| `slice-ui` | the application's own contract, documented or evident from its interface |
| `judgement` | a defensible design choice with no external authority behind it |
| `suspect` | behaviour that looks like a defect; carries the evidence and a proposed verdict |

A `suspect` entry is not a licence to differ. It is a flag for adjudication: the test
corpus encodes what the behaviour *should* be, the original is scored against that, and
the disagreement is resolved on evidence.

---

## A. Loading a font

**A1** · `slice-ui` · A file is accepted as a font if `fontTools.ttLib.TTFont` can open
it. Anything else raises, and the error is shown in a dialog with the exception text as
detail.
*(`__main__.py:707` `load_font`)*

**A2** · `slice-ui` · A font is treated as variable if and only if it has an `fvar`
table. A font without one is rejected with: "The font is missing the OpenType fvar table
and is not recognized as a variable font."
*(`models.py:454` `FontModel.is_variable_font`)*

**A3** · `fonttools` · TrueType, CFF, WOFF and WOFF2 inputs are all readable, because
`TTFont` reads all four. The open dialog filters on `.ttf .otf .woff .woff2`.
*(`dialogs.py:44`)*

**A4** · `slice-ui` · On a successful load the status bar reads
`{family} {version} loaded ({n} axes)`, where family is nameID 1 and version is nameID 5
truncated at the first `;`.
*(`__main__.py:746`, `models.py:160`)*

**A5** · `slice-ui` · Loading a font resets all six bit-flag checkboxes to unchecked,
regardless of what the font contains.
*(`__main__.py:730-735`)* — **also D5**, and see `suspect` there.

---

## B. Axis Editor — parsing and validation

**B1** · `slice-ui` · An empty cell means "leave this axis alone": the axis keeps its
original range and stays in the output.
*(`models.py:288`)*

**B2** · `slice-ui` · A cell containing `:` is a restricted range. Anything else
non-empty is a pin.
*(`models.py:292`)*

**B3** · `slice-ui` · A pin is parsed with Python's `float()`.
*(`models.py:299`)*

**B4** · `slice-ui` · A pin that `float()` rejects raises
`'{value}' is not a valid {tag} axis value. Please enter a single numeric value and try
again.`
*(`models.py:302`)*

**B5** · `slice-ui` · A range is parsed with the regular expression
`(?P<start>-?\d+(\.\d+)?)\s*:\s*(?P<end>-?\d+(\.\d+)?)\s*(\[\s*(?P<default>-?\d+(\.?\d+)?)\s*\])?`
*(`models.py:200`)*

**B6** · `slice-ui` · A reversed range is sorted, so `800:400` and `400:800` mean the
same thing.
*(`models.py:344`)*

**B7** · `slice-ui` · A restricted range must contain the axis's original default value,
or it is rejected with `The {tag} range {min}:{max} does not include the default axis
value ({default}). This is currently a requirement.` This is the Level 3 sub-spacing rule:
the compiler cannot move a default axis location.
*(`models.py:352`, and the project README's footnote 1)*

**B8** · `slice-ui` · A trailing `[default]` group parses and is then **discarded**. The
code comments name it as groundwork for Level 4 sub-spacing.
*(`models.py:322-324`)*
**`suspect`.** Accepting `200:700[400]` and silently producing `200:700` gives the user a
font that is not what they asked for, with no warning. Proposed verdict: an unsupported
request must be refused, not silently reinterpreted. Evidence: the OpenType specification
has no notion of "ignore part of the user's request"; and the value is parsed, so the
program demonstrably understands what was asked and declines to say it cannot do it.

**B9** · `slice-ui` · A range whose bounds fall outside the axis's own extent is
**not** checked by the editor. It reaches fontTools, which clamps it into range.
*(no check in `models.py`; `fontTools` `AxisTriple.limitRangeAndPopulateDefaults`)*

**B10** · `fonttools` · A pin outside the axis extent is likewise not checked, and is
clamped by fontTools.
**`suspect`.** Measured against the original on a 300/300/1000 `wght` axis:

| typed | result |
|---|---|
| `5000` | accepted, `usWeightClass` 1000 |
| `-9999` | accepted, `usWeightClass` 300 |

Proposed verdict: warn or refuse. Weak `suspect` — clamping is what fontTools does
deliberately, and a case can be made either way. Flagged for adjudication rather than
assumed.

**B11** · The pin parser is `float()`, which accepts `nan`, `inf`, `-inf`, `1e3` and
leading/trailing whitespace.
**`suspect`.** Measured against the original, on a 300/300/1000 `wght` axis:

| typed | result |
|---|---|
| `inf` | **accepted**, silently clamped to 1000 |
| `-inf` | **accepted**, silently clamped to 300 |
| `nan` | refused, but by fontTools deep inside normalization, with the message `Ooops... v=nan, triple=(300.0, 300.0, 1000.0)` |
| `1e3` | accepted as 1000, which is correct |

An earlier draft of this entry claimed a NaN would propagate into the outlines. That is
**wrong** and the measurement above corrects it: fontTools catches NaN before it reaches
any delta, and the output coordinates are always finite. The real defects are smaller but
real: `inf` is not a weight, and silently reading it as "the heaviest available" invents
an intent the user never expressed; and `Ooops...` is not an error message a user can act
on. Proposed verdict: reject non-finite input in the editor, with a message naming the
axis. Evidence: OpenType axis coordinates are `Fixed` (16.16), a format with no
representation for infinity or NaN, so no such value can ever be written to a font — the
input is meaningless rather than merely out of range.

**B12** · The range parser uses `re.search`, not `re.fullmatch`.
**`suspect`.** `xyz200:700zzz` matches, because `search` finds `200:700` anywhere in the
string. So does `400:700 garbage`. Proposed verdict: the whole cell must parse. Evidence:
every other malformed entry is refused (B4); accepting a value embedded in noise is
inconsistent with that and hides typos.

**B13** · `slice-ui` · If every cell is empty, the Slice button refuses the job: "You
requested the same design space that is supported in the font path that you are
processing. Please define at least one axis location or restricted axis range."
*(`models.py:363`, `__main__.py:579`)*

---

## C. Axis Editor — display

**C1** · `slice-ui` · One row per `fvar` axis, in `fvar` order, labelled with the axis
tag.
*(`models.py:247-277`)*

**C2** · `slice-ui` · The read-only column shows `{min} : {max} [{default}]` using the
`fvar` values.
*(`models.py:262`)*

**C3** · `slice-ui` · The row tooltip is a human-readable axis name: the registered name
for `ital opsz slnt wdth wght`, the Google Fonts registry name for
`CASL CRSV XPRN GRAD MONO SOFT WONK`, otherwise the axis's own `name` table entry.
*(`models.py:380-408`)*

**C4** · `slice-ui` · The axis-name lookup consults the font's `name` table only as a
fallback, so a font that names `wght` something unusual still displays "Weight".
*(`models.py:404`)*

---

## D. Name Editor

**D1** · `slice-ui` · Nine rows, for nameIDs 1, 2, 3, 4, 6, 16, 17, 21, 22, labelled
`01 Family`, `02 Subfamily`, `03 Unique`, `04 Full`, `06 Postscript`, `16 Typo Family`,
`17 Typo Subfamily`, `21 WWS Family`, `22 WWS Subfamily`.
*(`models.py:63-88`)*

**D2** · `slice-ui` · Only the Windows / Unicode BMP / English (US) record — platform 3,
encoding 1, language 1033 — is read and written.
*(`models.py:93-95`, `instanceworker.py:93`)*

**D3** · `slice-ui` · nameIDs 1, 2, 3, 4 and 6 are always written to the output, even
when the editor cell is empty.
*(`instanceworker.py:97-101`)*

**D4** · `slice-ui` · nameIDs 16, 17, 21 and 22 are written when the cell is non-empty
and **removed from the font** when it is empty.
*(`instanceworker.py:108-136`)*

**D5** · The editor loads nameIDs 1, 2, 3, 4 and 6 from the font. It never loads 16, 17,
21 or 22.
*(`models.py:90-142` — no branch for those IDs)*
**`suspect`, and consequential.** Combined with D4, every font Slice touches loses its
Typographic and WWS family names. For a family with more than four styles those records
are how the OS groups it; stripping them silently breaks style linking. Proposed verdict:
load all nine, so an untouched editor preserves what the font had. Evidence: the
OpenType `name` specification describes nameIDs 16/17 as required "if the font has more
than four styles that are part of a family"; deleting them from such a font makes it
non-conformant. This is the clearest defect in the program.

**D6** · `slice-ui` · nameID 5 is read for the status bar's version display but is not
editable and is not rewritten.
*(`models.py:38-43`)*

---

## E. Bit Flag Editor

**E1** · `slice-ui` · Four `OS/2.fsSelection` checkboxes: bit 0 ITALIC, bit 5 BOLD,
bit 6 REGULAR, bit 8 WWS.
*(`__main__.py:380-384`)*

**E2** · `slice-ui` · Two `head.macStyle` checkboxes: bit 0 BOLD, bit 1 ITALIC.
*(`__main__.py:387-388`)*

**E3** · `slice-ui` · Each checkbox sets its bit when ticked and clears it when unticked.
Bits not exposed by the editor keep whatever value the font had.
*(`models.py:415-435`)*

**E4** · Loading a font clears all six checkboxes (A5), and slicing then writes that
state.
**`suspect`.** Any font whose `OS/2.fsSelection` has bit 6 REGULAR set — which is most
Regular-weight fonts — has it cleared unless the user notices and re-ticks it. The
resulting font declares itself neither Regular nor Bold nor Italic. Proposed verdict:
read the bits from the font, so an untouched editor is a no-op. Evidence: the OpenType
`OS/2` specification states that bit 6 "should be set if the font is a regular style" and
that bits 0, 5 and 6 are mutually informative; silently clearing them changes how the
font is classified. This is a data-loss defect of the same shape as D5.

**E5** · The editor permits contradictory combinations — REGULAR together with BOLD, or
`fsSelection` BOLD without `macStyle` BOLD — with no warning.
**`suspect`, mild.** The OpenType `OS/2` specification says bit 6 "is mutually exclusive
with bits 0 and 5", and that `head.macStyle` bits 0 and 1 "must be consistent with"
`fsSelection` bits 5 and 0. Proposed verdict: still allow it, since the user may be
correcting a font deliberately, but say so. A warning, not an error.

---

## F. The Slice action

**F1** · `slice-ui` · With no font loaded, the status bar reads "Requires a font path"
and nothing happens.
*(`__main__.py:561`)*

**F2** · `slice-ui` · Axis entries are validated before the save dialog opens; a parse
error is shown and the job abandoned.
*(`__main__.py:573-577`)*

**F3** · `slice-ui` · The job is refused if it does not narrow the design space (B13).
*(`__main__.py:579`)*

**F4** · `slice-ui` · If the font-path text field no longer matches the loaded font, the
job is refused: "The file path in the font path field does not match the loaded font
path. Please load your font again."
*(`__main__.py:589`)*
Not portable: it exists only because that field is editable.

**F5** · `slice-ui` · Cancelling the save dialog sets the status to "Canceled" and does
nothing else.
*(`__main__.py:604`)*

**F6** · `slice-ui` · The work runs on a `QThreadPool` with a modal indeterminate
progress dialog; failure shows an error dialog with the exception text and sets the
status to "Failed".
*(`__main__.py:610-650`)*

---

## G. What is written

The original delegates the whole of instancing to one call:

```python
instantiateVariableFont(self.ttfont, axis_instance_data, inplace=True, optimize=True)
```
*(`instanceworker.py:83`)*

so its output behaviour **is** fontTools 4.x's, at that call's defaults. The claims below
are the ones a test can observe.

**G1** · `fonttools` · Pinning every axis yields a static font: no `fvar`, no `gvar`, no
`HVAR`/`VVAR`/`MVAR`. `STAT` **survives** with all of its design axis records, which is
why name IDs that only `STAT` refers to legitimately remain in a font with no axes left
(see G5 and G10).

**G2** · `fonttools` · Pinning some axes and restricting others yields a variable font
whose `fvar` holds only the surviving axes, with the narrowed extents.

**G3** · `fonttools` · `avar` segment maps are renormalized onto the new extents.

**G4** · `fonttools` · Named instances that fall outside the new design space are dropped
from `fvar`.

**G5** · `fonttools` · `STAT` is kept; its axis values outside the new limits are
dropped, and its design axis records are kept in full.

**G6** · `fonttools` · `GSUB`/`GPOS` feature variations are resolved: a condition set that
holds at the new default has its substitutions folded into the feature list, one that
cannot hold is dropped, one that still depends on a surviving axis is renumbered and
renormalized.

**G7** · `fonttools` · `OS/2.usWeightClass`, `OS/2.usWidthClass` and `post.italicAngle`
are set from the pinned location (`varLib.set_default_weight_width_slant`).

**G8** · `fonttools` · `maxp`'s point, contour and composite maxima, the `head` bounding
box and flags bit 1, and `hhea`'s extremes are recalculated from the final outlines on
save (`maxp.recalc` and `hhea.recalc`, both gated on `recalcBBoxes`).

An earlier draft said "`maxp` ... recalculated" without qualification, which overstates
it. `maxp`'s **instruction** fields are not touched. Measured: planting
`maxSizeOfInstructions = 1` into a font whose longest glyph program is 5 bytes, then
instancing and saving, leaves it at 1. Those fields are inherited from the input, so a
font that arrives with them wrong stays wrong.

**G9** · `fonttools` · `OS/2.xAvgCharWidth` is recalculated.

**G10** · `fonttools` · Name records that existed only to name axes and instances the
output no longer has are pruned.

**G11** · `fonttools` · `DSIG` is deleted, because it signs bytes that no longer exist.

**G12** · `fonttools` · At the default overlap mode (`KEEP_AND_SET_FLAGS`), a static
instance gets `OVERLAP_SIMPLE` set on the first point of every simple glyph with
contours, and `OVERLAP_COMPOUND` on the **first component** of every composite — not on
every component, and not at all on a glyph with zero contours such as an empty
`.notdef`.

**G13** · `fonttools` · Outlines at the requested location match what a renderer produces
for the variable font at that location.

**G14** · The original never removes overlaps. `instantiateVariableFont` accepts
`overlap=OverlapMode.REMOVE`, and the original does not pass it.
**Not a defect — the missing feature this project exists to add.** Recorded here so the
corpus can mark cases that only the new program is expected to pass.

---

## H. Containers

**H1** · The output container is the **input's** container, whatever the user names the
file. `TTFont.save` never looks at the filename: it passes `self.flavor` to
`SFNTWriter` (`ttFont.py:411`), and `self.flavor` was set from the reader when the font
was opened (`ttFont.py:318`).
**`suspect`.** Measured, saving each input under each name:

| opened | saved as | container actually written |
|---|---|---|
| `.ttf` | `.woff` | **sfnt** |
| `.ttf` | `.woff2` | **sfnt** |
| `.woff` | `.ttf` | **WOFF** |
| `.woff2` | `.ttf` | **WOFF2** |

An earlier draft of this entry asserted the opposite and cited `dialogs.py:44`, which is
the *open* dialog's filter and says nothing about saving. The correction matters: a user
who opens a TTF and types `Bold.woff` gets an sfnt with a `.woff` name, which a web
server will send with the wrong MIME type and a build tool will misidentify. Proposed
verdict: the extension chooses the container, which is what the save dialog's own filter
list implies it will do. Evidence: WOFF and WOFF2 are distinct formats with their own
signatures (`wOFF`, `wOF2`), and the file extension is the only thing the user was given
to express which one they wanted.

A consequence: **H2**'s zopfli setting only ever takes effect when the input was already
a WOFF, since that is the only way a WOFF gets written.

**H2** · `slice-ui` · WOFF output is compressed with zopfli, because the worker sets
`sfnt.USE_ZOPFLI = True` before saving.
*(`instanceworker.py:69`)*
Observable only as file size, and not a correctness property; a WOFF written with any
deflate encoder is equally valid.

---

## I. The shell

Recorded for completeness. None of it is observable in an output font, and none of it is
portable to a web page unchanged, so the corpus does not test it.

**I1** File menu: Open Font (Ctrl+O), Quit.
**I2** References menu: fvar, head, name, OS/2 specification links.
**I3** Help menu: About, Check for Updates, Release Notes, Documentation, View License,
View Source, Issue Tracker, Report a Bug.
**I4** Drag and drop onto the font-path field.
**I5** Status bar carrying the version number.
**I6** The window omits its own title and logo when the screen is under 1000 px tall.

---

## Summary of suspected defects

Eight, in descending order of how much damage they do:

| id | defect | proposed verdict |
|---|---|---|
| D5 + D4 | Typographic and WWS family names are stripped from every font | load all nine records |
| E4 + A5 | `OS/2.fsSelection` and `head.macStyle` bits are cleared on load and written cleared | read the bits from the font |
| B11 | a pin of `nan` or `inf` is accepted | reject non-finite values |
| B12 | `re.search` accepts a range embedded in surrounding garbage | require the whole cell to parse |
| B8 | `[default]` is parsed and silently discarded | refuse the request |
| B10 | an out-of-range pin is silently clamped | warn; weak, flagged for adjudication |
| H1 | the output container is the input's, whatever the file is named | the extension chooses |
| E5 | contradictory bit combinations pass unremarked | warn |

Each is encoded in the corpus as the *corrected* behaviour, so the original is expected
to fail those cases. Section 4 of the plan adjudicates each one on evidence before any
of it is treated as settled.
