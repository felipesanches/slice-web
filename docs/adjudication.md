# Adjudication: every case the original fails, and why

The corpus in `tests/suite/` is written against what a Slice-like program *should* do, not
against what either implementation happens to do. So when the original fails a case there
are only two possibilities, and the difference matters:

* **the test is wrong** — it asserts something no correct program could satisfy, or it
  measures the wrong thing. Then the test gets fixed, and the fix is a commit with
  evidence.
* **the original has a defect** — then the test stands, and this implementation must not
  copy the behaviour.

This document records the verdict for every case in both categories. Nothing here is
argued from reading the source alone: each verdict names a measurement and the artefact
that produces it, so it can be re-run.

## Scoreboard

Against the 297-case corpus, 17 of which need overlap removal that the original never had
and which are therefore not counted against it:

| | passed | failed | needs a feature it lacks |
|---|---|---|---|
| the original | 230 | 50 | 17 |
| this implementation | 297 | 0 | 0 |

An earlier revision of this file recorded seven failures here, all CFF: this
implementation refused CFF outlines and the original, delegating to fontTools'
`instantiateCFF2`, did not, which was the one place the original scored better. That gap
is now closed — CFF2 is instanced rather than refused — and closing it turned up the
tenth wrong test, below.

Reproduce with:

```
tests/suite/run.py --verbose          # both
```

## Part 1 — tests that were wrong

Ten cases were unpassable, or were measuring something other than what they claimed.
Every one of them was caught by a *correct* implementation failing it, which is the
argument for running the corpus against two programs rather than one.

### The checker could not see composite glyphs

`checker.py::_draw` used a plain `RecordingPen`, which records `addComponent` verbatim. A
composite glyph therefore arrived as a list of glyph *names* and transforms with no
geometry in it, flattened to zero contours, and every point in the glyph read as outside
it. Two cases reported "1125 sampled points changed fill state" on fonts that were
correct; the same blindness handed the coordinate rounder a string and raised `TypeError`.

Measured: with a `DecomposingRecordingPen`, the identical check on the identical fonts
reports 10626 points in agreement.

Affected `overlap.overlapping-components-are-merged`,
`overlap.non-overlapping-composite-is-not-decomposed`. Fixed in `a3090e7`.

### An absent feature was not allowed to run zero lookups

`feature_lookup_count` returned failure whenever the named feature was missing, even for a
case asserting `equals: 0`. Four feature-variation cases each documented, in their own
rationale, that fontTools **deletes** the `rvrn` record once its lookups are resolved away
— and then demanded that the record be present and empty. The original failed all four for
doing exactly what those rationales called correct.

Affected `static.pin-all.dead-feature-variation-dropped`,
`partial.featurevars-unreachable-condition-dropped`,
`partial.featurevars-condition-outside-restricted-range-dropped`,
`partial.featurevars-all-records-die-but-axes-survive`. Fixed in the commit above this
file.

### Outlines were compared against a float interpolation

`outlines_match_source_at` drew its reference through `getGlyphSet(location=…)`, which
interpolates in floating point. A font cannot: `glyf` stores integers, so a pinned instance
holds 225 where the interpolation says 224.5. `outlines.static-matches-source-at-extremes`
sets a tolerance of **zero** deliberately — at a corner of the design space every scalar is
1 or 0 and the delta arithmetic is exact — and that deliberate strictness was unsatisfiable
by any correct program, because half a unit of rounding is not a claim about instancing.

The reference is now `instantiateVariableFont`, compiled and reloaded. Compiling matters:
the instancer leaves interpolated coordinates as Python floats in memory and rounds only
when the table is written.

`tests/suite/probes/probe-outline-tolerance.py` measures the gap directly, as the worst
coordinate disagreement over every point of every glyph between a saved instance and an
interpolation of the source at the same location:

```
wght=300,  every axis at an end     coord 0.0000
wght=653.7                          coord 0.4973
MONO=0.5 CASL=0.5 wght=500 slnt=-7  coord 0.4998
MONO=1 wght=300 slnt=-15 CRSV=0     coord 0.0000
CASL=1 wght=1000 CRSV=1             coord 0.0000
```

which is the case's own premise, measured: at a corner of the design space the deltas are
applied undivided and the disagreement is *exactly* nothing, while in the interior it is
one int16 rounding and never more. Zero tolerance at a corner is not merely satisfiable, it
is the right requirement — and 0.5 is the right one everywhere else.

Fixed in `5d8c8fa`.

### An axis extent was compared for exact float equality

`fvar` stores axis values as 16.16 `Fixed`. 0.95 is not representable in it and reads back
as 0.9499969482421875, so `partial.featurevars-condition-renormalized-to-restricted-range`
could not be satisfied by anything. Comparison now allows one ulp of that type.

Fixed in `35e060c`.

### A case asserted two contradictory things about one font

`overlap.clean-glyph-is-left-alone` enabled overlap removal and then asserted
`outlines_match_source_at` across the whole font, which requires `bars` to be unchanged —
contradicting `overlap.two-bars-merge-into-one-contour` on the same fixture, and ignoring
that instancing to wght=400 moves every outline by itself.

Its own rationale described the right check: comparison against the same job with the
feature switched off. That control case now exists, and the assertion is scoped to `clean`
and made point-level, which is what "not rebuilt" means — a rebuilt glyph can draw the same
shape with different points, and that is the cost the case exists to forbid.

Fixed in `a3090e7`.

### A case named an axis its fixture has never had

`partial.composites-survive-a-restricted-range` pinned `wdth` on a fixture with only
`wght`, pinned rather than restricted despite its title, and listed check locations at
`wght: 100` on an axis that starts at 400. It could not run at all.

Re-encoded on the axis the fixture has. It immediately found a real defect in *this*
implementation: composite bounding boxes were being written as (0, 0, 0, 0). Fixed in
`76e9295`.

### A fixture shipped malformed

`hinted.ttf` declared `maxp.maxSizeOfInstructions = 0` in front of a five-byte glyph
program. Nothing in the varLib build path computes that field. So
`static.hinting.maxp-instruction-limits-cover-the-programs` was unpassable by any
implementation that correctly leaves the instruction maxima alone — which is precisely what
that case's own rationale requires, citing fontTools' `maxp.recalc` documenting that it
recomputes "most other maxp values except for the TT instructions values".

Fixed in the fixture builder, where the defect was, rather than in the case. Commit
`b6e3a81`.

### Two cases required a refusal where the correct answer was a correct reading

`axis.range-scientific-notation-is-refused` and `axis.range-leading-dot-bound-is-refused`
demanded that `300:1e3` and `.5:900` be rejected, on the grounds that the original's range
grammar — a regular expression — has no production for an exponent or a leading decimal
point. Both rationales explicitly left the permissive reading open: *"extending the range
grammar to accept exponents would also be acceptable and would need its own case, but
silently reading `1e3` as `1` is not."*

Measured against fontTools 4.62.1, whose instancer is what Slice hands its plan to and
whose `--axis` syntax users learn:

```
parseLimits(['wght=300:1e3'])   ->  {'wght': (300, None, 1000)}
parseLimits(['wght=.5:900'])    ->  {'wght': (0.5, None, 900)}
```

A limit string cannot mean 300-to-1000 in `fonttools varLib.instancer` and be a syntax
error here. Both cases keep their subject and change their obligation from "refuse" to
"read correctly", which is the **stronger** check: each is now built so the two readings
are distinguishable. Commit `d1d2c0d`.

### `filled_region_matches` drew the reference at the wrong location

Every case using this check writes its reference location as
`reference.source_at`, which is what `tests/suite/README.md` documents. `checker.py` read
`reference.location`, found nothing, and fell back to `{}` — so the source was drawn at
its **default** instance rather than at the location the case named.

That was invisible for four of the five cases: they all use the `overlapping` fixture and
pin `wght` at 400, which *is* its default, so the two readings coincide. The fifth,
`cff.remove-overlaps-preserves-filled-region`, pins the `cff2-vf` fixture at wght=700, and
there the check compared a correctly merged bold outline against an unmerged regular one
and reported **2019 sampled points as having changed fill state**.

Measured: reading `source_at`, the same output against the same fixture reports **zero**
mismatched points in all four glyphs, on the checker's own 41 x 37 grid with its own
half-unit edge margin. The independent Rust check
`overlap_removal_preserves_shape::the_filled_region_survives_overlap_removal_on_cff2`
agrees, over a 61 x 61 grid.

Affected `cff.remove-overlaps-preserves-filled-region`; `location` is still accepted as a
spelling so a case written either way means the same thing.

## Part 2 — defects in the original

The remaining 50 failures reduce to four root causes. None is a matter of taste; each
produces a font that misrepresents itself.

### B10 / B11 / B12 — the axis cell is matched, not parsed (15 cases)

`DesignAxisModel.axis_range_regex` is applied with `re.search`, not `re.fullmatch`, and the
pin path is a bare `float()`. Together these mean the cell is scanned for something that
looks like a number and the rest is discarded.

Measured, with `tests/suite/probe-original.py axis`:

| typed | what the original does |
|---|---|
| `wght=300:1e3` | accepted as `1:300`, clamped to `300:300`, **the weight axis is deleted** — a static Light font, no message |
| `CRSV=.25:1` | read as `25:1`; refused with *"The CRSV range 1.0:25.0 does not include the default axis value (0.5)"* — a range the user never typed |
| `wght=3e2:7e2` | `re.search` scans to `2:7` inside it and refuses against that |
| `wght=inf` | accepted; `float('inf')` clamps to the axis maximum |
| `wght=nan` | refused, but with the debug print *"Ooops… v=nan, triple=(300.0, 300.0, 1000.0)"* |
| `wght=4_00` | accepted as 400 — Python literal syntax leaking into a text field |
| `abc 300:700 xyz` | accepted; the surrounding text is ignored |
| `300:700:900` | accepted as `300:700` |
| `300:700[500]` | accepted, and the `[500]` silently dropped |
| a pin outside the axis extent | accepted and clamped |

The first row is the serious one: a user asking to keep the whole weight range receives a
font with no weight axis and is told nothing.

**Verdict: defect.** The tests stand.

Two further cases in this area, `slice.whitespace-only-cell-counts-as-blank` and
`slice.only-whitespace-cells-is-refused-as-blank`, are a smaller matter: a cell holding two
spaces reaches `float("  ")` and the job is refused with a message quoting whitespace back
at the user. A cell containing spaces is indistinguishable on screen from an empty one, so
the message describes a problem nobody can see. Both cases are labelled `judgement` and say
so.

### E4 — loading a font clears the bit-flag checkboxes (15 cases)

`MainWindow.load_font` clears all six bit checkboxes without reading the font's current
values, and the worker then writes whatever the boxes say. So opening a font and slicing it
without touching that panel **clears every exposed bit**.

Measured, with `tests/suite/probe-original.py --enrich --both roundtrip --axis wght=400`:

```
               usWeightClass       fsSelection          macStyle
input                    300  0000000011000000  0000000000000000
original                 400  0000000010000000  0000000000000000
ours                     400  0000000011000000  0000000000000000
```

Bit 6 (`USE_TYPO_METRICS`) is cleared. Bit 7 (`WWS`) survives — and that asymmetry is the
proof of the mechanism: bit 7 is not one of the six the editor exposes, so nothing
overwrites it. A font that declared `USE_TYPO_METRICS` comes out of a no-op slice without
it, which changes line spacing in every application that honours the flag.

**Verdict: defect.** The tests stand.

### D5 — the four typographic and WWS name records are deleted (8 cases)

`FontNameModel.load_font` fills rows for nameIDs 1, 2, 3, 4 and 6 only. Rows 16, 17, 21 and
22 stay empty, and `edit_name_table` deletes optional records whose row is empty. So a font
carrying typographic or WWS family names loses all four on any slice.

Measured, same probe run as above (the `--enrich` flag adds the four records first, because
the stock fixture has none and their loss is otherwise invisible):

```
                 name 16       name 17       name 21       name 22
input            present       present       present       present
original            GONE          GONE          GONE          GONE
ours             present       present       present       present
```

nameID 16/17 are how a family with more than four styles tells an operating system which
styles belong together. Losing them collapses a large family into a pile of unrelated
four-member groups in every font menu.

**Verdict: defect.** The tests stand.

### H1 — the output container is inherited from the input, not from the extension (12 cases)

`TTFont.save` never looks at the filename. It passes `self.flavor` to `SFNTWriter`
(`ttFont.py:411`), and `self.flavor` was copied from the reader when the input was opened
(`ttFont.py:318`). The extension the user types is not consulted in either direction:

* a `.ttf` input saved as `o.woff` produces a bare sfnt with a `.woff` name — verified with
  fontTools 4.62.1, flavor `None`;
* a `.woff` input saved as `o.ttf` produces a WOFF with a `.ttf` name — verified, flavor
  `woff`, 1628 bytes.

`tests/suite/probes/probe-output-container.py` runs the whole matrix:

```
Recursive-VF.subset.ttf    -> .woff    result flavor: None    size 2048
Recursive-VF.subset.woff   -> .ttf     result flavor: woff    size 1628
Recursive-VF.subset.woff2  -> .ttf     result flavor: woff2   size 1052
Recursive-VF.subset.ttf    -> .woff2   result flavor: None    size 2048
```

A desktop font manager asked to install the second will reject it; a web server handed the
first will serve an uncompressed font with a `.woff` content type. This is the claim the
behaviour map originally recorded backwards — H1 was written as "the extension decides" from
reading the UI, and the measurement corrected it.

**Verdict: defect.** The tests stand.

## Part 3 — what the original does that the corpus does not count against it

17 cases need overlap removal. The original passes no `overlap` argument to
`instantiateVariableFont` and never claimed to; `run.py` reports these separately as
"needs a feature it does not have" rather than as failures. Adding the feature is the
reason this reimplementation exists.

## Re-running any of this

```
tests/suite/run.py                      # both programs, whole corpus
tests/suite/run.py --runner original --verbose
tests/suite/probe-original.py axis wght -- 400 inf nan 1e3 '300:700[500]'
tests/suite/probe-original.py --enrich --both roundtrip --axis wght=400
```

`tests/suite/README.md` describes the corpus layout; `docs/test-suite.md` describes every
case in plain English; `docs/original-behaviour.md` is the numbered behaviour map the
`covers` fields point at.
