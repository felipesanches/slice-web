//! Does our static instancer put the outlines where the font says they should be?
//!
//! The engine applies `gvar` deltas itself rather than delegating to a renderer, so it
//! needs an independent check. skrifa is that check: it is a separate implementation of
//! the same specification, written by different people, and it can evaluate a variable
//! font at an arbitrary location.
//!
//! The property tested here is:
//!
//! > drawing the **variable** font at location L
//! > must produce the same outlines and advances as
//! > drawing our **instance at L** at its own default location.
//!
//! Any disagreement means one of the two got the variation maths wrong, and since skrifa
//! is what browsers and Android use to render these fonts, it is not going to be skrifa.
//!
//! Run with `cargo test -p slice-core --test static_instance_matches_skrifa`. Pass
//! `-- --nocapture` to see the largest deviation actually observed, which is the number
//! quoted in the README.

use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::prelude::*;
use read_fonts::TableProvider;
use skrifa::{FontRef, MetadataProvider};

use slice_core::instancer::{instantiate_static, normalize_location};
use slice_core::SliceFont;

const RECURSIVE_VF: &[u8] = include_bytes!("../../../testdata/fonts/Recursive-VF.subset.ttf");

/// Coordinates are integers in the instanced font, so up to half a unit of rounding
/// would be defensible. In practice the agreement is exact -- see
/// `outlines_match_skrifa_at_every_location`, which asserts that the observed deviation
/// is zero. The tolerance exists so that a regression reports *how far* it drifted
/// rather than failing on the first point.
const TOLERANCE: f32 = 0.5001;

/// Records what a glyph is drawn as, so two drawings can be compared.
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

/// Compare two drawings, returning the largest coordinate deviation.
fn compare(reference: &Recorder, actual: &Recorder, what: &str) -> f32 {
    assert_eq!(
        reference.ops.len(),
        actual.ops.len(),
        "{what}: different number of path operations"
    );
    let mut worst = 0.0f32;
    for (i, (r, a)) in reference.ops.iter().zip(&actual.ops).enumerate() {
        assert_eq!(r.kind(), a.kind(), "{what}: operation {i} is a different kind");
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

/// Build a static instance at the given user-space coordinates.
fn instance_at(user: &[(&str, f64)]) -> Vec<u8> {
    let slice_font = SliceFont::load(RECURSIVE_VF.to_vec()).unwrap();
    let font = slice_font.font_ref().unwrap();
    let axes = slice_font.axes().unwrap();

    let coords: Vec<f64> = axes
        .iter()
        .map(|axis| {
            user.iter()
                .find(|(tag, _)| *tag == axis.tag)
                .map(|(_, v)| *v)
                .unwrap_or(axis.default)
        })
        .collect();

    let location = normalize_location(&font, &axes, &coords);
    instantiate_static(&font, &location).expect("instancing should succeed")
}

/// Every location worth checking: the default, each axis at each end, and a couple of
/// interior points where interpolation rather than a master is being exercised.
fn locations() -> Vec<Vec<(&'static str, f64)>> {
    vec![
        vec![],
        vec![("wght", 1000.0)],
        vec![("wght", 650.0)],
        vec![("wght", 437.5)],
        vec![("MONO", 1.0)],
        vec![("MONO", 0.5)],
        vec![("CASL", 1.0)],
        vec![("CASL", 0.25)],
        vec![("slnt", -15.0)],
        vec![("slnt", -7.5)],
        vec![("CRSV", 0.0)],
        vec![("CRSV", 1.0)],
        vec![("wght", 800.0), ("CASL", 1.0)],
        vec![("wght", 800.0), ("CASL", 1.0), ("MONO", 1.0)],
        vec![
            ("wght", 612.0),
            ("CASL", 0.4),
            ("MONO", 0.7),
            ("slnt", -9.0),
            ("CRSV", 0.8),
        ],
    ]
}

#[test]
fn outlines_match_skrifa_at_every_location() {
    let variable = FontRef::new(RECURSIVE_VF).unwrap();
    let glyph_count = variable.maxp().unwrap().num_glyphs();
    let mut worst_overall = 0.0f32;
    let mut worst_where = String::new();

    for user in locations() {
        let instanced_bytes = instance_at(&user);
        let instanced = FontRef::new(&instanced_bytes).expect("instance should parse");

        // The instance must have no axes left at all.
        assert!(
            instanced.fvar().is_err(),
            "a fully pinned instance should have no fvar"
        );
        assert!(
            instanced.gvar().is_err(),
            "a fully pinned instance should have no gvar"
        );
        assert_eq!(
            instanced.maxp().unwrap().num_glyphs(),
            glyph_count,
            "instancing must not change the glyph count"
        );

        let location = variable.axes().location(
            user.iter()
                .map(|(tag, value)| (*tag, *value as f32))
                .collect::<Vec<_>>(),
        );

        for gid in 0..glyph_count {
            let gid = GlyphId::new(gid as u32);
            let reference = draw(&variable, gid, (&location).into());
            let actual = draw(&instanced, gid, LocationRef::default());
            let label = format!("{user:?} glyph {}", gid.to_u32());
            let worst = compare(&reference, &actual, &label);
            if worst > worst_overall {
                worst_overall = worst;
                worst_where = label;
            }
        }
    }

    println!(
        "largest outline deviation across {} locations: {worst_overall} font units",
        locations().len()
    );
    assert_eq!(
        worst_overall, 0.0,
        "the instancer is expected to agree with skrifa exactly, but drifted by \
         {worst_overall} font units at {worst_where}. Investigate with:\n  \
         cargo run -p slice-core --features testdata --example probe_glyph -- <gid> <tag=value ...>"
    );
}

#[test]
fn advance_widths_match_skrifa_at_every_location() {
    let variable = FontRef::new(RECURSIVE_VF).unwrap();
    let glyph_count = variable.maxp().unwrap().num_glyphs();
    let mut worst_overall = 0.0f32;

    for user in locations() {
        let instanced_bytes = instance_at(&user);
        let instanced = FontRef::new(&instanced_bytes).unwrap();

        let location = variable.axes().location(
            user.iter()
                .map(|(tag, value)| (*tag, *value as f32))
                .collect::<Vec<_>>(),
        );
        let reference_metrics = variable.glyph_metrics(Size::unscaled(), &location);
        let actual_metrics = instanced.glyph_metrics(Size::unscaled(), LocationRef::default());

        for gid in 0..glyph_count {
            let gid = GlyphId::new(gid as u32);
            let reference = reference_metrics.advance_width(gid).unwrap_or(0.0);
            let actual = actual_metrics.advance_width(gid).unwrap_or(0.0);
            let diff = (reference - actual).abs();
            worst_overall = worst_overall.max(diff);
            assert!(
                diff <= TOLERANCE,
                "{user:?} glyph {}: advance {reference} vs {actual}",
                gid.to_u32()
            );
        }
    }

    println!("largest advance deviation: {worst_overall} font units");
    assert_eq!(worst_overall, 0.0, "advances are expected to agree with skrifa exactly");
}

#[test]
fn instancing_at_the_default_preserves_the_outlines_exactly() {
    // At the default location no delta applies, so this is a pure round trip through
    // read-glyph / build-glyph and should be exact, not merely within tolerance.
    let variable = FontRef::new(RECURSIVE_VF).unwrap();
    let instanced_bytes = instance_at(&[]);
    let instanced = FontRef::new(&instanced_bytes).unwrap();

    for gid in 0..variable.maxp().unwrap().num_glyphs() {
        let gid = GlyphId::new(gid as u32);
        let reference = draw(&variable, gid, LocationRef::default());
        let actual = draw(&instanced, gid, LocationRef::default());
        assert_eq!(
            reference, actual,
            "glyph {} changed when instanced at the default location",
            gid.to_u32()
        );
    }
}

#[test]
fn the_instance_is_smaller_than_the_variable_font() {
    // Not a correctness property, but a strong smoke test that the variation tables
    // really were dropped rather than copied across.
    let instanced = instance_at(&[("wght", 700.0)]);
    assert!(
        instanced.len() < RECURSIVE_VF.len(),
        "instance is {} bytes, variable font is {} bytes",
        instanced.len(),
        RECURSIVE_VF.len()
    );
}
