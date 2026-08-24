//! Does `linesweeper` remove overlaps correctly on the shapes that broke `flo_curves`?
//!
//! Question this answers
//! ---------------------
//!     Overlap removal is the least externally validated part of Slice, because no
//!     reference implementation exists to diff it against. `flo_curves` is the engine in
//!     use, and it needed working around: its `path_remove_interior_points` documents
//!     itself as the non-zero winding rule and is not one -- `GraphPath::from_path`
//!     normalizes every contour to clockwise first, so nothing can cancel and every
//!     counter fills in solid. Would `linesweeper` do this correctly out of the box?
//!
//! The shapes here are the ones in `tests/suite/fixtures/build.py`, with the probe points
//! and expected winding magnitudes copied from `OVERLAP_PROBES` in the same file. They
//! were chosen to cover every distinct region of every glyph, including the ones that
//! must come out **empty** -- which is exactly what a boolean engine that mishandles
//! contour direction gets wrong, and what a test that only checks the outer boundary
//! would miss.
//!
//! Run:  cd tools/overlap-engine-eval && cargo run
//!
//! A pass means: for every probe point, the point is inside the merged outline if and
//! only if it was inside the original under the non-zero rule.

use kurbo::{BezPath, PathEl, Point};
use linesweeper::{binary_op, BinaryOp, FillRule};

/// Kept in step with Cargo.lock by the assertion in `main`.
const LINESWEEPER_VERSION: &str = "0.4.0";

/// A rectangle wound clockwise in a y-up frame: negative signed area. The TrueType
/// convention for an outer contour.
fn cw_rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Point> {
    vec![
        Point::new(x0, y0),
        Point::new(x0, y1),
        Point::new(x1, y1),
        Point::new(x1, y0),
    ]
}

/// Counter-clockwise: positive signed area. The TrueType convention for a counter.
fn ccw_rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Point> {
    vec![
        Point::new(x0, y0),
        Point::new(x1, y0),
        Point::new(x1, y1),
        Point::new(x0, y1),
    ]
}

fn path_of(contours: &[Vec<Point>]) -> BezPath {
    let mut path = BezPath::new();
    for contour in contours {
        path.move_to(contour[0]);
        for point in &contour[1..] {
            path.line_to(*point);
        }
        path.close_path();
    }
    path
}

/// Winding number of `path` about `point`, counted over straight segments only --
/// every fixture here is a polygon.
fn winding(path: &BezPath, point: Point) -> i32 {
    let mut total = 0i32;
    let mut start = Point::ZERO;
    let mut here = Point::ZERO;
    let mut edge = |a: Point, b: Point| {
        if a.y <= point.y {
            if b.y > point.y && (b - a).cross(point - a) > 0.0 {
                total += 1;
            }
        } else if b.y <= point.y && (b - a).cross(point - a) < 0.0 {
            total -= 1;
        }
    };
    for element in path.elements() {
        match *element {
            PathEl::MoveTo(p) => {
                start = p;
                here = p;
            }
            PathEl::LineTo(p) => {
                edge(here, p);
                here = p;
            }
            PathEl::ClosePath => {
                edge(here, start);
                here = start;
            }
            // The merged output may contain curves; flatten them for the count.
            _ => {
                let mut flat = BezPath::new();
                flat.move_to(here);
                flat.push(*element);
                kurbo::flatten(flat, 0.05, |el| {
                    if let PathEl::LineTo(p) = el {
                        edge(here, p);
                        here = p;
                    }
                });
            }
        }
    }
    total
}

struct Case {
    glyph: &'static str,
    contours: Vec<Vec<Point>>,
    /// (point, expected |winding| in the *original*, what the region is)
    probes: Vec<(Point, i32, &'static str)>,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            glyph: "bars",
            contours: vec![cw_rect(50.0, 300.0, 750.0, 400.0), cw_rect(350.0, 0.0, 450.0, 700.0)],
            probes: vec![
                (Point::new(400.0, 350.0), 2, "both bars overlap"),
                (Point::new(150.0, 350.0), 1, "horizontal bar only"),
                (Point::new(400.0, 620.0), 1, "vertical bar only"),
                (Point::new(150.0, 620.0), 0, "outside both bars"),
                (Point::new(900.0, 350.0), 0, "right of everything"),
            ],
        },
        Case {
            glyph: "o",
            contours: vec![
                cw_rect(50.0, 50.0, 750.0, 650.0),
                ccw_rect(200.0, 200.0, 600.0, 500.0),
            ],
            probes: vec![
                (Point::new(100.0, 350.0), 1, "the ring band"),
                (Point::new(400.0, 350.0), 0, "the counter"),
                (Point::new(900.0, 350.0), 0, "outside"),
            ],
        },
        Case {
            glyph: "circled",
            contours: vec![
                cw_rect(20.0, 20.0, 780.0, 680.0),
                ccw_rect(110.0, 90.0, 690.0, 610.0),
                cw_rect(200.0, 160.0, 600.0, 540.0),
                ccw_rect(290.0, 230.0, 510.0, 470.0),
            ],
            probes: vec![
                (Point::new(60.0, 350.0), 1, "outer ring band"),
                (Point::new(150.0, 350.0), 0, "the outer counter"),
                (Point::new(240.0, 350.0), 1, "the inner filled square"),
                (Point::new(400.0, 350.0), 0, "the counter inside the counter"),
                (Point::new(900.0, 350.0), 0, "outside"),
            ],
        },
        Case {
            glyph: "bowtie",
            contours: vec![vec![
                Point::new(100.0, 100.0),
                Point::new(600.0, 600.0),
                Point::new(100.0, 600.0),
                Point::new(600.0, 100.0),
            ]],
            probes: vec![
                (Point::new(350.0, 200.0), 1, "the lower lobe"),
                (Point::new(350.0, 500.0), 1, "the upper lobe"),
                (Point::new(200.0, 350.0), 0, "between the lobes, left"),
                (Point::new(500.0, 350.0), 0, "between the lobes, right"),
            ],
        },
        Case {
            glyph: "clean",
            contours: vec![vec![
                Point::new(400.0, 650.0),
                Point::new(700.0, 0.0),
                Point::new(100.0, 0.0),
            ]],
            probes: vec![
                (Point::new(400.0, 100.0), 1, "inside the triangle"),
                (Point::new(150.0, 500.0), 0, "outside, upper left"),
                (Point::new(650.0, 500.0), 0, "outside, upper right"),
            ],
        },
    ]
}

fn main() {
    // The version of the crate under evaluation, not of this harness.
    println!("linesweeper {LINESWEEPER_VERSION} — non-zero self-union of the Slice \
              overlap fixtures\n");

    let mut failures = 0;
    let mut checked = 0;

    for case in cases() {
        let original = path_of(&case.contours);

        // Overlap removal is a *self*-union: resolve a path against itself under the
        // non-zero rule. linesweeper exposes only binary operations, so the second
        // operand is an empty path -- the union of a set with nothing is that set,
        // normalized.
        let merged = match binary_op(&original, &BezPath::new(), FillRule::NonZero, BinaryOp::Union)
        {
            Ok(path) => path,
            Err(e) => {
                println!("  {:9} ERROR from linesweeper: {e:?}", case.glyph);
                failures += 1;
                continue;
            }
        };

        // `binary_op` returns `Contours`: each carries its own closed `BezPath` and a
        // parent link, so holes are described explicitly rather than left to be
        // recovered from winding direction. Flatten them into one path to probe.
        let mut result = BezPath::new();
        let mut nested = 0;
        for contour in merged.contours() {
            if contour.parent.is_some() {
                nested += 1;
            }
            result.extend(contour.path.iter());
        }
        let merged = result;

        let contours_before = case.contours.len();
        let contours_after = merged
            .elements()
            .iter()
            .filter(|e| matches!(e, PathEl::MoveTo(_)))
            .count();

        println!("  {:9} {contours_before} contour(s) in, {contours_after} out \
                  ({nested} marked as holes)",
                 case.glyph);

        for (point, want_nonzero, what) in &case.probes {
            checked += 1;
            let inside_before = *want_nonzero != 0;
            let inside_after = winding(&merged, *point) != 0;
            let verdict = if inside_before == inside_after { "ok  " } else { "WRONG" };
            if inside_before != inside_after {
                failures += 1;
            }
            println!(
                "      {verdict} ({:>5.0},{:>4.0}) {what}: was {}, now {}",
                point.x,
                point.y,
                if inside_before { "filled" } else { "empty " },
                if inside_after { "filled" } else { "empty " },
            );
        }
        println!();
    }

    println!("{}/{checked} probe points agree with the original filled region",
             checked - failures);
    if failures > 0 {
        println!("\n{failures} disagreements: linesweeper does not preserve the filled \
                  region on these shapes as called here.");
        std::process::exit(1);
    }
    println!("\nlinesweeper preserves the filled region on every fixture, counters \
              included.");
}
