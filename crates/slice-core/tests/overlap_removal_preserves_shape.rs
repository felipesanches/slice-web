//! Does removing overlaps change what the glyphs look like?
//!
//! Merging contours is only correct if the *filled region* comes out the same. The
//! outline changes by design — that is the whole point — so the outlines cannot be
//! compared directly. What can be compared is which points are inside the glyph.
//!
//! Both the before and after outlines are filled with the non-zero winding rule, the
//! rule `glyf` is defined in terms of, and a dense grid of sample points is checked
//! against both. Any systematic error shows up immediately: a filled counter turns a
//! whole region from empty to solid, and a dropped contour does the reverse.
//!
//! A thin band of disagreement along the edges is expected and unavoidable. Merged
//! contours are refitted from cubics back to quadratics and then rounded to integer
//! coordinates, so an edge can move by up to about a unit. Sample points within that
//! distance of an edge are therefore not counted; the test asserts that *no* point
//! further away than that changes.

use kurbo::{BezPath, ParamCurveNearest, Point, Shape};
use read_fonts::TableProvider;
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::prelude::*;
use skrifa::{FontRef, MetadataProvider};

use slice_core::axes::AxisLimit;
use slice_core::instancer::{instantiate_static, normalize_location, plan_axes};
use slice_core::overlaps::remove_overlaps;
use slice_core::SliceFont;

const RECURSIVE_VF: &[u8] = include_bytes!("../../../testdata/fonts/Recursive-VF.subset.ttf");

/// The conformance corpus's CFF2 fixture, so the other outline format is exercised too.
const CFF2_VF: &[u8] = include_bytes!("../../../tests/suite/fixtures/out/cff2-vf.otf");

/// How far from an edge a sample point must be to count, in font units.
///
/// Covers the quadratic refit tolerance plus a half-unit of coordinate rounding, with
/// room to spare.
const EDGE_MARGIN: f64 = 1.5;

#[derive(Default)]
struct Contours {
    done: Vec<BezPath>,
    current: Option<BezPath>,
}

impl Contours {
    fn flush(&mut self) {
        if let Some(mut path) = self.current.take() {
            if !path.elements().is_empty() {
                path.close_path();
                self.done.push(path);
            }
        }
    }
    fn finish(mut self) -> Vec<BezPath> {
        self.flush();
        self.done
    }
}

impl OutlinePen for Contours {
    fn move_to(&mut self, x: f32, y: f32) {
        self.flush();
        let mut path = BezPath::new();
        path.move_to(Point::new(x as f64, y as f64));
        self.current = Some(path);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        if let Some(p) = &mut self.current {
            p.line_to(Point::new(x as f64, y as f64));
        }
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        if let Some(p) = &mut self.current {
            p.quad_to(
                Point::new(cx as f64, cy as f64),
                Point::new(x as f64, y as f64),
            );
        }
    }
    fn curve_to(&mut self, a: f32, b: f32, c: f32, d: f32, x: f32, y: f32) {
        if let Some(p) = &mut self.current {
            p.curve_to(
                Point::new(a as f64, b as f64),
                Point::new(c as f64, d as f64),
                Point::new(x as f64, y as f64),
            );
        }
    }
    fn close(&mut self) {
        self.flush();
    }
}

fn contours(font: &FontRef, gid: GlyphId) -> Vec<BezPath> {
    let mut pen = Contours::default();
    if let Some(glyph) = font.outline_glyphs().get(gid) {
        glyph
            .draw(
                DrawSettings::unhinted(Size::unscaled(), LocationRef::default()),
                &mut pen,
            )
            .expect("glyph should draw");
    }
    pen.finish()
}

fn filled(paths: &[BezPath], point: Point) -> bool {
    paths.iter().map(|p| p.winding(point)).sum::<i32>() != 0
}

/// Distance from `point` to the nearest point on any outline.
fn distance_to_outline(paths: &[BezPath], point: Point) -> f64 {
    let mut best = f64::MAX;
    for path in paths {
        for segment in path.segments() {
            let nearest = segment.nearest(point, 1e-3);
            best = best.min(nearest.distance_sq.sqrt());
            if best < EDGE_MARGIN {
                return best;
            }
        }
    }
    best
}

fn instance(user: &[(&str, f64)]) -> Vec<u8> {
    let slice_font = SliceFont::load(RECURSIVE_VF.to_vec()).unwrap();
    let font = slice_font.font_ref().unwrap();
    let axes = slice_font.axes().unwrap();
    let coords: Vec<f64> = axes
        .iter()
        .map(|axis| {
            user.iter()
                .find(|(t, _)| *t == axis.tag)
                .map(|(_, v)| *v)
                .unwrap_or(axis.default)
        })
        .collect();
    let limits: Vec<AxisLimit> = coords.iter().map(|v| AxisLimit::Pin(*v)).collect();
    let location = normalize_location(&font, &axes, &coords);
    let plans = plan_axes(&font, &axes, &limits);
    instantiate_static(&font, &location, &plans).unwrap()
}

#[test]
fn the_filled_region_survives_overlap_removal() {
    // A heavy weight is where a typeface's strokes are most likely to overlap.
    let instanced = instance(&[("wght", 1000.0)]);
    let (simplified, report) = remove_overlaps(&instanced).expect("overlap removal should run");

    println!("{}", report.summary());
    assert!(
        report.failed.is_empty(),
        "some glyphs could not be processed: {:?}",
        report.failed
    );

    let before_font = FontRef::new(&instanced).unwrap();
    let after_font = FontRef::new(&simplified).unwrap();
    let glyph_count = before_font.maxp().unwrap().num_glyphs();

    let mut checked = 0u64;
    let mut skipped_near_edge = 0u64;
    let mut mismatches = Vec::new();

    for gid in 0..glyph_count {
        let gid = GlyphId::new(gid as u32);
        let before = contours(&before_font, gid);
        let after = contours(&after_font, gid);
        if before.is_empty() && after.is_empty() {
            continue;
        }

        let bounds = before
            .iter()
            .map(|p| p.bounding_box())
            .reduce(|a, b| a.union(b))
            .unwrap_or_default();
        if bounds.width() <= 0.0 || bounds.height() <= 0.0 {
            continue;
        }

        // A grid a little larger than the glyph, so a contour that grew is caught too.
        const STEPS: i32 = 60;
        for i in 0..=STEPS {
            for j in 0..=STEPS {
                let point = Point::new(
                    bounds.x0 - 4.0 + (bounds.width() + 8.0) * (i as f64 / STEPS as f64),
                    bounds.y0 - 4.0 + (bounds.height() + 8.0) * (j as f64 / STEPS as f64),
                );
                let a = filled(&before, point);
                let b = filled(&after, point);
                if a == b {
                    checked += 1;
                    continue;
                }
                // Disagreements right on an edge are the refit tolerance, not an error.
                if distance_to_outline(&before, point) < EDGE_MARGIN
                    || distance_to_outline(&after, point) < EDGE_MARGIN
                {
                    skipped_near_edge += 1;
                    continue;
                }
                checked += 1;
                mismatches.push((gid.to_u32(), point, a, b));
            }
        }
    }

    println!(
        "checked {checked} sample points ({skipped_near_edge} skipped as within \
         {EDGE_MARGIN} units of an edge)"
    );
    assert!(
        mismatches.is_empty(),
        "{} sample points changed fill state away from any edge; first few: {:?}",
        mismatches.len(),
        &mismatches[..mismatches.len().min(5)]
    );
}

#[test]
fn overlap_removal_leaves_a_usable_font() {
    let instanced = instance(&[("wght", 1000.0)]);
    let (simplified, _) = remove_overlaps(&instanced).unwrap();

    let before = FontRef::new(&instanced).unwrap();
    let after = FontRef::new(&simplified).expect("the result should parse as a font");

    assert_eq!(
        before.maxp().unwrap().num_glyphs(),
        after.maxp().unwrap().num_glyphs(),
        "glyph count must not change"
    );

    // Advances are a property of hmtx, which overlap removal does not touch.
    let before_metrics = before.glyph_metrics(Size::unscaled(), LocationRef::default());
    let after_metrics = after.glyph_metrics(Size::unscaled(), LocationRef::default());
    for gid in 0..before.maxp().unwrap().num_glyphs() {
        let gid = GlyphId::new(gid as u32);
        assert_eq!(
            before_metrics.advance_width(gid),
            after_metrics.advance_width(gid),
            "advance changed for glyph {}",
            gid.to_u32()
        );
    }
}

#[test]
fn overlap_removal_refuses_a_variable_font() {
    // gvar deltas are indexed by point number, and merging contours renumbers points, so
    // this has to be refused rather than silently producing a corrupt font.
    let error = remove_overlaps(RECURSIVE_VF).unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("static"),
        "expected a message about needing a static font, got: {message}"
    );
}

/// Pin the CFF2 fixture's only axis.
fn cff2_instance(weight: f64) -> Vec<u8> {
    let slice_font = SliceFont::load(CFF2_VF.to_vec()).unwrap();
    let font = slice_font.font_ref().unwrap();
    let axes = slice_font.axes().unwrap();
    let limits = vec![AxisLimit::Pin(weight); axes.len()];
    let location = normalize_location(&font, &axes, &[weight]);
    let plans = plan_axes(&font, &axes, &limits);
    instantiate_static(&font, &location, &plans).unwrap()
}

#[test]
fn the_filled_region_survives_overlap_removal_on_cff2() {
    // The `H` in this fixture has its crossbar overlapping both stems, so merging it is
    // a real union rather than a no-op. Everything downstream of the union is different
    // from the glyf path -- the merged cubics go straight into a charstring with no
    // quadratic refit -- so the invariant has to be checked separately.
    let instanced = cff2_instance(700.0);
    let (simplified, report) = remove_overlaps(&instanced).expect("overlap removal should run");
    println!("{}", report.summary());
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert!(
        !report.modified.is_empty(),
        "the fixture has overlapping contours; a run that changed nothing is not \
         evidence of anything"
    );

    let before_font = FontRef::new(&instanced).unwrap();
    let after_font = FontRef::new(&simplified).expect("the result should parse");
    assert!(
        after_font.cff2().is_ok() && after_font.glyf().is_err(),
        "overlap removal must not change the outline format"
    );

    let glyph_count = before_font.maxp().unwrap().num_glyphs();
    let mut mismatches = Vec::new();
    for gid in 0..glyph_count {
        let gid = GlyphId::new(gid as u32);
        let before = contours(&before_font, gid);
        let after = contours(&after_font, gid);
        if before.is_empty() && after.is_empty() {
            continue;
        }
        let bounds = before
            .iter()
            .map(|p| p.bounding_box())
            .reduce(|a, b| a.union(b))
            .unwrap_or_default();
        if bounds.width() <= 0.0 || bounds.height() <= 0.0 {
            continue;
        }
        const STEPS: i32 = 60;
        for i in 0..=STEPS {
            for j in 0..=STEPS {
                let point = Point::new(
                    bounds.x0 - 4.0 + (bounds.width() + 8.0) * (i as f64 / STEPS as f64),
                    bounds.y0 - 4.0 + (bounds.height() + 8.0) * (j as f64 / STEPS as f64),
                );
                if filled(&before, point) == filled(&after, point) {
                    continue;
                }
                // CFF keeps fractional coordinates, so the only edge slack here is the
                // boolean solver's own accuracy rather than a refit plus rounding.
                if distance_to_outline(&before, point) < 0.5
                    || distance_to_outline(&after, point) < 0.5
                {
                    continue;
                }
                mismatches.push((gid.to_u32(), point));
            }
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} sample points changed fill state away from any edge; first few: {:?}",
        mismatches.len(),
        &mismatches[..mismatches.len().min(5)]
    );
}

#[test]
fn overlap_removal_refuses_a_variable_cff2_font() {
    // A CFF2 charstring's blends are stated against the variation store, and merging
    // contours throws the operators that carry them away. Refusing is the only honest
    // answer; producing a font with a store nothing indexes is not.
    let error = remove_overlaps(CFF2_VF).unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("static"),
        "expected a message about needing a static font, got: {message}"
    );
}
