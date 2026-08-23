#!/usr/bin/env python3
"""Build the conformance-corpus font fixtures into ``tests/suite/fixtures/out/``.

Every font in the roster in ``tests/suite/README.md`` is produced here, from
source, with fontTools.  Nothing is collected from the wild except
``recursive-vf``, which is copied verbatim from ``testdata/fonts/``.

The fonts are deliberately tiny.  Each one isolates a single property that a
slicing tool has to get right; none of them is trying to be a typeface.  See
``README.md`` next to this file for what each fixture is for.

Determinism
-----------
The output is byte-for-byte reproducible.  ``head.created`` and
``head.modified`` are pinned to :data:`FIXED_TIMESTAMP` and every ``TTFont`` is
opened or created with ``recalcTimestamp=False``, because fontTools otherwise
stamps ``head.modified`` with the wall clock on save.  The script builds
everything twice in one process and refuses to write anything if the two runs
disagree.

Verification
------------
Nothing is written until it has passed :func:`verify_font`:

* it loads in fontTools and survives a ``TTFont(...).save()`` round trip;
* the tables a font needs in order to be openable are all present;
* every glyph has a positive advance width;
* if it has ``fvar``, ``varLib.instancer.instantiateVariableFont`` can pin it
  at the default, at every axis minimum and at every axis maximum, and each
  resulting static font saves.

The ``overlapping`` fixture gets a further check: the winding number of every
region of every glyph is computed from the compiled outlines and compared with
what the glyph is supposed to look like, at the default *and* at the extreme of
its axis.  See :func:`check_overlapping_windings`.

Usage
-----
    python3 tests/suite/fixtures/build.py            # build and verify
    python3 tests/suite/fixtures/build.py --check    # verify only, write nothing
"""

from __future__ import annotations

import argparse
import copy
import hashlib
import io
import sys
from array import array
from pathlib import Path

from fontTools.designspaceLib import (
    AxisDescriptor,
    DesignSpaceDocument,
    InstanceDescriptor,
    SourceDescriptor,
)
from fontTools.feaLib.builder import addOpenTypeFeaturesFromString
from fontTools.fontBuilder import FontBuilder
from fontTools.otlLib.builder import buildStatTable
from fontTools.pens.t2CharStringPen import T2CharStringPen
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.ttLib import TTFont, newTable
from fontTools.ttLib.tables.ttProgram import Program
from fontTools.varLib import build as varlib_build
from fontTools.varLib.instancer import instantiateVariableFont

HERE = Path(__file__).resolve().parent
OUT = HERE / "out"
REPO = HERE.parents[2]
RECURSIVE_TTF = REPO / "testdata" / "fonts" / "Recursive-VF.subset.ttf"

#: 2000-01-01T00:00:00Z expressed in the ``head`` epoch (seconds since
#: 1904-01-01).  96 years * 365 days + 24 leap days = 35064 days.
FIXED_TIMESTAMP = 35064 * 86400  # 3029529600

UPEM = 1000
ASCENT = 800
DESCENT = -200

#: Tables without which a font is not openable / not meaningfully a font.
REQUIRED_TABLES = {"head", "hhea", "hmtx", "maxp", "cmap", "name", "OS/2", "post"}


# --------------------------------------------------------------------------
# small geometry helpers
# --------------------------------------------------------------------------

def ccw_rect(x0, y0, x1, y1):
    """A rectangle wound counter-clockwise in a y-up frame: positive area.

    Bottom edge left to right, up the right side, back along the top.  This is
    the PostScript/CFF convention for an *outer* contour, and the TrueType
    convention for a *counter*.
    """
    return [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]


def cw_rect(x0, y0, x1, y1):
    """A rectangle wound clockwise: negative signed area.

    The TrueType convention for an *outer* contour, and the PostScript/CFF
    convention for a *counter*.
    """
    return list(reversed(ccw_rect(x0, y0, x1, y1)))


def draw_contours(pen, contours):
    for contour in contours:
        pen.moveTo(contour[0])
        for point in contour[1:]:
            pen.lineTo(point)
        pen.closePath()


def simple_glyph(contours, glyph_set=None):
    pen = TTGlyphPen(glyph_set)
    draw_contours(pen, contours)
    return pen.glyph()


def signed_area(contour):
    """Shoelace.  Positive = counter-clockwise in a y-up frame."""
    total = 0.0
    n = len(contour)
    for i in range(n):
        x0, y0 = contour[i]
        x1, y1 = contour[(i + 1) % n]
        total += x0 * y1 - x1 * y0
    return total / 2.0


def winding_number(point, contours):
    """Non-zero winding number of ``point`` with respect to ``contours``.

    The usual signed crossing count: an upward edge passing to the left of the
    point contributes +1, a downward edge passing to the left contributes -1.
    Counter-clockwise contours therefore give positive numbers.  A point is
    inside under the non-zero rule -- the rule ``glyf`` outlines are filled
    with -- exactly when this is not zero.
    """
    px, py = point
    total = 0
    for contour in contours:
        n = len(contour)
        for i in range(n):
            x0, y0 = contour[i]
            x1, y1 = contour[(i + 1) % n]
            # (x0,y0)->(x1,y1) cross (x0,y0)->(px,py): >0 means point is left
            side = (x1 - x0) * (py - y0) - (px - x0) * (y1 - y0)
            if y0 <= py:
                if y1 > py and side > 0:
                    total += 1
            else:
                if y1 <= py and side < 0:
                    total -= 1
    return total


def glyph_contours(font, glyph_name):
    """Contours of a *line-only* glyph, read back out of the compiled ``glyf``."""
    glyf = font["glyf"]
    glyph = glyf[glyph_name]
    coords, end_points, flags = glyph.getCoordinates(glyf)
    coords = list(coords)
    if any(not (f & 0x01) for f in flags):
        raise AssertionError(f"{glyph_name}: expected only on-curve points")
    contours = []
    start = 0
    for end in end_points:
        contours.append([tuple(p) for p in coords[start : end + 1]])
        start = end + 1
    return contours


# --------------------------------------------------------------------------
# master construction
# --------------------------------------------------------------------------

def new_master(
    glyph_order,
    cmap,
    advances,
    family,
    style="Regular",
    is_ttf=True,
):
    fb = FontBuilder(UPEM, isTTF=is_ttf)
    fb.setupGlyphOrder(list(glyph_order))
    fb.setupCharacterMap(cmap)
    fb.setupHorizontalMetrics({name: advances[name] for name in glyph_order})
    fb.setupHorizontalHeader(ascent=ASCENT, descent=DESCENT)
    ps_name = f"{family}-{style}".replace(" ", "")
    fb.setupNameTable(
        {
            "familyName": family,
            "styleName": style,
            "psName": ps_name,
            "version": "Version 1.000",
            "uniqueFontIdentifier": f"{family}:{style}:2000",
            "fullName": f"{family} {style}",
        }
    )
    fb.setupOS2(
        sTypoAscender=ASCENT,
        sTypoDescender=DESCENT,
        sTypoLineGap=0,
        usWinAscent=ASCENT,
        usWinDescent=-DESCENT,
        sxHeight=500,
        sCapHeight=700,
        achVendID="SLCE",
    )
    fb.setupPost(keepGlyphNames=True)
    fb.setupHead(
        unitsPerEm=UPEM,
        created=FIXED_TIMESTAMP,
        modified=FIXED_TIMESTAMP,
    )
    fb.font.recalcTimestamp = False
    return fb


def axis(tag, name, minimum, default, maximum, mapping=None):
    a = AxisDescriptor()
    a.tag = tag
    a.name = name
    a.minimum = minimum
    a.default = default
    a.maximum = maximum
    if mapping:
        a.map = list(mapping)
    return a


def source(font, location, name, is_default=False):
    s = SourceDescriptor()
    s.font = font
    s.location = dict(location)
    s.name = name
    s.familyName = "Fixture"
    s.styleName = name
    if is_default:
        s.copyInfo = True
    return s


def make_vf(axes, sources, instances=()):
    ds = DesignSpaceDocument()
    for a in axes:
        ds.addAxis(a)
    for s in sources:
        ds.addSource(s)
    for i in instances:
        ds.addInstance(i)
    vf, _, _ = varlib_build(ds, optimize=True)
    finalize(vf)
    return vf


def finalize(font):
    """Pin the timestamps.  Called on every font just before it is serialised."""
    font.recalcTimestamp = False
    head = font["head"]
    head.created = FIXED_TIMESTAMP
    head.modified = FIXED_TIMESTAMP
    return font


def to_bytes(font, flavor=None):
    font.flavor = flavor
    buf = io.BytesIO()
    font.save(buf)
    return buf.getvalue()


# --------------------------------------------------------------------------
# the fixtures
# --------------------------------------------------------------------------

def _normalise_head(data):
    """``head`` bytes with the two fields a container may legitimately change.

    ``checkSumAdjustment`` (offset 8) is a checksum over the whole file, and
    the WOFF2 writer sets ``flags`` bit 11 ("lossless") at offset 16.  Neither
    says anything about the font itself.
    """
    flags = int.from_bytes(data[16:18], "big") & ~0x0800
    return data[:8] + b"\0\0\0\0" + data[12:16] + flags.to_bytes(2, "big") + data[18:]


def build_recursive_family():
    """``recursive-vf`` and its two compressed containers.

    The real font, copied verbatim.  The WOFF and WOFF2 are re-containered from
    exactly those bytes so that all three are provably the same font.
    """
    raw = RECURSIVE_TTF.read_bytes()
    results = [("recursive-vf.ttf", raw)]
    reference = TTFont(io.BytesIO(raw), recalcTimestamp=False, lazy=True)
    for suffix, flavor in (("woff", "woff"), ("woff2", "woff2")):
        font = TTFont(
            io.BytesIO(raw), recalcTimestamp=False, recalcBBoxes=False, lazy=True
        )
        data = to_bytes(font, flavor)
        font.close()
        # the container must carry exactly the same font
        packed = TTFont(io.BytesIO(data), recalcTimestamp=False, lazy=True)
        if set(packed.keys()) != set(reference.keys()):
            raise AssertionError(
                f"recursive-vf-{suffix}: table set differs from the source ttf"
            )
        for tag in reference.keys():
            if tag == "GlyphOrder":
                continue
            a, b = packed.reader[tag], reference.reader[tag]
            if tag == "head":
                a, b = _normalise_head(a), _normalise_head(b)
            if a != b:
                raise AssertionError(
                    f"recursive-vf-{suffix}: table {tag!r} differs from the source ttf"
                )
        packed.close()
        results.append((f"recursive-vf-{suffix}.{suffix}", data))
    reference.close()
    return results


TWO_AXIS_GLYPHS = [".notdef", "H", "O", "period"]
TWO_AXIS_CMAP = {0x48: "H", 0x4F: "O", 0x2E: "period"}


def two_axis_master(stem, width_scale, family):
    """A stem-weight / width parametric master with simple contours only."""
    adv = {
        ".notdef": (600, 50),
        "H": (int(600 * width_scale), 60),
        "O": (int(620 * width_scale), 60),
        "period": (300, 90),
    }
    fb = new_master(TWO_AXIS_GLYPHS, TWO_AXIS_CMAP, adv, family)
    sx = width_scale

    def x(value):
        return int(round(value * sx))

    glyphs = {}
    glyphs[".notdef"] = simple_glyph(
        [cw_rect(50, 0, 550, 700), ccw_rect(120, 70, 480, 630)]
    )
    # H: two stems and a crossbar, three separate rectangles (overlap-free)
    glyphs["H"] = simple_glyph(
        [
            cw_rect(x(60), 0, x(60 + stem), 700),
            cw_rect(x(480 - stem), 0, x(480), 700),
            cw_rect(x(60 + stem), 300, x(480 - stem), 300 + stem // 2),
        ]
    )
    # O: a rectangular ring
    glyphs["O"] = simple_glyph(
        [
            cw_rect(x(60), 0, x(560), 700),
            ccw_rect(x(60 + stem), stem, x(560 - stem), 700 - stem),
        ]
    )
    glyphs["period"] = simple_glyph([cw_rect(x(90), 0, x(90 + stem), stem)])
    fb.setupGlyf(glyphs)
    return fb.font


def build_two_axis(with_avar):
    """wght x wdth, simple contours only.  ``with_avar`` toggles the axis maps."""
    family = "FixtureTwoAxis" if with_avar else "FixtureNoAvar"
    wght_map = [(100, 100), (400, 400), (700, 800), (900, 900)] if with_avar else None
    wdth_map = [(75, 75), (100, 100), (125, 125)] if with_avar else None
    axes = [
        axis("wght", "Weight", 100, 400, 900, wght_map),
        axis("wdth", "Width", 75, 100, 125, wdth_map),
    ]
    designs = [
        ({"Weight": 400, "Width": 100}, "default", 90, 1.00, True),
        ({"Weight": 100, "Width": 100}, "thin", 40, 1.00, False),
        ({"Weight": 900, "Width": 100}, "black", 200, 1.00, False),
        ({"Weight": 400, "Width": 75}, "narrow", 90, 0.78, False),
        ({"Weight": 400, "Width": 125}, "wide", 90, 1.22, False),
    ]
    sources = [
        source(two_axis_master(stem, scale, family), loc, name, is_default)
        for loc, name, stem, scale, is_default in designs
    ]
    vf = make_vf(axes, sources)
    if with_avar and "avar" not in vf:
        raise AssertionError("two-axis: expected an avar table")
    if not with_avar and "avar" in vf:
        raise AssertionError("no-avar: an avar table was produced anyway")
    return vf


def build_centred_default():
    """One axis, 100 / 400 / 900: the default sits off-centre.

    The two halves of the axis normalize with different scale factors, which is
    what a renormalizing slicer has to get right.
    """
    axes = [axis("wght", "Weight", 100, 400, 900)]
    designs = [
        ({"Weight": 400}, "default", 90, True),
        ({"Weight": 100}, "thin", 30, False),
        ({"Weight": 900}, "black", 220, False),
    ]
    sources = [
        source(two_axis_master(stem, 1.0, "FixtureCentred"), loc, name, is_default)
        for loc, name, stem, is_default in designs
    ]
    return make_vf(axes, sources)


def build_single_axis_min_default():
    """Default == minimum, the shape Recursive's ``wght`` has (300/300/1000)."""
    axes = [axis("wght", "Weight", 300, 300, 1000)]
    designs = [
        ({"Weight": 300}, "light", 60, True),
        ({"Weight": 1000}, "black", 230, False),
    ]
    sources = [
        source(two_axis_master(stem, 1.0, "FixtureMinDefault"), loc, name, is_default)
        for loc, name, stem, is_default in designs
    ]
    return make_vf(axes, sources)


COMPOSITE_GLYPHS = [".notdef", "square", "dot", "squaredot", "double"]
COMPOSITE_CMAP = {0x41: "square", 0x42: "dot", 0x43: "squaredot", 0x44: "double"}


def composite_master(size, dx, dy):
    adv = {name: (760, 0) for name in COMPOSITE_GLYPHS}
    adv[".notdef"] = (600, 50)
    fb = new_master(COMPOSITE_GLYPHS, COMPOSITE_CMAP, adv, "FixtureComposites")
    glyphs = {}
    glyphs[".notdef"] = simple_glyph(
        [cw_rect(50, 0, 550, 700), ccw_rect(120, 70, 480, 630)]
    )
    glyphs["square"] = simple_glyph([cw_rect(0, 0, size, size)])
    glyphs["dot"] = simple_glyph([cw_rect(0, 0, 120, 120)])
    # depth 1: two simple components
    pen = TTGlyphPen(glyphs)
    pen.addComponent("square", (1, 0, 0, 1, 0, 0))
    pen.addComponent("dot", (1, 0, 0, 1, dx, 520 + dy))
    glyphs["squaredot"] = pen.glyph()
    # depth 2: a component that is itself a composite
    pen = TTGlyphPen(glyphs)
    pen.addComponent("squaredot", (1, 0, 0, 1, 0, 0))
    pen.addComponent("dot", (1, 0, 0, 1, 500 + dx, 0))
    glyphs["double"] = pen.glyph()
    fb.setupGlyf(glyphs)
    return fb.font


def build_composites():
    """Composite glyphs, nested two deep, with gvar deltas on component offsets.

    ``double`` references ``squaredot``, which references ``square`` and ``dot``.
    The component offsets differ between masters, so ``gvar`` carries deltas on
    the component offsets rather than on point coordinates.
    """
    axes = [axis("wght", "Weight", 400, 400, 900)]
    designs = [
        ({"Weight": 400}, "default", 400, 0, 0, True),
        ({"Weight": 900}, "black", 470, 130, 70, False),
    ]
    sources = [
        source(composite_master(size, dx, dy), loc, name, is_default)
        for loc, name, size, dx, dy, is_default in designs
    ]
    vf = make_vf(axes, sources)
    glyf = vf["glyf"]
    if glyf["double"].components[0].glyphName != "squaredot":
        raise AssertionError("composites: `double` must reference `squaredot`")
    if not glyf["squaredot"].isComposite() or not glyf["double"].isComposite():
        raise AssertionError("composites: expected composite glyphs")
    def component_depth(name):
        glyph = glyf[name]
        if not glyph.isComposite():
            return 0
        return 1 + max(component_depth(c.glyphName) for c in glyph.components)

    depth = component_depth("double")
    if depth < 2:
        raise AssertionError(f"composites: `double` nests {depth} deep, want >= 2")
    for name in ("squaredot", "double"):
        variations = vf["gvar"].variations.get(name)
        if not variations:
            raise AssertionError(f"composites: no gvar deltas for composite {name}")
        # a composite's gvar deltas are one per component plus four phantoms
        n_components = len(glyf[name].components)
        for var in variations:
            if len(var.coordinates) != n_components + 4:
                raise AssertionError(
                    f"composites: {name} gvar tuple has {len(var.coordinates)} deltas, "
                    f"want {n_components} components + 4 phantom points"
                )
            if all(d in (None, (0, 0)) for d in var.coordinates[:n_components]):
                raise AssertionError(
                    f"composites: {name} has no gvar delta on any component offset"
                )
    return vf


def build_named_instances():
    """Several fvar named instances and a STAT with values on every axis."""
    family = "FixtureNamedInstances"
    axes = [
        axis("wght", "Weight", 100, 400, 900),
        axis("wdth", "Width", 75, 100, 125),
    ]
    designs = [
        ({"Weight": 400, "Width": 100}, "default", 90, 1.00, True),
        ({"Weight": 100, "Width": 100}, "thin", 40, 1.00, False),
        ({"Weight": 900, "Width": 100}, "black", 200, 1.00, False),
        ({"Weight": 400, "Width": 75}, "narrow", 90, 0.78, False),
        ({"Weight": 400, "Width": 125}, "wide", 90, 1.22, False),
    ]
    sources = [
        source(two_axis_master(stem, scale, family), loc, name, is_default)
        for loc, name, stem, scale, is_default in designs
    ]
    named = [
        ("Thin", 100, 100),
        ("Light", 300, 100),
        ("Regular", 400, 100),
        ("Bold", 700, 100),
        ("Black", 900, 100),
        ("Condensed Regular", 400, 75),
        ("Condensed Bold", 700, 75),
        ("Expanded Regular", 400, 125),
    ]
    instances = []
    for style, wght, wdth in named:
        inst = InstanceDescriptor()
        inst.familyName = family
        inst.styleName = style
        inst.postScriptFontName = f"{family}-{style.replace(' ', '')}"
        inst.location = {"Weight": wght, "Width": wdth}
        instances.append(inst)
    vf = make_vf(axes, sources, instances)

    ELIDABLE = 0x0002
    buildStatTable(
        vf,
        [
            {
                "tag": "wght",
                "name": "Weight",
                "ordering": 0,
                "values": [
                    {"value": 100, "name": "Thin"},
                    {"value": 300, "name": "Light"},
                    {"value": 400, "name": "Regular", "flags": ELIDABLE},
                    {"value": 700, "name": "Bold"},
                    {"value": 900, "name": "Black"},
                ],
            },
            {
                "tag": "wdth",
                "name": "Width",
                "ordering": 1,
                "values": [
                    {"value": 75, "name": "Condensed"},
                    {"value": 100, "name": "Normal", "flags": ELIDABLE},
                    {"value": 125, "name": "Expanded"},
                ],
            },
        ],
        elidedFallbackName="Regular",
    )
    if len(vf["fvar"].instances) != len(named):
        raise AssertionError("named-instances: fvar instance count is wrong")
    stat = vf["STAT"].table
    tagged = {rec.AxisTag for rec in stat.DesignAxisRecord.Axis}
    if tagged != {"wght", "wdth"}:
        raise AssertionError(f"named-instances: STAT design axes are {tagged}")
    valued = {
        stat.DesignAxisRecord.Axis[v.AxisIndex].AxisTag
        for v in stat.AxisValueArray.AxisValue
    }
    if valued != {"wght", "wdth"}:
        raise AssertionError(
            f"named-instances: STAT has axis values only on {valued}, want every axis"
        )
    return finalize(vf)


# -- overlapping ----------------------------------------------------------

OVERLAP_GLYPHS = [".notdef", "plus", "ring", "circled", "bowtie"]
OVERLAP_CMAP = {0x2B: "plus", 0x6F: "ring", 0x40: "circled", 0x78: "bowtie"}


def overlap_contours(g):
    """The four overlap glyphs, parameterised by a single growth amount ``g``.

    ``g`` is 0 in the default master and positive in the heavy master; every
    boundary moves outward by ``g`` and every counter inward by ``g``, so the
    topology -- and therefore the probe points used to verify the winding --
    is the same at both ends of the axis.
    """
    return {
        # two crossing bars.  Both wound clockwise, so the overlap in the
        # middle has winding -2: filled under non-zero, a hole under even-odd.
        "plus": [
            cw_rect(50, 300 - g, 750, 400 + g),
            cw_rect(350 - g, 0, 450 + g, 700),
        ],
        # an 'o': clockwise outer square with a counter-clockwise counter.
        "ring": [
            cw_rect(50 - g, 50 - g, 750 + g, 650 + g),
            ccw_rect(200 + g, 200 + g, 600 - g, 500 - g),
        ],
        # nesting depth 3: ring, its counter, a filled square inside that
        # counter, and that square's own counter.  Directions alternate.
        "circled": [
            cw_rect(20 - g, 20 - g, 780 + g, 680 + g),
            ccw_rect(110, 90, 690, 610),
            cw_rect(200, 160, 600, 540),
            ccw_rect(290 + g, 230 + g, 510 - g, 470 - g),
        ],
        # one self-intersecting contour: the two diagonals cross at (350, 350),
        # making two lobes of opposite winding, both filled under non-zero.
        "bowtie": [
            [(100 - g, 100 - g), (600 + g, 600 + g), (100 - g, 600 + g), (600 + g, 100 - g)]
        ],
    }


#: (glyph, probe point, expected |winding|).  The probes sit in every distinct
#: region of every glyph, including the ones that must be empty.  They are
#: chosen to stay in the same region across the whole axis.
OVERLAP_PROBES = [
    ("plus", (400, 350), 2, "both bars overlap"),
    ("plus", (150, 350), 1, "horizontal bar only"),
    ("plus", (400, 620), 1, "vertical bar only"),
    ("plus", (150, 620), 0, "outside both bars"),
    ("plus", (900, 350), 0, "right of everything"),
    ("ring", (100, 350), 1, "the ring band"),
    ("ring", (400, 350), 0, "the counter"),
    ("ring", (900, 350), 0, "outside"),
    ("circled", (60, 350), 1, "outer ring band"),
    ("circled", (150, 350), 0, "the outer counter"),
    ("circled", (240, 350), 1, "the inner filled square"),
    ("circled", (400, 350), 0, "the counter inside the counter"),
    ("circled", (900, 350), 0, "outside"),
    ("bowtie", (350, 500), 1, "upper lobe"),
    ("bowtie", (350, 200), 1, "lower lobe"),
    ("bowtie", (150, 350), 0, "outside, level with the crossing"),
    ("bowtie", (900, 350), 0, "outside"),
]

#: Expected orientation of each contour, as the sign of its signed area.
#: -1 is clockwise (a TrueType outer contour), +1 counter-clockwise (a counter).
OVERLAP_ORIENTATIONS = {
    "plus": [-1, -1],
    "ring": [-1, +1],
    "circled": [-1, +1, -1, +1],
}


def overlap_master(g):
    adv = {name: (820, 40) for name in OVERLAP_GLYPHS}
    adv[".notdef"] = (600, 50)
    fb = new_master(OVERLAP_GLYPHS, OVERLAP_CMAP, adv, "FixtureOverlapping")
    glyphs = {
        ".notdef": simple_glyph([cw_rect(50, 0, 550, 700), ccw_rect(120, 70, 480, 630)])
    }
    for name, contours in overlap_contours(g).items():
        glyphs[name] = simple_glyph(contours)
    fb.setupGlyf(glyphs)
    return fb.font


def build_overlapping():
    axes = [axis("wght", "Weight", 400, 400, 900)]
    sources = [
        source(overlap_master(0), {"Weight": 400}, "default", True),
        source(overlap_master(40), {"Weight": 900}, "black", False),
    ]
    return make_vf(axes, sources)


def check_overlapping_windings(font, report):
    """Prove the four overlap glyphs are filled the way they are meant to be.

    Reads the contours back out of the compiled ``glyf`` (not out of the source
    data structures), checks contour orientation by signed area, and evaluates
    the non-zero winding number at a probe point in every distinct region --
    at the axis default and at the axis maximum.
    """
    for location, label in (({"wght": 400}, "wght=400 (default)"), ({"wght": 900}, "wght=900 (max)")):
        static = instantiateVariableFont(copy.deepcopy(font), location, inplace=False)
        for glyph_name, expected in OVERLAP_ORIENTATIONS.items():
            contours = glyph_contours(static, glyph_name)
            got = [1 if signed_area(c) > 0 else -1 for c in contours]
            if got != expected:
                raise AssertionError(
                    f"overlapping/{glyph_name} at {label}: contour orientations "
                    f"{got}, expected {expected}"
                )
            report(
                f"    {label} {glyph_name}: orientations "
                + ", ".join("CW" if s < 0 else "CCW" for s in got)
            )
        # the bowtie is self-intersecting, so a single orientation is
        # meaningless; report its (near-zero) signed area instead.
        bowtie = glyph_contours(static, "bowtie")
        assert len(bowtie) == 1, "bowtie must be a single contour"
        report(
            f"    {label} bowtie: 1 self-intersecting contour, "
            f"signed area {signed_area(bowtie[0]):.0f} (two opposing lobes)"
        )
        for glyph_name, point, expected_abs, why in OVERLAP_PROBES:
            contours = glyph_contours(static, glyph_name)
            w = winding_number(point, contours)
            if abs(w) != expected_abs:
                raise AssertionError(
                    f"overlapping/{glyph_name} at {label}: winding at {point} "
                    f"is {w}, expected magnitude {expected_abs} ({why})"
                )
            report(
                f"    {label} {glyph_name} {point}: winding {w:+d} -> "
                f"{'FILLED' if w else 'empty '}  ({why})"
            )


# -- hinted ---------------------------------------------------------------

HINTED_GLYPHS = [".notdef", "H", "I"]
HINTED_CMAP = {0x48: "H", 0x49: "I"}

FPGM_ASM = [
    # function 0: round the point number on the stack to the grid
    "PUSHB[ ]",
    "0",
    "FDEF[ ]",
    "MDAP[1]",
    "ENDF[ ]",
]

PREP_ASM = [
    # turn dropout control on, and pick a scan type
    "PUSHW[ ]",
    "511",
    "SCANCTRL[ ]",
    "PUSHB[ ]",
    "4",
    "SCANTYPE[ ]",
]

#: cvt: 0 = baseline, 1 = cap height, 2 = descender, 3 = stem width
CVT_VALUES = [0, 700, -200, 90]

GLYPH_ASM = [
    # measure along the y axis, then round point 0 to the grid via fpgm fn 0
    "SVTCA[0]",
    "PUSHB[ ]",
    "0",
    "0",
    "CALL[ ]",
]


def hinted_master(stem):
    adv = {".notdef": (600, 50), "H": (560, 60), "I": (300, 90)}
    fb = new_master(HINTED_GLYPHS, HINTED_CMAP, adv, "FixtureHinted")
    glyphs = {
        ".notdef": simple_glyph([cw_rect(50, 0, 550, 700), ccw_rect(120, 70, 480, 630)]),
        "H": simple_glyph(
            [
                cw_rect(60, 0, 60 + stem, 700),
                cw_rect(500 - stem, 0, 500, 700),
                cw_rect(60 + stem, 300, 500 - stem, 300 + stem // 2),
            ]
        ),
        "I": simple_glyph([cw_rect(90, 0, 90 + stem, 700)]),
    }
    program = Program()
    program.fromAssembly(list(GLYPH_ASM))
    for name in ("H", "I"):
        glyphs[name].program = program
    fb.setupGlyf(glyphs)

    font = fb.font
    fpgm = newTable("fpgm")
    fpgm.program = Program()
    fpgm.program.fromAssembly(list(FPGM_ASM))
    font["fpgm"] = fpgm
    prep = newTable("prep")
    prep.program = Program()
    prep.program.fromAssembly(list(PREP_ASM))
    font["prep"] = prep
    cvt = newTable("cvt ")
    cvt.values = array("h", CVT_VALUES)
    font["cvt "] = cvt
    # maxp fields the hinting needs; these are not recalculated on compile.
    maxp = font["maxp"]
    maxp.maxZones = 2
    maxp.maxTwilightPoints = 16
    maxp.maxStorage = 8
    maxp.maxFunctionDefs = 1
    maxp.maxInstructionDefs = 0
    maxp.maxStackElements = 32
    return font


def build_hinted():
    """``prep``, ``fpgm``, ``cvt `` and per-glyph instructions, on a wght axis."""
    axes = [axis("wght", "Weight", 400, 400, 900)]
    sources = [
        source(hinted_master(90), {"Weight": 400}, "default", True),
        source(hinted_master(200), {"Weight": 900}, "black", False),
    ]
    vf = make_vf(axes, sources)
    for table in ("fpgm", "prep", "cvt "):
        if table not in vf:
            raise AssertionError(f"hinted: missing {table!r}")
    if list(vf["cvt "].values) != CVT_VALUES:
        raise AssertionError("hinted: cvt values did not survive")
    for name in ("H", "I"):
        program = getattr(vf["glyf"][name], "program", None)
        if program is None or not program.getBytecode():
            raise AssertionError(f"hinted: glyph {name} has no instructions")
    if vf["maxp"].maxFunctionDefs < 1:
        raise AssertionError("hinted: maxp.maxFunctionDefs must cover the fpgm FDEF")
    return vf


# -- gdef-varstore --------------------------------------------------------

KERN_GLYPHS = [".notdef", "A", "V", "T"]
KERN_CMAP = {0x41: "A", 0x56: "V", 0x54: "T"}

KERN_FEA = """
languagesystem DFLT dflt;
languagesystem latn dflt;

feature kern {
    pos A V %(av)d;
    pos V A %(va)d;
    pos T A %(ta)d;
} kern;
"""


def kern_master(stem, av, va, ta):
    adv = {".notdef": (600, 50), "A": (620, 20), "V": (620, 20), "T": (580, 10)}
    fb = new_master(KERN_GLYPHS, KERN_CMAP, adv, "FixtureGdefVarStore")
    glyphs = {
        ".notdef": simple_glyph([cw_rect(50, 0, 550, 700), ccw_rect(120, 70, 480, 630)]),
        "A": simple_glyph([cw_rect(40, 0, 40 + stem, 700)]),
        "V": simple_glyph([cw_rect(560 - stem, 0, 560, 700)]),
        "T": simple_glyph([cw_rect(40, 700 - stem, 540, 700)]),
    }
    fb.setupGlyf(glyphs)
    addOpenTypeFeaturesFromString(fb.font, KERN_FEA % {"av": av, "va": va, "ta": ta})
    return fb.font


def build_gdef_varstore():
    """Variable kerning: a GDEF item variation store referenced from GPOS.

    Two masters with the same ``kern`` feature and different values.  varLib
    merges the pair positioning into variable value records, which means a
    ``Device`` table of format ``VariationIndex`` pointing into the item
    variation store it puts in ``GDEF``.
    """
    axes = [axis("wght", "Weight", 400, 400, 900)]
    sources = [
        source(kern_master(90, -40, -20, -60), {"Weight": 400}, "default", True),
        source(kern_master(200, -120, -70, -150), {"Weight": 900}, "black", False),
    ]
    vf = make_vf(axes, sources)
    gdef = vf["GDEF"].table
    if getattr(gdef, "VarStore", None) is None:
        raise AssertionError("gdef-varstore: GDEF has no item variation store")
    if not _gpos_has_variation_index(vf):
        raise AssertionError("gdef-varstore: no GPOS value record references the store")
    return vf


def _gpos_has_variation_index(font):
    for lookup in font["GPOS"].table.LookupList.Lookup:
        for sub in lookup.SubTable:
            for pair_set in getattr(sub, "PairSet", None) or []:
                for record in pair_set.PairValueRecord:
                    for value in (record.Value1, record.Value2):
                        if value is None:
                            continue
                        for attr in ("XAdvDevice", "XPlaDevice", "YAdvDevice", "YPlaDevice"):
                            device = getattr(value, attr, None)
                            if device is not None and device.DeltaFormat == 0x8000:
                                return True
    return False


# -- cff2 -----------------------------------------------------------------

CFF2_GLYPHS = [".notdef", "H", "I", "period"]
CFF2_CMAP = {0x48: "H", 0x49: "I", 0x2E: "period"}


def cff2_master(stem):
    adv = {".notdef": (600, 50), "H": (560, 60), "I": (300, 90), "period": (300, 90)}
    fb = new_master(CFF2_GLYPHS, CFF2_CMAP, adv, "FixtureCff2", is_ttf=False)
    # CFF outlines follow the PostScript convention: outer contours
    # counter-clockwise, counters clockwise -- the opposite of TrueType.
    outlines = {
        ".notdef": [ccw_rect(50, 0, 550, 700), cw_rect(120, 70, 480, 630)],
        "H": [
            ccw_rect(60, 0, 60 + stem, 700),
            ccw_rect(500 - stem, 0, 500, 700),
            ccw_rect(60 + stem, 300, 500 - stem, 300 + stem // 2),
        ],
        "I": [ccw_rect(90, 0, 90 + stem, 700)],
        "period": [ccw_rect(90, 0, 90 + stem, stem)],
    }
    charstrings = {}
    for name, contours in outlines.items():
        pen = T2CharStringPen(adv[name][0], None)
        draw_contours(pen, contours)
        charstrings[name] = pen.getCharString()
    fb.setupCFF(
        "FixtureCff2-Regular",
        {
            "FullName": "FixtureCff2 Regular",
            "FamilyName": "FixtureCff2",
            "Weight": "Regular",
            "version": "1.000",
        },
        charstrings,
        {},
    )
    return fb.font


def build_cff2():
    """CFF2 outlines: varLib turns CFF masters into a CFF2 variable font."""
    axes = [axis("wght", "Weight", 400, 400, 900)]
    sources = [
        source(cff2_master(90), {"Weight": 400}, "default", True),
        source(cff2_master(220), {"Weight": 900}, "black", False),
    ]
    vf = make_vf(axes, sources)
    if "CFF2" not in vf:
        raise AssertionError("cff2-vf: no CFF2 table was produced")
    return vf


# --------------------------------------------------------------------------
# verification
# --------------------------------------------------------------------------

def verify_font(name, data, report):
    """Open, round-trip, structurally check and (if variable) instance a font."""
    flavor = None
    if name.endswith(".woff"):
        flavor = "woff"
    elif name.endswith(".woff2"):
        flavor = "woff2"

    font = TTFont(io.BytesIO(data), recalcTimestamp=False)
    if font.flavor != flavor:
        raise AssertionError(f"{name}: flavor is {font.flavor!r}, expected {flavor!r}")

    outline_tables = {"glyf", "CFF ", "CFF2"} & set(font.keys())
    if not outline_tables:
        raise AssertionError(f"{name}: no outline table")
    missing = REQUIRED_TABLES - set(font.keys())
    if missing:
        raise AssertionError(f"{name}: missing required tables {sorted(missing)}")
    if "glyf" in font and "loca" not in font:
        raise AssertionError(f"{name}: glyf without loca")

    hmtx = font["hmtx"]
    for glyph_name in font.getGlyphOrder():
        advance = hmtx[glyph_name][0]
        if advance <= 0:
            raise AssertionError(f"{name}: glyph {glyph_name} has advance {advance}")

    # round trip
    buf = io.BytesIO()
    font.save(buf)
    buf.seek(0)
    reopened = TTFont(buf, recalcTimestamp=False)
    reopened.getGlyphOrder()
    for table in sorted(reopened.keys()):
        reopened[table]  # force decompile of everything

    detail = f"{sorted(outline_tables)[0]}, {font['maxp'].numGlyphs} glyphs"
    if "fvar" in font:
        axes = font["fvar"].axes
        detail += ", axes " + " ".join(
            f"{a.axisTag} {a.minValue:g}/{a.defaultValue:g}/{a.maxValue:g}" for a in axes
        )
        detail += f", {len(font['fvar'].instances)} named instances"
        locations = [{a.axisTag: a.defaultValue for a in axes}]
        for a in axes:
            for value in (a.minValue, a.maxValue):
                loc = {b.axisTag: b.defaultValue for b in axes}
                loc[a.axisTag] = value
                locations.append(loc)
        for loc in locations:
            static = instantiateVariableFont(copy.deepcopy(font), loc, inplace=False)
            if "fvar" in static:
                raise AssertionError(f"{name}: fvar survived pinning every axis at {loc}")
            sbuf = io.BytesIO()
            static.save(sbuf)
            TTFont(io.BytesIO(sbuf.getvalue()), recalcTimestamp=False).getGlyphOrder()
        detail += f", instanced at {len(locations)} locations"
    else:
        detail += ", static"

    optional = sorted({"avar", "gvar", "STAT", "HVAR", "GDEF", "GPOS", "GSUB",
                       "fpgm", "prep", "cvt "} & set(font.keys()))
    if optional:
        detail += ", has " + " ".join(t.strip() for t in optional)
    report(f"    {detail}")
    return font


def build_all(report):
    """Return ``[(filename, bytes), ...]`` for the whole roster."""
    fonts = []
    fonts.extend(build_recursive_family())
    fonts.append(("two-axis.ttf", to_bytes(build_two_axis(with_avar=True))))
    fonts.append(("no-avar.ttf", to_bytes(build_two_axis(with_avar=False))))
    fonts.append(("centred-default.ttf", to_bytes(build_centred_default())))
    fonts.append(
        ("single-axis-min-default.ttf", to_bytes(build_single_axis_min_default()))
    )
    fonts.append(("composites.ttf", to_bytes(build_composites())))
    fonts.append(("named-instances.ttf", to_bytes(build_named_instances())))

    overlapping = build_overlapping()
    check_overlapping_windings(overlapping, report)
    fonts.append(("overlapping.ttf", to_bytes(overlapping)))

    fonts.append(("hinted.ttf", to_bytes(build_hinted())))
    fonts.append(("gdef-varstore.ttf", to_bytes(build_gdef_varstore())))
    fonts.append(("cff2-vf.otf", to_bytes(build_cff2())))
    return fonts


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--check",
        action="store_true",
        help="build and verify but do not write anything",
    )
    args = parser.parse_args(argv)

    if not RECURSIVE_TTF.is_file():
        print(f"missing source font: {RECURSIVE_TTF}", file=sys.stderr)
        return 2

    lines = []
    report = lines.append

    print("building and verifying the winding of the `overlapping` glyphs")
    fonts = build_all(report)
    for line in lines:
        print(line)

    print("\nrebuilding to confirm determinism")
    again = build_all(lambda _line: None)
    if [n for n, _ in fonts] != [n for n, _ in again]:
        print("  FAIL: the two runs produced different files", file=sys.stderr)
        return 1
    for (name, first), (_, second) in zip(fonts, again):
        if first != second:
            print(f"  FAIL: {name} is not reproducible", file=sys.stderr)
            return 1
    print(f"  ok: {len(fonts)} fonts identical across two runs")

    print("\nverifying")
    for name, data in fonts:
        print(f"  {name}")
        verify_font(name, data, print)

    if args.check:
        print("\n--check: nothing written")
        return 0

    OUT.mkdir(parents=True, exist_ok=True)
    stale = {p.name for p in OUT.iterdir() if p.is_file()} - {n for n, _ in fonts}
    for name in sorted(stale):
        (OUT / name).unlink()
        print(f"\nremoved stale {name}")

    print("\nwriting to", OUT.relative_to(REPO))
    total = 0
    for name, data in fonts:
        (OUT / name).write_bytes(data)
        total += len(data)
        print(f"  {name:30s} {len(data):7d} bytes  sha256:{hashlib.sha256(data).hexdigest()[:16]}")
    print(f"  {'total':30s} {total:7d} bytes")
    return 0


if __name__ == "__main__":
    sys.exit(main())
