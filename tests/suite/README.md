# The conformance corpus

A set of declarative test cases describing what a font-slicing tool should do, written so
that the **same case runs against both implementations**: the original PyQt5 Slice and
this one. Neither is the reference; the cases are.

That is the point of the format. A test written in Python would be easy to write in a way
that flatters the Python program, and a test written in Rust likewise. A case here is
data — a fixture, some editor input, and a list of expectations — and a single shared
checker evaluates the result. Neither program marks its own homework.

```
cases/*.json          the corpus, one file per area
fixtures/             generator script and the fonts it produces
runners/original.py   drives slice.models + InstanceWorker from the original
runners/ours.py       drives slice-cli
checker.py            evaluates expectations against an output font, using fontTools
run.py                runs corpus x runner, scores, tabulates
```

Every case names the behavioural claims it covers, by ID, from
[`docs/original-behaviour.md`](../../docs/original-behaviour.md). Coverage is measured
as: every claim has at least one case.

## A case

```json
{
  "id": "axis.pin-all-yields-static",
  "title": "Pinning every axis produces a static font",
  "covers": ["B2", "B3", "G1"],
  "class": "slice-ui",
  "rationale": "A font with no axes left has nothing for fvar to describe, and the OpenType specification requires fvar only for variable fonts. Leaving an fvar with zero axes behind would be malformed, so pinning everything must remove it along with the rest of the variation data that indexes into it.",
  "fixture": "recursive-vf",
  "input": {
    "axes": {"MONO": "0", "CASL": "1", "wght": "1000", "slnt": "0", "CRSV": "0.5"}
  },
  "expect": {
    "outcome": "success",
    "checks": [
      {"kind": "no_table", "table": "fvar"},
      {"kind": "no_table", "table": "gvar"},
      {"kind": "glyph_count", "equals": 3}
    ]
  }
}
```

### Required fields

| field | meaning |
|---|---|
| `id` | unique, dotted, stable. Renaming one loses its history. |
| `title` | one line, what the case establishes |
| `covers` | claim IDs from the behaviour map |
| `class` | `spec`, `fonttools`, `slice-ui`, `judgement` — where the authority comes from |
| `rationale` | **why this is correct behaviour.** See below. |
| `fixture` | a name from the roster |
| `input` | what the user does |
| `expect` | what must come out |

### Writing a rationale

A rationale is a short paragraph — two to five sentences — answering *why is this the
right answer*, not *what does the test do*. It is read by someone deciding whether a
failing test is a bug in the program or a bug in the test, so it has to carry enough to
settle that.

Cite the authority. "The OpenType `OS/2` specification states that bit 6 is mutually
exclusive with bits 0 and 5" is a rationale. "Checks that bit 6 is cleared" is not.

Where the case encodes behaviour the original gets **wrong**, say so and say why, so the
expected failure is not mistaken for a broken test.

### `input`

All fields optional; omitted means "the user left it alone".

```json
{
  "axes":    {"wght": "400", "wdth": "75:100"},   // exactly what is typed into each cell
  "names":   {"1": "Family", "16": ""},            // nameID -> cell contents; "" means cleared
  "bits":    {"fsSelection": {"6": true}, "macStyle": {"0": false}},
  "remove_overlaps": false,
  "format":  "ttf"                                 // ttf | otf | woff | woff2
}
```

`axes` values are **strings**, because the thing under test is what happens to the
characters the user typed. `"400"`, `" 400 "`, `"4e2"` and `"400.0"` are different inputs
and may legitimately behave differently.

Any axis absent from `axes` is left blank, i.e. keeps its whole range.

### `expect`

```json
{"outcome": "success", "checks": [ ... ]}
{"outcome": "error", "message_contains": ["wght", "not a valid"]}
```

`error` means the tool must refuse the job and say why. `message_contains` is a list of
substrings that must all appear, matched case-insensitively — enough to show the message
names the problem and the axis, without pinning exact wording that the two programs have
no reason to share.

An `error` expectation is **not** satisfied by a crash, by a written-out font, or by a
message that does not contain the substrings.

## Check kinds

The checker evaluates these against the output font with fontTools. Any check can take
`"not": true` to invert it.

### Structure

| kind | fields | passes when |
|---|---|---|
| `parses_as_font` | | fontTools can open the output |
| `has_table` | `table` | the table is present |
| `no_table` | `table` | the table is absent |
| `glyph_count` | `equals` | `maxp.numGlyphs` matches |
| `sfnt_flavor` | `equals` — `truetype`\|`cff`\|`woff`\|`woff2` | the container and outline type match |

### Axes

| kind | fields | passes when |
|---|---|---|
| `axis_count` | `equals` | `fvar` has this many axes |
| `axis_tags` | `equals` (ordered list) | `fvar` axis tags in order |
| `axis_extent` | `tag`, `min`, `default`, `max` | that axis's `fvar` record matches |
| `named_instance_count` | `equals` | `fvar` instance count |
| `named_instances_within_extent` | | every instance sits inside every axis's declared range |

### Tables and fields

| kind | fields | passes when |
|---|---|---|
| `os2_field` | `field`, `equals` | e.g. `usWeightClass`, `usWidthClass`, `xAvgCharWidth`, `fsSelection` |
| `head_field` | `field`, `equals` | e.g. `macStyle`, `unitsPerEm`, `indexToLocFormat` |
| `hhea_field` | `field`, `equals` | e.g. `advanceWidthMax`, `numberOfHMetrics` |
| `post_field` | `field`, `equals` | e.g. `italicAngle`, `underlinePosition` |
| `maxp_field` | `field`, `equals` | e.g. `maxPoints`, `maxContours` |
| `bit_set` | `field` (`fsSelection`\|`macStyle`), `bit` | that bit is 1 |
| `bit_clear` | `field`, `bit` | that bit is 0 |
| `head_bbox_matches_outlines` | | `head`'s bounding box equals the union of the glyph bounds |
| `maxp_covers_outlines` | | `maxp`'s point and contour maxima are at least what `glyf` contains |
| `hhea_matches_hmtx` | | `hhea`'s extremes agree with `hmtx` and `glyf` |

### Names

| kind | fields | passes when |
|---|---|---|
| `name_record` | `id`, `equals` | the 3/1/1033 record has that string |
| `name_absent` | `id` | no 3/1/1033 record with that ID |
| `name_present` | `id` | there is one |
| `name_ids_subset_of` | `ids` | the font has no name IDs outside this set |
| `no_dangling_name_ids` | | every name ID above 255 is referenced by `fvar` or `STAT` |

### Outlines

| kind | fields | passes when |
|---|---|---|
| `outlines_match_source_at` | `location` (tag→value), `tolerance` (default 1.0) | every glyph drawn from the output at its default matches the **source fixture** drawn at `location` |
| `outlines_match_source_across` | `locations`, `tolerance` | as above, sampled at several locations, for a still-variable output |
| `advances_match_source_at` | `location`, `tolerance` | same, for advance widths |
| `filled_region_matches` | `reference` (`source_at` + location), `tolerance_units` | the set of points inside each glyph is unchanged — the check for overlap removal, where the outline is meant to change but the shape is not |
| `no_self_intersections` | | no glyph has a contour crossing itself or another |
| `all_coordinates_finite` | | no coordinate is NaN or infinite |
| `contour_count` | `glyph`, `equals` | that glyph has this many contours |

### Layout

| kind | fields | passes when |
|---|---|---|
| `feature_lookup_count` | `table`, `feature`, `equals`\|`min` | how many lookups that feature runs |
| `no_feature_variations` | `table` | the table has no `FeatureVariations` |
| `feature_variation_axes_valid` | `table` | every condition's axis index exists in the output `fvar` |
| `substitutes` | `table`, `feature`, `from`, `to` | that feature's lookups map one glyph to another |

## Fixtures

Named in the roster below and built by `fixtures/build.py`, which is committed. The fonts
are generated rather than collected so that each one isolates a property, and so that a
reader can see exactly what is in them.

| name | what it is for |
|---|---|
| `recursive-vf` | the real thing: 5 axes, `avar`, `GSUB` feature variations, no composites. Copied from the original repository. |
| `recursive-vf-woff` / `-woff2` | the same font in each container, for input handling |
| `two-axis` | minimal `wght`+`wdth` variable font, simple contours only |
| `composites` | includes composite glyphs, one nested two deep, with `gvar` deltas on component offsets |
| `centred-default` | one axis, `100 / 400 / 900`, so the default is off-centre and the two halves renormalize differently |
| `named-instances` | several `fvar` named instances and a `STAT` with values on every axis |
| `overlapping` | deliberate overlaps: two crossing bars, a counter, a counter inside a counter, and a self-intersecting contour |
| `hinted` | `prep`, `fpgm`, `cvt ` and per-glyph instructions |
| `gdef-varstore` | variable kerning: `GDEF` item variation store referenced from `GPOS` |
| `cff2-vf` | CFF2 outlines, to pin down what happens to a font neither program claims to support |
| `no-avar` | like `two-axis` but without `avar`, so the mapping is pure |
| `single-axis-min-default` | default equal to the axis minimum, the shape Recursive's `wght` has |

## Running it

```sh
tests/suite/run.py                     # both runners, full corpus
tests/suite/run.py --runner ours       # one implementation
tests/suite/run.py --case axis.        # id prefix filter
tests/suite/run.py --verbose           # per-check detail
tests/suite/run.py --json report.json  # machine-readable
```

Exit status is 0 when every case that is expected to pass, passes.

## The rule this corpus is under

A case that the original passes, this implementation must also pass. A case that the
original fails is adjudicated on evidence before anything is concluded from it: the
verdict is either *the test is wrong* (fix the test), *the original is wrong* (this
implementation must be right), or *a deliberate divergence* (documented, in
`docs/original-behaviour.md`). Nothing is resolved by changing an expectation to match
whatever a program happens to do.
