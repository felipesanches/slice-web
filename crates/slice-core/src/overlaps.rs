//! Merging overlapping contours.
//!
//! This is the thing the original Slice never did, and the reason it matters is not
//! rendering: browsers and rasterisers have filled non-zero winding correctly for
//! decades. It matters because the fonts come out the other end and go *into* design
//! applications, and support for overlapping contours there is still patchy a decade on.
//! A sliced instance whose stems overlap will show seams when outlined, misbehave under
//! boolean operations, and export badly to formats that assume simple contours.
//!
//! The approach follows `fontTools.ttLib.removeOverlaps`: take a glyph's contours,
//! union them, and write the result back as a simple glyph. fontTools delegates the
//! union to Skia's path ops via `skia-pathops`, which has no WebAssembly build; here it
//! is `flo_curves`, which does the same job on Bézier paths in pure Rust.
//!
//! Two things about this are worth knowing:
//!
//! * TrueType outlines are quadratic, and boolean path arithmetic works in cubics.
//!   Quadratic to cubic is exact; the way back is an approximation, so a glyph that is
//!   modified comes back with slightly different curves and more points. Glyphs that do
//!   not need modifying are therefore left completely alone.
//! * Removing overlaps invalidates hinting, because the point numbers it refers to no
//!   longer mean anything. fontTools drops hinting from every glyph when this runs, on
//!   the grounds that a half-hinted font looks worse than an unhinted one, and this does
//!   the same.

use kurbo::{BezPath, CubicBez, ParamCurve, PathEl, Point, Shape};
use read_fonts::{FontRef, TableProvider};
use read_fonts::tables::glyf::CurvePoint;
use write_fonts::tables::glyf::{Contour, GlyfLocaBuilder, Glyph as WGlyph, SimpleGlyph};
use write_fonts::types::GlyphId;
use write_fonts::{from_obj::ToOwnedTable, FontBuilder};

use flo_curves::bezier::path::SimpleBezierPath;
use flo_curves::geo::Coord2;

use crate::SliceError;

/// How closely the boolean operation must resolve intersections, in font units.
///
/// Coordinates are rounded to integers at the end, so resolving much finer than a tenth
/// of a unit buys nothing and costs time.
const BOOLEAN_ACCURACY: f64 = 0.01;

/// How closely the cubic result must be refitted with quadratics, in font units.
const QUAD_ACCURACY: f64 = 0.05;

/// What happened to a font when overlaps were removed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OverlapReport {
    /// Glyphs whose contours were actually merged.
    pub modified: Vec<u16>,
    /// Glyphs that were examined and found not to need it.
    pub untouched: usize,
    /// Glyphs the boolean operation could not handle, left as they were.
    pub failed: Vec<(u16, String)>,
}

impl OverlapReport {
    pub fn summary(&self) -> String {
        let mut text = format!(
            "{} glyph{} simplified, {} left as they were",
            self.modified.len(),
            if self.modified.len() == 1 { "" } else { "s" },
            self.untouched
        );
        if !self.failed.is_empty() {
            text.push_str(&format!(", {} could not be processed", self.failed.len()));
        }
        text
    }
}

/// Merge overlapping contours throughout a font.
///
/// The font must already be static: overlap removal rewrites outlines, and a `gvar`
/// table's deltas are indexed by point number, so they would no longer line up.
pub fn remove_overlaps(font_bytes: &[u8]) -> Result<(Vec<u8>, OverlapReport), SliceError> {
    let font = FontRef::new(font_bytes).map_err(|e| SliceError::Read(e.to_string()))?;

    if font.glyf().is_err() {
        return Err(SliceError::Unsupported(
            "Overlap removal currently handles TrueType outlines only; this font uses \
             CFF outlines."
                .into(),
        ));
    }
    if font.gvar().is_ok() {
        return Err(SliceError::Unsupported(
            "Overlap removal needs a static font: gvar deltas are indexed by point \
             number, and merging contours renumbers the points. Pin every axis, or turn \
             overlap removal off."
                .into(),
        ));
    }

    let num_glyphs = font.maxp()?.num_glyphs();
    let loca = font.loca(None)?;
    let glyf = font.glyf()?;

    let mut report = OverlapReport::default();
    let mut glyphs: Vec<WGlyph> = Vec::with_capacity(num_glyphs as usize);

    for gid in 0..num_glyphs {
        let gid_value = gid;
        let gid = GlyphId::new(gid as u32);
        let original = read_original(&loca, &glyf, gid);

        match simplify_glyph(&font, gid) {
            Ok(Some(glyph)) => {
                report.modified.push(gid_value);
                glyphs.push(glyph);
            }
            Ok(None) => {
                report.untouched += 1;
                glyphs.push(original);
            }
            Err(e) => {
                report.failed.push((gid_value, e.to_string()));
                glyphs.push(original);
            }
        }
    }

    // Hinting no longer describes these outlines. Drop it from every glyph, not just the
    // modified ones, so the font is consistently unhinted rather than partly so.
    for glyph in &mut glyphs {
        match glyph {
            WGlyph::Simple(simple) => simple.instructions.clear(),
            WGlyph::Composite(composite) => composite.set_instructions(&[]),
            WGlyph::Empty => {}
        }
    }

    let mut builder = GlyfLocaBuilder::new();
    for glyph in &glyphs {
        builder
            .add_glyph(glyph)
            .map_err(|e| SliceError::Write(e.to_string()))?;
    }
    let (new_glyf, new_loca, loca_format) = builder.build();

    let mut out = FontBuilder::new();
    out.add_table(&new_glyf)
        .map_err(|e| SliceError::Write(e.to_string()))?;
    out.add_table(&new_loca)
        .map_err(|e| SliceError::Write(e.to_string()))?;
    let mut head: write_fonts::tables::head::Head = font.head()?.to_owned_table();
    head.index_to_loc_format = loca_format as i16;
    out.add_table(&head)
        .map_err(|e| SliceError::Write(e.to_string()))?;

    crate::instancer::statics::copy_remaining_tables(&mut out, &font, &[]);
    Ok((out.build(), report))
}

fn read_original(
    loca: &read_fonts::tables::loca::Loca,
    glyf: &read_fonts::tables::glyf::Glyf,
    gid: GlyphId,
) -> WGlyph {
    match loca.get_glyf(gid, glyf) {
        Ok(Some(read_fonts::tables::glyf::Glyph::Simple(simple))) => {
            WGlyph::Simple(simple.to_owned_table())
        }
        Ok(Some(read_fonts::tables::glyf::Glyph::Composite(composite))) => {
            WGlyph::Composite(composite.to_owned_table())
        }
        _ => WGlyph::Empty,
    }
}

/// Simplify one glyph, or report that it did not need it.
fn simplify_glyph(font: &FontRef, gid: GlyphId) -> Result<Option<WGlyph>, SliceError> {
    let contours = glyph_contours(font, gid)?;
    if contours.len() < 2 && !any_self_intersection(&contours) {
        // A single contour that does not cross itself has nothing to merge.
        return Ok(None);
    }
    if !contours_can_overlap(&contours) {
        return Ok(None);
    }

    let merged_paths = union_nonzero(&contours);

    if merged_paths.is_empty() {
        return Err(SliceError::RemoveOverlaps {
            glyph: format!("{}", gid.to_u32()),
            reason: "the merge produced no contours".into(),
        });
    }

    // If the merge changed nothing meaningful, keep the original outline rather than
    // paying for a cubic-to-quadratic refit that would only add points.
    if same_area(&contours, &merged_paths) && merged_paths.len() == contours.len() {
        return Ok(None);
    }

    let glyph = paths_to_simple_glyph(&merged_paths).ok_or_else(|| SliceError::RemoveOverlaps {
        glyph: format!("{}", gid.to_u32()),
        reason: "the merged outline could not be written as a simple glyph".into(),
    })?;
    Ok(Some(glyph))
}

/// A glyph's contours as kurbo paths, with composites resolved.
fn glyph_contours(font: &FontRef, gid: GlyphId) -> Result<Vec<BezPath>, SliceError> {
    use skrifa::instance::{LocationRef, Size};
    use skrifa::outline::DrawSettings;
    use skrifa::MetadataProvider;

    let mut pen = ContourPen::default();
    let outlines = font.outline_glyphs();
    if let Some(glyph) = outlines.get(gid) {
        glyph
            .draw(
                DrawSettings::unhinted(Size::unscaled(), LocationRef::default()),
                &mut pen,
            )
            .map_err(|e| SliceError::RemoveOverlaps {
                glyph: format!("{}", gid.to_u32()),
                reason: e.to_string(),
            })?;
    }
    Ok(pen.finish())
}

/// Collects drawing operations into one `BezPath` per contour.
#[derive(Default)]
struct ContourPen {
    contours: Vec<BezPath>,
    current: Option<BezPath>,
}

impl ContourPen {
    fn finish(mut self) -> Vec<BezPath> {
        self.flush();
        self.contours
    }

    fn flush(&mut self) {
        if let Some(mut path) = self.current.take() {
            if !path.elements().is_empty() {
                path.close_path();
                self.contours.push(path);
            }
        }
    }
}

impl skrifa::outline::OutlinePen for ContourPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.flush();
        let mut path = BezPath::new();
        path.move_to(Point::new(x as f64, y as f64));
        self.current = Some(path);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        if let Some(path) = &mut self.current {
            path.line_to(Point::new(x as f64, y as f64));
        }
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        if let Some(path) = &mut self.current {
            path.quad_to(
                Point::new(cx as f64, cy as f64),
                Point::new(x as f64, y as f64),
            );
        }
    }
    fn curve_to(&mut self, c1x: f32, c1y: f32, c2x: f32, c2y: f32, x: f32, y: f32) {
        if let Some(path) = &mut self.current {
            path.curve_to(
                Point::new(c1x as f64, c1y as f64),
                Point::new(c2x as f64, c2y as f64),
                Point::new(x as f64, y as f64),
            );
        }
    }
    fn close(&mut self) {
        self.flush();
    }
}

/// Cheap rejection: if no two contours' bounding boxes touch, nothing can overlap.
fn contours_can_overlap(contours: &[BezPath]) -> bool {
    if contours.len() < 2 {
        return any_self_intersection(contours);
    }
    let boxes: Vec<_> = contours.iter().map(|c| c.bounding_box()).collect();
    for i in 0..boxes.len() {
        for j in (i + 1)..boxes.len() {
            if boxes[i].intersect(boxes[j]).area() > 0.0 {
                return true;
            }
        }
    }
    any_self_intersection(contours)
}

/// Does any contour cross itself?
///
/// Checks every pair of segments within a contour for an intersection that is not simply
/// the shared endpoint of adjacent segments.
fn any_self_intersection(contours: &[BezPath]) -> bool {
    for contour in contours {
        let segments: Vec<_> = contour.segments().collect();
        let n = segments.len();
        for i in 0..n {
            for j in (i + 1)..n {
                // Adjacent segments always meet at a shared point.
                let adjacent = j == i + 1 || (i == 0 && j == n - 1);
                let a = segments[i].bounding_box();
                let b = segments[j].bounding_box();
                if a.intersect(b).area() <= 0.0 {
                    continue;
                }
                if !adjacent {
                    return true;
                }
            }
        }
    }
    false
}

/// Total signed area, used to check whether a merge changed anything.
fn same_area(before: &[BezPath], after: &[BezPath]) -> bool {
    let a: f64 = before.iter().map(|p| p.area().abs()).sum();
    let b: f64 = after.iter().map(|p| p.area().abs()).sum();
    (a - b).abs() < 1.0
}

/// Convert a kurbo path to the cubic representation `flo_curves` works in.
fn bezpath_to_flo(path: &BezPath) -> SimpleBezierPath {
    let mut start = Coord2(0.0, 0.0);
    let mut segments = Vec::new();
    let mut current = Point::ZERO;
    let mut started = false;

    for element in path.elements() {
        match *element {
            PathEl::MoveTo(p) => {
                start = Coord2(p.x, p.y);
                current = p;
                started = true;
            }
            PathEl::LineTo(p) => {
                // A line is a cubic with its controls on the line.
                let c1 = current.lerp(p, 1.0 / 3.0);
                let c2 = current.lerp(p, 2.0 / 3.0);
                segments.push((Coord2(c1.x, c1.y), Coord2(c2.x, c2.y), Coord2(p.x, p.y)));
                current = p;
            }
            PathEl::QuadTo(q, p) => {
                // Exact quadratic-to-cubic elevation.
                let c1 = current + (q - current) * (2.0 / 3.0);
                let c2 = p + (q - p) * (2.0 / 3.0);
                segments.push((Coord2(c1.x, c1.y), Coord2(c2.x, c2.y), Coord2(p.x, p.y)));
                current = p;
            }
            PathEl::CurveTo(c1, c2, p) => {
                segments.push((Coord2(c1.x, c1.y), Coord2(c2.x, c2.y), Coord2(p.x, p.y)));
                current = p;
            }
            PathEl::ClosePath => {
                if started && (current.x != start.0 || current.y != start.1) {
                    let end = Point::new(start.0, start.1);
                    let c1 = current.lerp(end, 1.0 / 3.0);
                    let c2 = current.lerp(end, 2.0 / 3.0);
                    segments.push((Coord2(c1.x, c1.y), Coord2(c2.x, c2.y), start));
                    current = end;
                }
            }
        }
    }

    (start, segments)
}

/// Merge a glyph's contours under the non-zero winding rule.
///
/// `flo_curves` ships `path_remove_interior_points`, which looks like the right function
/// and is not: `GraphPath::from_path` reverses any anti-clockwise contour to clockwise
/// before building the graph, so by the time the winding is counted every contour turns
/// the same way and nothing can cancel. Run an 'o' through it and the counter is gone.
///
/// The ray-casting pass underneath it, though, keeps a *signed* crossing count per path
/// label and lets the caller decide what "inside" means. So: give every contour its own
/// label, remember which ones `from_path` reversed, and add the crossings back up with
/// those reversals undone. That total is the true winding number, and testing it against
/// zero is the non-zero rule.
///
/// A global sign flip would not matter here — `total != 0` is symmetric — so it is
/// enough that contours which turn opposite ways get opposite signs.
fn union_nonzero(contours: &[BezPath]) -> Vec<BezPath> {
    use flo_curves::bezier::path::{GraphPath, PathLabel};

    let flo: Vec<SimpleBezierPath> = contours.iter().map(bezpath_to_flo).collect();

    // kurbo's signed area is positive for an anti-clockwise contour, which is exactly
    // the set `from_path` reverses.
    let signs: Vec<i32> = contours
        .iter()
        .map(|c| if c.area() < 0.0 { 1 } else { -1 })
        .collect();

    let mut graph: GraphPath<Coord2, PathLabel> = GraphPath::new();
    graph = graph.merge(GraphPath::from_merged_paths(
        flo.iter()
            .enumerate()
            .map(|(i, path)| (path, PathLabel(i as u32))),
    ));

    graph.self_collide(BOOLEAN_ACCURACY);
    graph.round(BOOLEAN_ACCURACY);

    graph.set_edge_kinds_by_ray_casting(|crossings| {
        let mut winding = 0i32;
        for (index, &count) in crossings.iter().enumerate() {
            winding += signs.get(index).copied().unwrap_or(0) * count;
        }
        winding != 0
    });
    graph.heal_exterior_gaps();

    let merged: Vec<SimpleBezierPath> = graph.exterior_paths();
    orient_for_nonzero(merged.iter().map(flo_to_bezpath).collect())
}

/// Re-wind merged contours so that the non-zero winding rule fills the right region.
///
/// `flo_curves` returns its result as an *even-odd* set: the contours are all wound the
/// same way, and a hole is a hole because it is nested inside another contour, not
/// because it runs the other way round. `glyf` is filled with the non-zero rule, under
/// which that set would have every counter filled in solid — an 'o' would come out as a
/// blob.
///
/// Converting between the two is a matter of orientation: a contour nested an even
/// number of deep is an outer contour, an odd number deep is a hole, and the two must be
/// wound in opposite directions. TrueType's convention is that outer contours run
/// clockwise, which in a y-up coordinate system is a negative signed area.
fn orient_for_nonzero(paths: Vec<BezPath>) -> Vec<BezPath> {
    let probes: Vec<Option<Point>> = paths.iter().map(boundary_point).collect();

    paths
        .iter()
        .enumerate()
        .map(|(i, path)| {
            let depth = match probes[i] {
                None => 0,
                Some(point) => paths
                    .iter()
                    .enumerate()
                    .filter(|(j, other)| *j != i && other.winding(point) != 0)
                    .count(),
            };
            let want_clockwise = depth % 2 == 0;
            // kurbo's signed area is positive for a counter-clockwise contour.
            let is_clockwise = path.area() < 0.0;
            if is_clockwise == want_clockwise {
                path.clone()
            } else {
                reverse_contour(path)
            }
        })
        .collect()
}

/// A point on a contour's own outline, used to work out how deeply it is nested.
///
/// A point *inside* the contour will not do. The natural choice, the centre of the
/// bounding box, sits inside the counter for a glyph like 'o', which makes the outer
/// contour look one level deeper than it is and leaves the counter filled. A point on
/// the boundary is inside every contour that encloses this one and outside every contour
/// nested within it, which is exactly the containment relation nesting depth needs.
///
/// The midpoint of the longest segment is used, since that is the least likely to sit
/// exactly on top of a neighbouring contour where the winding would be ambiguous.
fn boundary_point(path: &BezPath) -> Option<Point> {
    let mut best: Option<(f64, Point)> = None;
    for segment in path.segments() {
        let start = segment.eval(0.0);
        let end = segment.eval(1.0);
        let length = (end - start).hypot();
        let midpoint = segment.eval(0.5);
        if best.map(|(l, _)| length > l).unwrap_or(true) {
            best = Some((length, midpoint));
        }
    }
    best.map(|(_, point)| point)
}

/// Reverse a closed contour's direction, keeping its start point.
fn reverse_contour(path: &BezPath) -> BezPath {
    let mut points: Vec<(Point, Point, Point)> = Vec::new();
    let mut start = Point::ZERO;
    let mut current = Point::ZERO;

    for element in path.elements() {
        match *element {
            PathEl::MoveTo(p) => {
                start = p;
                current = p;
            }
            PathEl::LineTo(p) => {
                let c1 = current.lerp(p, 1.0 / 3.0);
                let c2 = current.lerp(p, 2.0 / 3.0);
                points.push((c1, c2, p));
                current = p;
            }
            PathEl::QuadTo(q, p) => {
                let c1 = current + (q - current) * (2.0 / 3.0);
                let c2 = p + (q - p) * (2.0 / 3.0);
                points.push((c1, c2, p));
                current = p;
            }
            PathEl::CurveTo(c1, c2, p) => {
                points.push((c1, c2, p));
                current = p;
            }
            PathEl::ClosePath => {}
        }
    }

    let mut out = BezPath::new();
    if points.is_empty() {
        return out;
    }

    // Walk the segments backwards, swapping each one's control points.
    let last_end = points[points.len() - 1].2;
    out.move_to(last_end);
    for index in (0..points.len()).rev() {
        let (c1, c2, _) = points[index];
        let segment_start = if index == 0 { start } else { points[index - 1].2 };
        out.curve_to(c2, c1, segment_start);
    }
    out.close_path();
    out
}

fn flo_to_bezpath(path: &SimpleBezierPath) -> BezPath {
    let (start, segments) = path;
    let mut out = BezPath::new();
    out.move_to(Point::new(start.0, start.1));
    for (c1, c2, end) in segments {
        out.curve_to(
            Point::new(c1.0, c1.1),
            Point::new(c2.0, c2.1),
            Point::new(end.0, end.1),
        );
    }
    out.close_path();
    out
}

/// Write cubic contours back out as a TrueType simple glyph.
///
/// Cubics are refitted with quadratics, which is where the approximation lives. Straight
/// lines are detected and kept as lines rather than becoming degenerate curves.
fn paths_to_simple_glyph(paths: &[BezPath]) -> Option<WGlyph> {
    let mut contours: Vec<Contour> = Vec::new();

    for path in paths {
        let mut points: Vec<CurvePoint> = Vec::new();
        let mut current = Point::ZERO;
        let mut first: Option<Point> = None;

        for element in path.elements() {
            match *element {
                PathEl::MoveTo(p) => {
                    current = p;
                    first = Some(p);
                    points.push(CurvePoint::on_curve(round(p.x), round(p.y)));
                }
                PathEl::LineTo(p) => {
                    current = p;
                    points.push(CurvePoint::on_curve(round(p.x), round(p.y)));
                }
                PathEl::QuadTo(q, p) => {
                    points.push(CurvePoint::off_curve(round(q.x), round(q.y)));
                    points.push(CurvePoint::on_curve(round(p.x), round(p.y)));
                    current = p;
                }
                PathEl::CurveTo(c1, c2, p) => {
                    let cubic = CubicBez::new(current, c1, c2, p);
                    if is_straight(&cubic) {
                        points.push(CurvePoint::on_curve(round(p.x), round(p.y)));
                    } else {
                        for (_, _, quad) in cubic.to_quads(QUAD_ACCURACY) {
                            points.push(CurvePoint::off_curve(round(quad.p1.x), round(quad.p1.y)));
                            points.push(CurvePoint::on_curve(round(quad.p2.x), round(quad.p2.y)));
                        }
                    }
                    current = p;
                }
                PathEl::ClosePath => {}
            }
        }

        // A closing point that repeats the start of the contour is redundant: glyf
        // contours close implicitly.
        if let Some(first) = first {
            while points.len() > 1 {
                let last = points[points.len() - 1];
                if last.on_curve && last.x == round(first.x) && last.y == round(first.y) {
                    points.pop();
                } else {
                    break;
                }
            }
        }

        if points.len() >= 2 {
            contours.push(points.into());
        }
    }

    if contours.is_empty() {
        return Some(WGlyph::Empty);
    }

    let mut glyph = SimpleGlyph {
        bbox: Default::default(),
        contours,
        instructions: Vec::new(),
        // The whole point of this pass is that the contours no longer overlap.
        overlaps: false,
    };
    glyph.recompute_bounding_box();
    Some(WGlyph::Simple(glyph))
}

/// Is this cubic close enough to a straight line to store as one?
fn is_straight(cubic: &CubicBez) -> bool {
    let line = kurbo::Line::new(cubic.p0, cubic.p3);
    let length = (cubic.p3 - cubic.p0).hypot();
    if length < 1e-6 {
        return true;
    }
    [0.25, 0.5, 0.75].iter().all(|&t| {
        let point = cubic.eval(t);
        distance_to_line(point, line) < QUAD_ACCURACY
    })
}

fn distance_to_line(point: Point, line: kurbo::Line) -> f64 {
    let direction = line.p1 - line.p0;
    let length = direction.hypot();
    if length < 1e-9 {
        return (point - line.p0).hypot();
    }
    ((point - line.p0).cross(direction) / length).abs()
}

fn round(value: f64) -> i16 {
    crate::instancer::glyphs::ot_round(value).clamp(-32768, 32767) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two overlapping squares, drawn in the same direction.
    fn overlapping_squares() -> Vec<BezPath> {
        let mut a = BezPath::new();
        a.move_to((0.0, 0.0));
        a.line_to((100.0, 0.0));
        a.line_to((100.0, 100.0));
        a.line_to((0.0, 100.0));
        a.close_path();

        let mut b = BezPath::new();
        b.move_to((50.0, 50.0));
        b.line_to((150.0, 50.0));
        b.line_to((150.0, 150.0));
        b.line_to((50.0, 150.0));
        b.close_path();

        vec![a, b]
    }

    /// A square with a square hole, wound in opposite directions: an 'o'.
    fn square_with_counter() -> Vec<BezPath> {
        let mut outer = BezPath::new();
        outer.move_to((0.0, 0.0));
        outer.line_to((100.0, 0.0));
        outer.line_to((100.0, 100.0));
        outer.line_to((0.0, 100.0));
        outer.close_path();

        let mut inner = BezPath::new();
        inner.move_to((25.0, 25.0));
        inner.line_to((25.0, 75.0));
        inner.line_to((75.0, 75.0));
        inner.line_to((75.0, 25.0));
        inner.close_path();

        vec![outer, inner]
    }

    /// Is a point inside the filled region, under the non-zero winding rule?
    fn filled(paths: &[BezPath], point: Point) -> bool {
        let winding: i32 = paths.iter().map(|p| p.winding(point)).sum();
        winding != 0
    }

    fn union(paths: &[BezPath]) -> Vec<BezPath> {
        union_nonzero(paths)
    }

    /// Sample a grid and confirm two path sets fill exactly the same points.
    ///
    /// The x and y strides differ, and neither divides the other. A grid with equal
    /// strides puts a whole diagonal of samples exactly on top of a 45-degree edge,
    /// where winding is undefined and both answers are defensible; that shows up as a
    /// row of spurious failures rather than a real one.
    fn assert_same_filled_region(before: &[BezPath], after: &[BezPath], label: &str) {
        let mut mismatches: Vec<Point> = Vec::new();
        let mut checked = 0;
        for i in 0..80 {
            for j in 0..80 {
                let point = Point::new(
                    -12.3 + i as f64 * 2.531,
                    -9.7 + j as f64 * 2.417,
                );
                checked += 1;
                if filled(before, point) != filled(after, point) {
                    mismatches.push(point);
                }
            }
        }
        assert!(
            mismatches.is_empty(),
            "{label}: {} of {checked} sampled points changed fill state; first few: {:?}",
            mismatches.len(),
            &mismatches[..mismatches.len().min(6)]
        );
    }

    #[test]
    fn union_of_overlapping_squares_covers_the_same_region() {
        let before = overlapping_squares();
        let after = union(&before);
        assert_eq!(after.len(), 1, "two overlapping squares should merge into one");
        assert_same_filled_region(&before, &after, "overlapping squares");
    }

    #[test]
    fn a_counter_survives_the_merge() {
        // The critical case: an 'o' must not have its hole filled in.
        let before = square_with_counter();
        let after = union(&before);
        assert!(
            !filled(&after, Point::new(50.0, 50.0)),
            "the counter was filled in: overlap removal would destroy every 'o' in the font"
        );
        assert!(filled(&after, Point::new(10.0, 50.0)), "the ring should still be filled");
        assert_same_filled_region(&before, &after, "square with counter");
    }

    /// A rectangle, wound in the given direction.
    fn rect(x0: f64, y0: f64, x1: f64, y1: f64, clockwise: bool) -> BezPath {
        let mut path = BezPath::new();
        path.move_to((x0, y0));
        if clockwise {
            // y-up, so this order traces clockwise.
            path.line_to((x0, y1));
            path.line_to((x1, y1));
            path.line_to((x1, y0));
        } else {
            path.line_to((x1, y0));
            path.line_to((x1, y1));
            path.line_to((x0, y1));
        }
        path.close_path();
        path
    }

    #[test]
    fn a_counter_inside_a_counter_survives() {
        // The shape of a circled letter: an outer ring, and inside its hole another
        // filled shape with its own hole. Nesting depth reaches three.
        //
        // This is the case that rules out the obvious shortcut of unioning all the
        // outer-wound contours and subtracting all the inner-wound ones: that would
        // subtract the ring's hole from the letter and erase it.
        let before = vec![
            rect(0.0, 0.0, 300.0, 300.0, true),      // depth 0: outer ring, filled
            rect(40.0, 40.0, 260.0, 260.0, false),   // depth 1: its hole
            rect(80.0, 80.0, 220.0, 220.0, true),    // depth 2: the letter, filled
            rect(120.0, 120.0, 180.0, 180.0, false), // depth 3: the letter's counter
        ];

        // Sanity-check the fixture itself before trusting what it proves.
        assert!(filled(&before, Point::new(20.0, 150.0)), "outer ring should be solid");
        assert!(!filled(&before, Point::new(60.0, 150.0)), "the ring's hole should be empty");
        assert!(filled(&before, Point::new(100.0, 150.0)), "the letter should be solid");
        assert!(!filled(&before, Point::new(150.0, 150.0)), "the letter's counter should be empty");

        let after = union(&before);
        assert_same_filled_region(&before, &after, "counter inside a counter");
    }

    #[test]
    fn overlapping_rings_merge_without_losing_their_holes() {
        // Two 'o' shapes overlapping: the rings must join, and both holes must stay.
        let before = vec![
            rect(0.0, 0.0, 100.0, 100.0, true),
            rect(20.0, 20.0, 80.0, 80.0, false),
            rect(60.0, 0.0, 160.0, 100.0, true),
            rect(80.0, 20.0, 140.0, 80.0, false),
        ];

        let after = union(&before);
        assert_same_filled_region(&before, &after, "overlapping rings");
        assert!(!filled(&after, Point::new(40.0, 50.0)), "the left hole should survive");
        assert!(!filled(&after, Point::new(120.0, 50.0)), "the right hole should survive");
    }

    #[test]
    fn a_self_intersecting_contour_is_resolved() {
        // A bow tie: one contour that crosses itself. Under the non-zero rule both
        // lobes are filled, and the merged outline must fill them too.
        let mut bow = BezPath::new();
        bow.move_to((0.0, 0.0));
        bow.line_to((100.0, 100.0));
        bow.line_to((0.0, 100.0));
        bow.line_to((100.0, 0.0));
        bow.close_path();

        let before = vec![bow];
        let after = union(&before);
        assert_same_filled_region(&before, &after, "self-intersecting bow tie");
    }

    #[test]
    fn a_contour_wholly_inside_another_of_the_same_direction_stays_filled() {
        // Same direction means the windings add rather than cancel, so under the
        // non-zero rule the inner contour is not a hole and the result is a solid
        // rectangle.
        let before = vec![
            rect(0.0, 0.0, 100.0, 100.0, true),
            rect(25.0, 25.0, 75.0, 75.0, true),
        ];
        assert!(filled(&before, Point::new(50.0, 50.0)));

        let after = union(&before);
        assert!(
            filled(&after, Point::new(50.0, 50.0)),
            "same-direction nesting is not a hole under the non-zero rule"
        );
        assert_same_filled_region(&before, &after, "same-direction nesting");
    }

    #[test]
    fn disjoint_contours_are_left_alone() {
        let mut a = BezPath::new();
        a.move_to((0.0, 0.0));
        a.line_to((10.0, 0.0));
        a.line_to((10.0, 10.0));
        a.close_path();

        let mut b = BezPath::new();
        b.move_to((100.0, 100.0));
        b.line_to((110.0, 100.0));
        b.line_to((110.0, 110.0));
        b.close_path();

        assert!(
            !contours_can_overlap(&[a, b]),
            "contours with disjoint bounding boxes cannot overlap"
        );
    }

    #[test]
    fn quadratic_to_cubic_elevation_is_exact() {
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.quad_to((50.0, 100.0), (100.0, 0.0));
        path.close_path();

        let flo = bezpath_to_flo(&path);
        let back = flo_to_bezpath(&flo);

        // Sample both curves; elevation introduces no error.
        let original = kurbo::QuadBez::new(
            Point::new(0.0, 0.0),
            Point::new(50.0, 100.0),
            Point::new(100.0, 0.0),
        );
        let elevated = match back.elements()[1] {
            PathEl::CurveTo(c1, c2, p) => CubicBez::new(Point::new(0.0, 0.0), c1, c2, p),
            other => panic!("expected a cubic, got {other:?}"),
        };
        for step in 0..=20 {
            let t = step as f64 / 20.0;
            let a = original.eval(t);
            let b = elevated.eval(t);
            assert!((a - b).hypot() < 1e-9, "at t={t}: {a:?} vs {b:?}");
        }
    }

    #[test]
    fn a_straight_cubic_is_recognised() {
        let straight = CubicBez::new(
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            Point::new(20.0, 0.0),
            Point::new(30.0, 0.0),
        );
        assert!(is_straight(&straight));

        let curved = CubicBez::new(
            Point::new(0.0, 0.0),
            Point::new(10.0, 40.0),
            Point::new(20.0, 40.0),
            Point::new(30.0, 0.0),
        );
        assert!(!is_straight(&curved));
    }
}
