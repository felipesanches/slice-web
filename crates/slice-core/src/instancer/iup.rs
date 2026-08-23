//! Interpolation of untouched points (IUP).
//!
//! A tuple variation does not have to carry a delta for every point in a glyph. Points
//! it leaves out are not "unmoved": their deltas are interpolated from the nearest
//! points on either side of them along the contour that *do* have one. Any code that
//! wants to apply a sparse tuple has to reproduce that interpolation, so this is a port
//! of `fontTools.varLib.iup.iup_contour` / `iup_segment`.
//!
//! The two axes are interpolated independently, which is why `iup_segment` loops over
//! `j in 0, 1` upstream and why the two coordinates are handled separately here.

/// A point's delta, or `None` when the tuple did not specify one.
pub type MaybeDelta = Option<(f64, f64)>;

/// Interpolate the deltas of one contour's untouched points.
///
/// `deltas` and `coords` describe the same contour and must be the same length. `coords`
/// are the glyph's original point coordinates; `deltas` are the explicit deltas, with
/// `None` where the tuple was silent.
pub fn iup_contour(deltas: &[MaybeDelta], coords: &[(f64, f64)]) -> Vec<(f64, f64)> {
    assert_eq!(deltas.len(), coords.len(), "deltas and coords must agree");
    let n = deltas.len();

    // Nothing to infer.
    if deltas.iter().all(Option::is_some) {
        return deltas.iter().map(|d| d.unwrap()).collect();
    }

    let indices: Vec<usize> = (0..n).filter(|&i| deltas[i].is_some()).collect();
    if indices.is_empty() {
        // The tuple says nothing about this contour at all, so it does not move.
        return vec![(0.0, 0.0); n];
    }

    let mut out: Vec<(f64, f64)> = Vec::with_capacity(n);
    let start = indices[0];

    if start != 0 {
        // The run before the first explicit point wraps around the end of the contour,
        // so it is bounded by the last explicit point and the first one.
        let last = *indices.last().unwrap();
        out.extend(iup_segment(
            &coords[0..start],
            coords[start],
            deltas[start].unwrap(),
            coords[last],
            deltas[last].unwrap(),
        ));
    }
    out.push(deltas[start].unwrap());

    let mut prev = start;
    for &end in &indices[1..] {
        if end - prev > 1 {
            out.extend(iup_segment(
                &coords[prev + 1..end],
                coords[prev],
                deltas[prev].unwrap(),
                coords[end],
                deltas[end].unwrap(),
            ));
        }
        out.push(deltas[end].unwrap());
        prev = end;
    }

    if prev != n - 1 {
        // The trailing run wraps around to the first explicit point.
        let first = indices[0];
        out.extend(iup_segment(
            &coords[prev + 1..n],
            coords[prev],
            deltas[prev].unwrap(),
            coords[first],
            deltas[first].unwrap(),
        ));
    }

    debug_assert_eq!(out.len(), n);
    out
}

/// Interpolate deltas for `coords`, between two reference points.
fn iup_segment(
    coords: &[(f64, f64)],
    rc1: (f64, f64),
    rd1: (f64, f64),
    rc2: (f64, f64),
    rd2: (f64, f64),
) -> Vec<(f64, f64)> {
    let mut xs = Vec::with_capacity(coords.len());
    let mut ys = Vec::with_capacity(coords.len());

    for axis in 0..2 {
        let out = if axis == 0 { &mut xs } else { &mut ys };
        let pick = |p: (f64, f64)| if axis == 0 { p.0 } else { p.1 };

        let (mut x1, mut x2) = (pick(rc1), pick(rc2));
        let (mut d1, mut d2) = (pick(rd1), pick(rd2));

        if x1 == x2 {
            // The two reference points sit at the same coordinate on this axis, so
            // there is no direction to interpolate along. If they agree on the delta,
            // everything between them takes it; if they disagree, there is no defensible
            // answer and the points stay put.
            let value = if d1 == d2 { d1 } else { 0.0 };
            out.extend(std::iter::repeat(value).take(coords.len()));
            continue;
        }

        if x1 > x2 {
            std::mem::swap(&mut x1, &mut x2);
            std::mem::swap(&mut d1, &mut d2);
        }

        let scale = (d2 - d1) / (x2 - x1);
        for &p in coords {
            let x = pick(p);
            let d = if x <= x1 {
                d1
            } else if x >= x2 {
                d2
            } else {
                // Upstream keeps the multiplication in its own variable to stop the
                // compiler fusing it into a multiply-add, which would change the result
                // in the last bit (fontTools#3703). Rust does not fuse these without
                // being asked, but the shape is kept so the two read the same.
                let nudge = (x - x1) * scale;
                d1 + nudge
            };
            out.push(d);
        }
    }

    xs.into_iter().zip(ys).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fully_specified_contour_is_returned_unchanged() {
        let coords = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)];
        let deltas = [Some((1.0, 2.0)), Some((3.0, 4.0)), Some((5.0, 6.0))];
        assert_eq!(
            iup_contour(&deltas, &coords),
            vec![(1.0, 2.0), (3.0, 4.0), (5.0, 6.0)]
        );
    }

    #[test]
    fn a_contour_with_no_deltas_does_not_move() {
        let coords = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)];
        let deltas = [None, None, None];
        assert_eq!(iup_contour(&deltas, &coords), vec![(0.0, 0.0); 3]);
    }

    #[test]
    fn a_single_delta_propagates_to_the_whole_contour() {
        // With one reference point, every segment is bounded by that point on both
        // sides, so x1 == x2 and d1 == d2: the whole contour shifts with it.
        let coords = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let deltas = [Some((5.0, 7.0)), None, None, None];
        assert_eq!(iup_contour(&deltas, &coords), vec![(5.0, 7.0); 4]);
    }

    #[test]
    fn midpoints_interpolate_proportionally() {
        // Three points in a row on the x axis: 0, 5, 10, with the ends touched.
        let coords = [(0.0, 0.0), (5.0, 0.0), (10.0, 0.0)];
        let deltas = [Some((0.0, 0.0)), None, Some((10.0, 0.0))];
        let out = iup_contour(&deltas, &coords);
        assert_eq!(out[1], (5.0, 0.0), "halfway along should take half the delta");
    }

    #[test]
    fn points_outside_the_reference_span_clamp() {
        // The untouched point sits beyond both references on x, so it takes the nearer
        // reference's delta rather than an extrapolation.
        let coords = [(0.0, 0.0), (20.0, 0.0), (10.0, 0.0)];
        let deltas = [Some((0.0, 0.0)), None, Some((4.0, 0.0))];
        let out = iup_contour(&deltas, &coords);
        assert_eq!(out[1].0, 4.0, "x=20 is past x=10, so it clamps to that delta");
    }

    #[test]
    fn disagreeing_deltas_at_one_coordinate_pin_the_span_to_zero() {
        // Both references sit at x = 0 but disagree, so x deltas in between go to zero;
        // on y they are at 0 and 10 and interpolate normally.
        let coords = [(0.0, 0.0), (0.0, 5.0), (0.0, 10.0)];
        let deltas = [Some((1.0, 0.0)), None, Some((9.0, 10.0))];
        let out = iup_contour(&deltas, &coords);
        assert_eq!(out[1].0, 0.0);
        assert_eq!(out[1].1, 5.0);
    }

    #[test]
    fn the_run_before_the_first_touched_point_wraps_around() {
        // Only index 2 is touched, so indices 0 and 1 are covered by the wrapping run
        // and every point ends up with the same delta.
        let coords = [(0.0, 0.0), (10.0, 0.0), (20.0, 0.0), (30.0, 0.0)];
        let deltas = [None, None, Some((2.0, 3.0)), None];
        assert_eq!(iup_contour(&deltas, &coords), vec![(2.0, 3.0); 4]);
    }

    #[test]
    fn interpolation_happens_per_axis() {
        // x interpolates, y clamps, within the same segment.
        let coords = [(0.0, 0.0), (5.0, 100.0), (10.0, 0.0)];
        let deltas = [Some((0.0, 0.0)), None, Some((10.0, 0.0))];
        let out = iup_contour(&deltas, &coords);
        assert_eq!(out[1].0, 5.0);
        assert_eq!(out[1].1, 0.0);
    }
}
