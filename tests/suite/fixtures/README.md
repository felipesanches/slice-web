# Fixtures

The fonts the conformance corpus runs against. All of them are produced by
[`build.py`](build.py) into [`out/`](out/), and all of them are committed — the whole set
is under 50 KB, and having the bytes in the repository means a test failure can be
reproduced without a working fontTools.

```sh
tests/suite/fixtures/build.py            # build, verify, write out/
tests/suite/fixtures/build.py --check    # build and verify, write nothing
```

A case names a fixture by the `name` column below; the file is `out/<name>.<ext>`, where
the extension is whatever container the fixture is testing (`.ttf`, `.woff`, `.woff2`,
`.otf`).

Everything except `recursive-vf` is generated. Each one is a handful of glyphs at
1000 upem, drawn from rectangles, and exists to isolate exactly one property. None of
them is trying to look like a typeface, and no case should assert anything about how they
look beyond the property they are for.

## Reproducibility

The output is byte-for-byte reproducible: `head.created` and `head.modified` are pinned to
2000-01-01, every `TTFont` is opened with `recalcTimestamp=False` (fontTools otherwise
stamps `head.modified` with the wall clock on save), and nothing random or
insertion-ordered goes into the fonts. `build.py` builds the whole roster twice in one
process and refuses to write if the two runs differ. Verified reproducible across
processes under `PYTHONHASHSEED` 1, 12345 and 99999 as well.

## What is verified before anything is written

`build.py` will not write a font that has not passed all of this:

* it opens in fontTools, and survives a `TTFont(...).save()` round trip with every table
  force-decompiled afterwards;
* `head`, `hhea`, `hmtx`, `maxp`, `cmap`, `name`, `OS/2`, `post` and an outline table
  (`glyf` + `loca`, or `CFF2`) are all present;
* every glyph has a strictly positive advance width;
* the container is the one intended (`font.flavor`);
* if it has `fvar`: `varLib.instancer.instantiateVariableFont` pins it at the default and
  at each axis's minimum and maximum, `fvar` is gone from each result, and each result
  saves and reopens.

On top of that, several fixtures assert the specific thing they exist for — see the
paragraphs below. `overlapping` gets a full winding computation, described at the end.

---

## The roster

### `recursive-vf` — `out/recursive-vf.ttf`, 18288 bytes

The real font: Recursive, subset to `.notdef a a.italic`, copied **verbatim** from
`testdata/fonts/Recursive-VF.subset.ttf` (which in turn came from the original Slice
repository). Five axes — `MONO` 0/0/1, `CASL` 0/0/1, `wght` 300/300/1000, `slnt` -15/0/0,
`CRSV` 0/0.5/1 — with `avar`, `GSUB` feature variations for `rvrn`, `HVAR`, `MVAR`, `STAT`,
64 named instances and TrueType `prep`. It is the one fixture that is not simplified, so
it is the one that catches interactions the isolated fixtures cannot. No composites.

### `recursive-vf-woff` / `recursive-vf-woff2` — `.woff` 7816 bytes, `.woff2` 6340 bytes

The same font in the two compressed containers, for the input-handling path. They are
re-containered from the exact bytes of `recursive-vf.ttf` rather than collected separately,
and `build.py` asserts that every table in the container is byte-identical to the source
`.ttf` — the only exceptions allowed being `head.checkSumAdjustment`, which is a checksum
over the whole file, and `head` flags bit 11, which the WOFF2 writer sets. So any
behavioural difference between these three fixtures is a difference in container handling
and nothing else.

### `two-axis` — 1824 bytes

The plain variable font: `wght` 100/400/900 and `wdth` **50/100/200**, five masters
(default plus each axis extreme), glyphs `.notdef H O period`, simple contours only, no
composites, no layout tables. Both axes carry an `avar` segment map — `wght` is
deliberately non-linear (400 → 800 at user 700), `wdth` is the identity — so that
renormalization of `avar` onto a restricted range has something to get wrong. This is the
default fixture for anything that is about axes rather than about outlines.

`wdth` spans the whole registered range on purpose, and `build.py` asserts the extents are
exactly 50/100/200. `OS/2.usWidthClass` is derived from a pinned `wdth` through a
nine-node piecewise-linear map defined over exactly that interval, and fontTools clamps
the pin into the *axis* extent before consulting the map — so on the earlier 75/100/125
axis a case pinning `wdth=175` silently measured 125 and got the answer for a value it
never asked about. The six `static.uswidthclass.*` cases pin 50, 70, 100, 120 and 175;
three of them were answering the wrong question until the axis was widened. The two width
masters moved with the axis, to 0.62× at `wdth` 50 and 1.60× at `wdth` 200.

### `no-avar` — 1756 bytes

Identical to `two-axis` in every way — including the 50/100/200 `wdth` — except that the
design space declares no axis maps, so no `avar` is produced. `build.py` asserts `avar` is absent here and present in `two-axis`.
Pairing the two isolates *"is this behaviour the `avar` renormalization, or the normalization
itself?"* — with no `avar`, user-space to normalized is the pure piecewise-linear mapping
from the `fvar` extents.

### `centred-default` — 1520 bytes

One axis, `wght` 100/400/900. The default is off-centre, so the two halves of the axis
normalize with different scale factors: user 100..400 maps to -1..0 over 300 units, user
400..900 maps to 0..+1 over 500. Three masters. A slicer that renormalizes with a single
scale factor, or that assumes the default is the midpoint, produces visibly wrong deltas
here and nowhere else.

### `single-axis-min-default` — 1520 bytes

One axis, `wght` 300/300/1000 — default **equal to the minimum**, the shape Recursive's
`wght` axis has. Two masters. The whole axis normalizes to 0..+1 with no negative side at
all, so any code path that assumes a two-sided axis, or that divides by
`default - minimum`, fails here. Also the case where restricting the range from below is
a no-op and restricting from above must keep the default where it is (see claim B7).

### `composites` — 1572 bytes

Composite glyphs, one nested two deep. Glyphs: `square` and `dot` are simple; `squaredot`
is a composite of `square` + `dot`; `double` is a composite of `squaredot` + `dot`, so it
is a composite referencing a composite. One axis, `wght` 400/400/900. The two masters
differ in the *component offsets*, not only in the base outlines, so `gvar` carries deltas
on component offsets — a composite's `gvar` tuple has one delta per component plus four
phantom points, and `build.py` asserts exactly that shape and that at least one component
offset actually varies. Composites are also where `OVERLAP_COMPOUND` rather than
`OVERLAP_SIMPLE` has to be set (claim G12).

Two further composites exist for the *overlap* half of the question, because removing
overlaps has to treat them oppositely:

* **`comp.overlap`** — two `square` components, the second offset by (180, 160) so the
  pair genuinely shares area. A composite's overlap lives *between* its components and
  cannot be written as component references, so this glyph must be decomposed and merged.
* **`comp.plain`** — two `dot` components at x = 0 and x = 300. `dot` is 120 units square
  and does not vary, so the 180-unit gap is there at every location on the axis. There is
  nothing to merge, so decomposing it would only cost `glyf` size, and it must come out
  still a composite.

Neither one's component offsets vary, so the overlap and the gap are properties of the
whole axis rather than of one location. `build.py` asserts exactly that: it takes each
component's bounding box at the default *and* at `wght` 900 — every component here is a
filled rectangle, so a box intersection is an area intersection — and requires
`comp.overlap` to intersect at both ends and `comp.plain` at neither, refusing to write
the font if a glyph overlaps at one end of the axis and not the other.

> **Note.** The two cases these glyphs exist for —
> `overlap.overlapping-components-are-merged` and
> `overlap.non-overlapping-composite-is-not-decomposed` — still fail, and not on the
> fixture. Both carry a `filled_region_matches` check, and `checker.py`'s `_flatten`
> ignores `addComponent`, so a composite flattens to *no* contours. A glyph that is a
> composite in the source and a simple glyph in the output therefore reads as "empty
> became filled" at every sampled point. Measured: swapping `_draw`'s `RecordingPen` for a
> `DecomposingRecordingPen` turns the same check on the same two fonts from
> `1125 sampled points changed fill state` into `10626 sampled points agree`. That is a
> one-line fix in the checker, and it is the only thing standing between these two cases
> and passing. The same blind spot is why `outlines_match_source_at` raises
> `TypeError: type str doesn't define __round__ method` on this fixture: `_round_ops` is
> handed the component's *glyph name*.

### `named-instances` — 3892 bytes

The same two axis tags and the same masters as `two-axis`, without the `avar` maps and
with the narrower `wdth` 75/100/125 that `two-axis` used to have — nothing here is about
the width mapping, and keeping the older extents means the eight instances below sit at
axis values the fixture actually has masters for. Plus eight `fvar`
named instances — Thin, Light, Regular, Bold, Black, Condensed Regular, Condensed Bold,
Expanded Regular — and a `STAT` table with axis values on **every** axis: five for `wght`,
three for `wdth`, with Regular and Normal flagged elidable. `build.py` asserts the instance
count and that `STAT`'s axis values cover both axes. This is the fixture for instance
dropping (G4), `STAT` value pruning (G5), and name-record pruning (G10) — the named
instances and `STAT` values between them own a good number of nameIDs above 255, which is
what `no_dangling_name_ids` is looking at.

It is also the only fixture with a name table that is more than the six mandatory records
on two platforms, because the name-editing claims need records that the roster did not
otherwise have anywhere. Three additions, all of them checked by
`check_named_instance_names` before the font is written:

**The four optional family names, at 3/1/1033.** ID 16 `Fixture Family`, 17
`Named Instances`, 21 `Fixture WWS Family`, 22 `Named Instances Regular`. No fixture had
*any* of these, which left D5 — the corpus's largest suspected defect, that all four are
stripped from every font the original touches — with nothing to be tested on at all. With
eight named instances this font is an extended typographic family in the specification's
sense, which is exactly the case where IDs 16 and 17 are recommended, so it is the right
home for them. The strings are deliberately unlike IDs 1 and 2 so a case can tell which
record an implementation actually read. None of the four is mirrored onto the Macintosh
platform, and `build.py` asserts that.

**A German localisation, at 3/1/1031.** ID 1 `FixtureNamedInstances Deutsch`, 2
`Standard`, 16 `Fixture Familie`, 17 `Überschrift`. Claim D2 is that only 3/1/1033 is read
and written, and the *only* half of that is untestable without a record on some other key
to watch survive untouched. Two of the strings are non-ASCII, so the fixture also carries
a UTF-16BE round trip through a record the user never typed; `build.py` asserts the stored
encoding is `utf_16_be` and that the per-ID record counts are 3, 3, 2, 2, 1, 1 for IDs 1,
2, 16, 17, 21, 22 — the numbers the cases assert against, so a case failing on a count is
a failure of the tool and not a drift in the fixture.

**A GSUB `ss01` whose `FeatureParams.UINameID` is 276**, with a 3/1/1033 record 276
`Alternate H` and a single substitution H → O to hang the feature on. G10's pruning rule
is subtler than "drop what is unused": only IDs above 255 that `fvar` or `STAT` referenced
*before* the slice and no longer do are candidates, so a label referenced from a layout
feature must survive even though no variation table ever pointed at it. `recursive-vf`'s
only GSUB feature is `rvrn`, whose `FeatureParams` is `None`, so nothing in the roster
exercised this. The ID is written out explicitly rather than left to feaLib to allocate,
so that it is a stable number a case can name; `build.py` asserts 276 was free before it
is claimed, and that the feature has at least one lookup. The name records are sorted into
(platform, encoding, language, nameID) order in memory as well as on compile, so the
`name_records_sorted` check is about the fixture rather than about when fontTools tidies
up.

### `overlapping` — 1720 bytes

The fixture for overlap removal, and the one whose geometry is checked most carefully.
One axis, `wght` 400/400/900; every boundary moves outward and every counter inward as
weight increases, so the topology is the same at both ends of the axis. Five glyphs:

* **`bars`** (U+002B) — two crossing rectangles, a horizontal bar and a vertical bar, both
  wound the same way. The region where they cross has winding ±2. Under the non-zero rule
  it is filled; under even-odd it would be a hole. This is the glyph that tells you which
  fill rule an implementation is actually using.
* **`o`** (U+006F) — an outer square with a square counter. No overlap; the control for
  "overlap removal must not eat counters".
* **`circled`** (U+0040) — nesting depth 3: an outer square, its counter, a filled square
  inside that counter, and that square's own counter. Four contours, alternating direction,
  producing filled / empty / filled / empty from the outside in.
* **`bowtie`** (U+0078) — one contour that crosses itself, at (350, 350). Two lobes of
  opposite winding, both filled under non-zero. A single self-intersecting contour is the
  case an overlap remover that only ever compares *pairs* of contours will miss.
* **`clean`** (U+0063) — one clockwise triangle, apex at (400, 650). The negative control.
  It has no second contour to overlap and three straight edges that cannot cross each
  other, so there is nothing for an overlap remover to do and it must hand the glyph back
  untouched — fontTools takes that position explicitly, replacing a glyph only
  `if not _same_path(path, path2)` in `ttLib/removeOverlaps.py`. Passing a clean glyph
  through a boolean engine renumbers its points and can refit exact curves through rounded
  intersections that were never needed, so "did nothing" is a real property with a cost
  attached, and until this glyph existed the corpus had no glyph on which to state it.

`bars` and `o` were called `plus` and `ring` until the cases were written; they were
renamed rather than duplicated, having confirmed by grep that no case, document or crate
referred to the old names. Nothing else about them changed.

> **Note.** `overlap.clean-glyph-is-left-alone` cannot pass as written, and no fixture can
> make it. It turns overlap removal *on* and then asserts `outlines_match_source_at` over
> the whole font, which requires every glyph's outline to be unchanged — but the same
> fixture's `bars` must become one contour, which is what the neighbouring case
> `overlap.two-bars-merge-into-one-contour` asserts. The two expectations contradict each
> other on the same font. The case's own rationale describes the right check ("byte-identical
> to what the same job produces with the feature off"), which is the proposed
> `matches_case_output` kind against the control case, not a comparison with the variable
> source. With `clean` in place the case now fails on one check instead of two, and it will
> pass once the check is rewritten the way its rationale already describes.

### `hinted` — 1452 bytes

TrueType hinting: `fpgm` (one `FDEF`, function 0, which does an `MDAP[1]` on the point
number handed to it), `prep` (`SCANCTRL` / `SCANTYPE`), `cvt ` (four values: baseline, cap
height, descender, stem width), and per-glyph instructions on `H` and `I` that `CALL`
function 0. `maxp`'s hinting fields — `maxZones`, `maxTwilightPoints`, `maxStorage`,
`maxFunctionDefs`, `maxInstructionDefs`, `maxStackElements` — are set explicitly, because
fontTools does not recalculate those on compile. One axis, `wght` 400/400/900, no `cvar`.
`build.py` asserts all four pieces survived the varLib merge. The question this fixture
asks is whether the hinting tables and the glyph programs come out the far side of a slice
intact.

### `gdef-varstore` — 1692 bytes

Variable kerning. Glyphs `A V T`; a `kern` feature with three pairs (`A V`, `V A`, `T A`),
built with feaLib into each of two masters with different values, then merged by varLib.
The result is `GPOS` pair positioning whose value records carry `Device` tables of format
`VariationIndex` (0x8000) pointing into an item variation store in `GDEF`. `build.py`
asserts both halves of that: `GDEF.VarStore` exists, and at least one `GPOS` value record
references it. This is the fixture for "the variation data a slicer has to follow is not
all in `gvar`" — pinning an axis has to resolve those deltas into the value records and
drop the store.

### `cff2-vf` — 1280 bytes

CFF2 outlines rather than `glyf`: glyphs `.notdef H I period`, one axis `wght` 400/400/900,
built from two CFF masters that varLib converts to a single CFF2 with a blend. Contours
follow the PostScript convention (outer counter-clockwise), which is the opposite of the
TrueType fixtures — worth knowing before comparing winding numbers across fixtures. The
roster entry exists to pin down what happens to a font *neither* program claims to support:
whatever the answer is, both must give the same one, and it must not be a crash or a
corrupt output.

### `static-ttf` — `out/static-ttf.ttf`, 2068 bytes

A TrueType font with **no `fvar`**: `testdata/fonts/Recursive-Sliced.subset.ttf`, copied
verbatim. It is an instance of the same family `recursive-vf` comes from — three glyphs,
`GDEF`/`GPOS`/`GSUB`, `prep`, no variation tables of any kind — so a case that hands it to
a slicer is handing over a real static font rather than a stripped-down imitation of one.
Every other fixture in the roster is variable, so before this there was nothing to test
"refuse a font with nothing to slice" against. Copied rather than generated for the same
reason `recursive-vf` is: the thing being tested is what happens to a font somebody
actually shipped. `build.py` asserts `fvar`, `gvar`, `avar`, `HVAR`, `MVAR` and `STAT` are
all absent and that it has `glyf` outlines.

### `with-dsig` — `out/with-dsig.ttf`, 1860 bytes

`two-axis` again — same axes, same masters, same glyphs, family `FixtureWithDsig` — plus a
stub `DSIG`. `DSIG` is a signature over the whole font file, so instancing rewrites `glyf`,
`hmtx`, `head`, `maxp`, `OS/2` and the table directory out from under it and the signature
must be dropped rather than carried over; an invalid signature is worse than none, because
Windows reads a broken `DSIG` as a tampered font.

Nothing in the roster had the table, which made the deletion claim vacuous — the check was
passing on a font that never had a `DSIG` to delete. It is a **new** fixture rather than a
`DSIG` bolted onto `recursive-vf` deliberately: `recursive-vf` is copied verbatim and
`build.py` asserts its WOFF and WOFF2 siblings are byte-identical to it table by table,
and some 170 cases compare their output against it, so changing it to make one check
non-vacuous is a poor trade. The stub is the empty, unsigned form (`usNumSigs` 0) that
font tools routinely emit, which is all that is needed: the property under test is that
the table is gone, not what was in it. `build.py` asserts the table survives serialisation
of the fixture itself.

> **Note.** `static.dsig-deleted` still names `recursive-vf` as its fixture, so it is still
> vacuous. Pointing it at `with-dsig` is a one-word change to the case, which was outside
> what this pass was allowed to touch.

---

## The winding check on `overlapping`

Overlap is the thing this project exists to add (claim G14), so the fixture that tests it
is not allowed to be approximately right. `build.py` proves the fill before it writes the
font, and prints what it proved.

The check reads the contours back out of the **compiled** `glyf` table — not out of the
Python data structures the glyphs were drawn from — after instancing the font at the axis
default *and* at the axis maximum. For each glyph it then:

1. computes the signed area of every contour (shoelace; positive is counter-clockwise in
   the y-up font frame) and compares the sign against the intended direction, and
2. evaluates the non-zero winding number at a probe point in every distinct region of the
   glyph, including the regions that must be *empty*, and compares the magnitude against
   what the glyph is supposed to look like, and
3. sweeps a 53 × 47 grid over the whole glyph — offset by half a step, with two sample
   counts that share no factor, so a sample landing exactly on an edge is unlikely and the
   strides never line up with an axis-aligned edge — and compares the **largest |winding|
   anywhere** against what the glyph is for. Step 2 says what the winding is in the regions
   whoever wrote the probe list thought of; step 3 says what it is everywhere, which is the
   only way to state the negative property `clean` exists for: *no* part of it is covered
   twice. Confirmed at both ends of the axis: `bars` reaches 2, and `o`, `circled`, `bowtie`
   and `clean` never exceed 1.

The winding number is the standard signed crossing count: an upward edge passing to the
left of the point counts +1, a downward edge passing to the left counts -1. A point is
inside under the non-zero rule — the rule `glyf` outlines are filled with — exactly when
the total is not zero.

Confirmed, identically at `wght=400` and `wght=900`:

| glyph | contour directions | probe | winding | |
|---|---|---|---|---|
| `bars` | CW, CW | (400, 350) crossing | **-2** | filled |
| | | (150, 350) horizontal bar | -1 | filled |
| | | (400, 620) vertical bar | -1 | filled |
| | | (150, 620), (900, 350) | 0 | empty |
| `o` | CW, CCW | (100, 350) band | -1 | filled |
| | | (400, 350) counter | 0 | empty |
| | | (900, 350) | 0 | empty |
| `circled` | CW, CCW, CW, CCW | (60, 350) outer band | -1 | filled |
| | | (150, 350) outer counter | 0 | empty |
| | | (240, 350) inner square | -1 | filled |
| | | (400, 350) counter in a counter | 0 | empty |
| | | (900, 350) | 0 | empty |
| `bowtie` | one self-intersecting contour, signed area 0 | (350, 500) upper lobe | +1 | filled |
| | | (350, 200) lower lobe | -1 | filled |
| | | (150, 350), (900, 350) | 0 | empty |
| `clean` | CW | (400, 100) low in the triangle | -1 | filled |
| | | (400, 400) high in the triangle | -1 | filled |
| | | (150, 500) left of the rising edge | 0 | empty |
| | | (650, 500) right of the falling edge | 0 | empty |
| | | (400, 720) above the apex | 0 | empty |
| | | (900, 350) | 0 | empty |

And the whole-glyph sweep, also identical at `wght=400` and `wght=900`:

| glyph | largest \|winding\| anywhere | reading |
|---|---|---|
| `bars` | **2** | two layers deep somewhere — there is an overlap to remove |
| `o` | 1 | no region is covered twice |
| `circled` | 1 | no region is covered twice, at any of the four nesting levels |
| `bowtie` | 1 | the two lobes are +1 and -1, never stacked |
| `clean` | 1 | nothing to remove — this is what makes it the control |

Three things to read off those tables. The outer contours are clockwise and the counters
counter-clockwise, which is the TrueType convention (`cff2-vf` is the other way round, per
PostScript). `bars` has winding **-2** in the crossing region while `bowtie` has +1 and
-1 in its two lobes: an overlap remover must turn both of those into the same filled area
they already describe, which is what `filled_region_matches` checks, while
`no_self_intersections` must go from failing to passing. And `clean` is the other side of
the same coin — its maximum is 1 everywhere, so a correct implementation must leave it
exactly as it found it.
