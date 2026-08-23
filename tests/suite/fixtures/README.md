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

### `two-axis` — 1804 bytes

The plain variable font: `wght` 100/400/900 and `wdth` 75/100/125, five masters (default
plus each axis extreme), glyphs `.notdef H O period`, simple contours only, no composites,
no layout tables. Both axes carry an `avar` segment map — `wght` is deliberately non-linear
(400 → 800 at user 700), `wdth` is the identity — so that renormalization of `avar` onto a
restricted range has something to get wrong. This is the default fixture for anything that
is about axes rather than about outlines.

### `no-avar` — 1736 bytes

Identical to `two-axis` in every way except that the design space declares no axis maps, so
no `avar` is produced. `build.py` asserts `avar` is absent here and present in `two-axis`.
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

### `composites` — 1484 bytes

Composite glyphs, one nested two deep. Glyphs: `square` and `dot` are simple; `squaredot`
is a composite of `square` + `dot`; `double` is a composite of `squaredot` + `dot`, so it
is a composite referencing a composite. One axis, `wght` 400/400/900. The two masters
differ in the *component offsets*, not only in the base outlines, so `gvar` carries deltas
on component offsets — a composite's `gvar` tuple has one delta per component plus four
phantom points, and `build.py` asserts exactly that shape and that at least one component
offset actually varies. Composites are also where `OVERLAP_COMPOUND` rather than
`OVERLAP_SIMPLE` has to be set (claim G12).

### `named-instances` — 3400 bytes

The same two axes and masters as `two-axis` (without the `avar` maps), plus eight `fvar`
named instances — Thin, Light, Regular, Bold, Black, Condensed Regular, Condensed Bold,
Expanded Regular — and a `STAT` table with axis values on **every** axis: five for `wght`,
three for `wdth`, with Regular and Normal flagged elidable. `build.py` asserts the instance
count and that `STAT`'s axis values cover both axes. This is the fixture for instance
dropping (G4), `STAT` value pruning (G5), and name-record pruning (G10) — the named
instances and `STAT` values between them own a good number of nameIDs above 255, which is
what `no_dangling_name_ids` is looking at.

### `overlapping` — 1644 bytes

The fixture for overlap removal, and the one whose geometry is checked most carefully.
One axis, `wght` 400/400/900; every boundary moves outward and every counter inward as
weight increases, so the topology is the same at both ends of the axis. Four glyphs:

* **`plus`** (U+002B) — two crossing rectangles, a horizontal bar and a vertical bar, both
  wound the same way. The region where they cross has winding ±2. Under the non-zero rule
  it is filled; under even-odd it would be a hole. This is the glyph that tells you which
  fill rule an implementation is actually using.
* **`ring`** (U+006F) — an outer square with a square counter. An 'o'. No overlap; the
  control for "overlap removal must not eat counters".
* **`circled`** (U+0040) — nesting depth 3: an outer square, its counter, a filled square
  inside that counter, and that square's own counter. Four contours, alternating direction,
  producing filled / empty / filled / empty from the outside in.
* **`bowtie`** (U+0078) — one contour that crosses itself, at (350, 350). Two lobes of
  opposite winding, both filled under non-zero. A single self-intersecting contour is the
  case an overlap remover that only ever compares *pairs* of contours will miss.

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
   what the glyph is supposed to look like.

The winding number is the standard signed crossing count: an upward edge passing to the
left of the point counts +1, a downward edge passing to the left counts -1. A point is
inside under the non-zero rule — the rule `glyf` outlines are filled with — exactly when
the total is not zero.

Confirmed, identically at `wght=400` and `wght=900`:

| glyph | contour directions | probe | winding | |
|---|---|---|---|---|
| `plus` | CW, CW | (400, 350) crossing | **-2** | filled |
| | | (150, 350) horizontal bar | -1 | filled |
| | | (400, 620) vertical bar | -1 | filled |
| | | (150, 620), (900, 350) | 0 | empty |
| `ring` | CW, CCW | (100, 350) band | -1 | filled |
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

Two things to read off that table. The outer contours are clockwise and the counters
counter-clockwise, which is the TrueType convention (`cff2-vf` is the other way round, per
PostScript). And `plus` has winding **-2** in the crossing region while `bowtie` has +1 and
-1 in its two lobes: an overlap remover must turn both of those into the same filled area
they already describe, which is what `filled_region_matches` checks, while
`no_self_intersections` must go from failing to passing.
