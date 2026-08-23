//! Does the CFF2 instancer put the outlines where the font says they should be?
//!
//! Same oracle as `static_instance_matches_skrifa`, pointed at the other outline format:
//!
//! > drawing the **variable** font at location L
//! > must produce the same outlines as
//! > drawing our **instance** at whatever is left of L.
//!
//! skrifa draws CFF2 through its own charstring interpreter and its own blend evaluation,
//! neither of which shares a line with the rewriter here, so an agreement between the two
//! is real evidence. It also exercises the serialiser end to end: a CFF2 table with a
//! misplaced offset does not draw at all.
//!
//! Run with `cargo test -p slice-core --test cff2_instance_matches_skrifa`.

use read_fonts::TableProvider;
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::prelude::*;
use skrifa::{FontRef, MetadataProvider};

use slice_core::axes::AxisLimit;
use slice_core::instancer::{
    instantiate_partial, instantiate_static, normalize_location, plan_axes,
};
use slice_core::SliceFont;

/// The conformance corpus's CFF2 fixture: `.notdef H I period`, one `wght` axis
/// 400/400/900, built by `tests/suite/fixtures/build.py`.
const CFF2_VF: &[u8] = include_bytes!("../../../tests/suite/fixtures/out/cff2-vf.otf");

/// One font unit, and the reason is worth writing down because it is *not* an error in
/// the instancing.
///
/// A resolved blend is stored as an integer, because fontTools stores it as one:
/// `instantiateCFF2` adds `round(defaultDelta)` to the base value, and this copies that
/// so the two agree exactly (`tools/compare-cff2-with-fonttools.py` shows every
/// charstring identical). The *reference* side of this comparison is not rounded that
/// way: skrifa quantizes the location to F2Dot14 and then truncates each final
/// coordinate towards zero in unscaled mode, so a value the instance stores as 384 is
/// drawn from the variable font as 383.998 and reported as 383.
///
/// Measured on this fixture, over five pinned weights and five interpolated ones, the
/// largest disagreement is exactly 1 unit and only ever appears where the exact value
/// lands within a thousandth of an integer.
const TOLERANCE: f32 = 1.0001;

#[derive(Default, PartialEq, Debug)]
struct Recorder {
    ops: Vec<Op>,
}

#[derive(PartialEq, Debug, Clone, Copy)]
enum Op {
    Move(f32, f32),
    Line(f32, f32),
    Quad(f32, f32, f32, f32),
    Curve(f32, f32, f32, f32, f32, f32),
    Close,
}

impl OutlinePen for Recorder {
    fn move_to(&mut self, x: f32, y: f32) {
        self.ops.push(Op::Move(x, y));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.ops.push(Op::Line(x, y));
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.ops.push(Op::Quad(cx, cy, x, y));
    }
    fn curve_to(&mut self, c1x: f32, c1y: f32, c2x: f32, c2y: f32, x: f32, y: f32) {
        self.ops.push(Op::Curve(c1x, c1y, c2x, c2y, x, y));
    }
    fn close(&mut self) {
        self.ops.push(Op::Close);
    }
}

impl Op {
    fn kind(&self) -> &'static str {
        match self {
            Op::Move(..) => "move",
            Op::Line(..) => "line",
            Op::Quad(..) => "quad",
            Op::Curve(..) => "curve",
            Op::Close => "close",
        }
    }

    fn coords(&self) -> Vec<f32> {
        match *self {
            Op::Move(x, y) | Op::Line(x, y) => vec![x, y],
            Op::Quad(a, b, c, d) => vec![a, b, c, d],
            Op::Curve(a, b, c, d, e, f) => vec![a, b, c, d, e, f],
            Op::Close => vec![],
        }
    }
}

fn draw(font: &FontRef, gid: GlyphId, location: LocationRef) -> Recorder {
    let mut pen = Recorder::default();
    if let Some(glyph) = font.outline_glyphs().get(gid) {
        glyph
            .draw(DrawSettings::unhinted(Size::unscaled(), location), &mut pen)
            .expect("glyph should draw");
    }
    pen
}

fn compare(reference: &Recorder, actual: &Recorder, what: &str) -> f32 {
    assert_eq!(
        reference.ops.len(),
        actual.ops.len(),
        "{what}: different number of path operations\n  source:   {:?}\n  instance: {:?}",
        reference.ops,
        actual.ops
    );
    let mut worst = 0.0f32;
    for (i, (r, a)) in reference.ops.iter().zip(&actual.ops).enumerate() {
        assert_eq!(
            r.kind(),
            a.kind(),
            "{what}: operation {i} is a different kind"
        );
        for (rc, ac) in r.coords().iter().zip(a.coords().iter()) {
            let diff = (rc - ac).abs();
            worst = worst.max(diff);
            assert!(
                diff <= TOLERANCE,
                "{what}: operation {i} ({}) differs by {diff} units\n  \
                 variable font: {r:?}\n  instance:      {a:?}",
                r.kind()
            );
        }
    }
    worst
}

fn pinned_at(weight: f64) -> Vec<u8> {
    let slice_font = SliceFont::load(CFF2_VF.to_vec()).unwrap();
    let font = slice_font.font_ref().unwrap();
    let axes = slice_font.axes().unwrap();
    let limits = vec![AxisLimit::Pin(weight); axes.len()];
    let location = normalize_location(&font, &axes, &[weight]);
    let plans = plan_axes(&font, &axes, &limits);
    instantiate_static(&font, &location, &plans).expect("instancing should succeed")
}

fn restricted_to(min: f64, max: f64) -> Vec<u8> {
    let slice_font = SliceFont::load(CFF2_VF.to_vec()).unwrap();
    let font = slice_font.font_ref().unwrap();
    let axes = slice_font.axes().unwrap();
    let plans = plan_axes(&font, &axes, &[AxisLimit::range(min, max)]);
    instantiate_partial(&font, &plans).expect("instancing should succeed")
}

#[test]
fn a_pinned_cff2_instance_draws_what_the_variable_font_draws() {
    let variable = FontRef::new(CFF2_VF).unwrap();
    let glyph_count = variable.maxp().unwrap().num_glyphs();
    let mut worst = 0.0f32;

    // Both masters and three interior weights, so interpolation is exercised as well as
    // the two ends where a blend's scalar is 0 or 1.
    for weight in [400.0, 500.0, 650.0, 700.0, 900.0] {
        let bytes = pinned_at(weight);
        let instance = FontRef::new(&bytes).expect("the instance should parse");

        assert!(instance.fvar().is_err(), "a pinned instance keeps no fvar");
        assert!(instance.glyf().is_err(), "outlines must stay in CFF2");
        assert!(
            instance.cff2().is_ok(),
            "fontTools keeps CFF2 for a static instance, and so does this"
        );
        assert!(
            instance.hvar().is_err(),
            "HVAR describes axes the instance no longer has"
        );

        let coords = variable.axes().location([("wght", weight as f32)]);
        for gid in 0..glyph_count {
            let gid = GlyphId::new(gid as u32);
            let reference = draw(&variable, gid, (&coords).into());
            let actual = draw(&instance, gid, LocationRef::default());
            worst = worst.max(compare(
                &reference,
                &actual,
                &format!("glyph {gid} at wght={weight}"),
            ));
        }
    }
    println!("largest deviation over the pinned locations: {worst} font units");
}

#[test]
fn a_restricted_cff2_instance_still_interpolates_correctly() {
    let variable = FontRef::new(CFF2_VF).unwrap();
    let glyph_count = variable.maxp().unwrap().num_glyphs();
    let bytes = restricted_to(400.0, 700.0);
    let instance = FontRef::new(&bytes).expect("the instance should parse");

    let fvar = instance.fvar().expect("a restricted axis survives in fvar");
    let axis = fvar.axes().unwrap().first().copied().unwrap();
    assert_eq!(
        (
            axis.min_value().to_f64(),
            axis.default_value().to_f64(),
            axis.max_value().to_f64()
        ),
        (400.0, 400.0, 700.0)
    );
    assert!(
        instance.cff2().is_ok(),
        "the outlines have to stay where they were"
    );

    let mut worst = 0.0f32;
    for weight in [400.0, 475.0, 550.0, 625.0, 700.0] {
        let source = variable.axes().location([("wght", weight as f32)]);
        let target = instance.axes().location([("wght", weight as f32)]);
        for gid in 0..glyph_count {
            let gid = GlyphId::new(gid as u32);
            let reference = draw(&variable, gid, (&source).into());
            let actual = draw(&instance, gid, (&target).into());
            worst = worst.max(compare(
                &reference,
                &actual,
                &format!("glyph {gid} at wght={weight}"),
            ));
        }
    }
    println!("largest deviation across the restricted range: {worst} font units");
}

#[test]
fn a_restricted_instance_still_says_no_to_a_weight_outside_the_range() {
    // The point of narrowing an axis is that the design space really is smaller. skrifa
    // clamps a request beyond the new maximum, so wght=900 on a 400:700 instance must
    // draw what wght=700 draws, not what the original drew at 900.
    let variable = FontRef::new(CFF2_VF).unwrap();
    let bytes = restricted_to(400.0, 700.0);
    let instance = FontRef::new(&bytes).unwrap();

    let gid = GlyphId::new(1); // H, the glyph with the most blends
    let clamped = draw(
        &instance,
        gid,
        (&instance.axes().location([("wght", 900.0)])).into(),
    );
    let at_max = draw(
        &instance,
        gid,
        (&instance.axes().location([("wght", 700.0)])).into(),
    );
    assert_eq!(clamped, at_max);

    let source_at_900 = draw(
        &variable,
        gid,
        (&variable.axes().location([("wght", 900.0)])).into(),
    );
    assert_ne!(
        clamped, source_at_900,
        "the instance should no longer reach the original maximum"
    );
}
