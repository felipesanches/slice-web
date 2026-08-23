//! Reading a glyph as a point set, applying `gvar` deltas to it, and writing it back.
//!
//! Every glyph is handled as a flat list of points followed by four *phantom points*,
//! which is how the OpenType variation machinery sees it:
//!
//! | index | meaning |
//! |---|---|
//! | `0 .. n` | contour points (simple glyph) or component offsets (composite) |
//! | `n`   | left side bearing point: `(xMin - lsb, 0)` |
//! | `n+1` | right side bearing point: `(pp1.x + advanceWidth, 0)` |
//! | `n+2` | top side bearing point |
//! | `n+3` | bottom side bearing point |
//!
//! The phantom points are what let a variation change a glyph's advance width, so they
//! have to travel with the outline and be read back out afterwards.

use read_fonts::tables::glyf::{Anchor, CurvePoint, Glyph as ReadGlyph};
use read_fonts::{FontRef, TableProvider};
use write_fonts::tables::glyf as wglyf;
use write_fonts::types::{F2Dot14, GlyphId, GlyphId16};

use super::iup::{iup_contour, MaybeDelta};
use super::normalize::NormalizedLocation;
use crate::SliceError;

/// How many phantom points every glyph carries.
pub const PHANTOM_COUNT: usize = 4;

/// The structural part of a glyph: everything except the point coordinates.
#[derive(Clone, Debug)]
pub enum GlyphShape {
    /// No outline. Still has phantom points, so its advance can still vary.
    Empty,
    Simple {
        end_pts: Vec<u16>,
        on_curve: Vec<bool>,
        instructions: Vec<u8>,
        overlaps: bool,
    },
    Composite {
        components: Vec<ComponentShape>,
        instructions: Vec<u8>,
    },
}

/// A component of a composite glyph, minus its offset, which lives in the point list.
#[derive(Clone, Debug)]
pub struct ComponentShape {
    pub glyph: GlyphId16,
    pub flags: wglyf::ComponentFlags,
    pub transform: wglyf::Transform,
    /// True when the component is positioned by matching a point rather than by an
    /// offset. Point-matched components have no offset for a delta to move, so their
    /// entry in the point list is a placeholder.
    pub point_matched: bool,
    /// Preserved verbatim for point-matched components.
    pub anchor: Anchor,
}

/// A glyph decomposed into structure plus points, ready to have deltas applied.
#[derive(Clone, Debug)]
pub struct GlyphPoints {
    pub shape: GlyphShape,
    /// Outline points followed by the four phantom points.
    pub coords: Vec<(f64, f64)>,
    /// Contour end indices, used to drive IUP. Empty for composites and empty glyphs.
    pub end_pts: Vec<u16>,
}

impl GlyphPoints {
    /// Number of real (non-phantom) points.
    pub fn outline_len(&self) -> usize {
        self.coords.len() - PHANTOM_COUNT
    }

    /// The four phantom points.
    pub fn phantoms(&self) -> &[(f64, f64)] {
        &self.coords[self.outline_len()..]
    }

    /// The advance width and left side bearing implied by the phantom points.
    ///
    /// `x_min` must be the bound of the glyph *after* deltas were applied, because the
    /// side bearing is measured from the new outline, not the old one.
    pub fn metrics(&self, x_min: i16) -> (u16, i16) {
        let phantoms = self.phantoms();
        let pp1x = phantoms[0].0;
        let pp2x = phantoms[1].0;
        let advance = ot_round(pp2x - pp1x).max(0) as u16;
        let lsb = ot_round(f64::from(x_min) - pp1x) as i16;
        (advance, lsb)
    }
}

/// Round the way OpenType does: halfway cases go towards positive infinity.
///
/// This is `fontTools.misc.roundTools.otRound`, and it is not the same as Rust's
/// `f64::round`, which rounds halves away from zero. The difference shows up on every
/// negative coordinate ending in .5, which in a font is often.
pub fn ot_round(value: f64) -> i32 {
    (value + 0.5).floor() as i32
}

/// Read one glyph as points plus structure.
pub fn read_glyph(font: &FontRef, gid: GlyphId) -> Result<GlyphPoints, SliceError> {
    let loca = font.loca(None)?;
    let glyf = font.glyf()?;
    let hmtx = font.hmtx()?;

    let advance = hmtx.advance(gid).unwrap_or(0);
    let lsb = hmtx.side_bearing(gid).unwrap_or(0);

    let glyph = loca.get_glyf(gid, &glyf)?;

    let (shape, mut coords, end_pts, x_min, y_max) = match glyph {
        None => (GlyphShape::Empty, Vec::new(), Vec::new(), 0i16, 0i16),
        Some(ReadGlyph::Simple(simple)) => {
            let end_pts: Vec<u16> = simple
                .end_pts_of_contours()
                .iter()
                .map(|v| v.get())
                .collect();
            let mut coords = Vec::with_capacity(simple.num_points());
            let mut on_curve = Vec::with_capacity(simple.num_points());
            for point in simple.points() {
                coords.push((f64::from(point.x), f64::from(point.y)));
                on_curve.push(point.on_curve);
            }
            let shape = GlyphShape::Simple {
                end_pts: end_pts.clone(),
                on_curve,
                instructions: simple.instructions().to_vec(),
                overlaps: simple.has_overlapping_contours(),
            };
            (shape, coords, end_pts, simple.x_min(), simple.y_max())
        }
        Some(ReadGlyph::Composite(composite)) => {
            let mut components = Vec::new();
            let mut coords = Vec::new();
            for component in composite.components() {
                let (point_matched, offset) = match component.anchor {
                    Anchor::Offset { x, y } => (false, (f64::from(x), f64::from(y))),
                    Anchor::Point { .. } => (true, (0.0, 0.0)),
                };
                coords.push(offset);
                components.push(ComponentShape {
                    glyph: component.glyph,
                    flags: convert_component_flags(component.flags),
                    transform: convert_transform(component.transform),
                    point_matched,
                    anchor: component.anchor,
                });
            }
            let shape = GlyphShape::Composite {
                components,
                instructions: composite.instructions().unwrap_or_default().to_vec(),
            };
            (
                shape,
                coords,
                Vec::new(),
                composite.x_min(),
                composite.y_max(),
            )
        }
    };

    // Phantom points. The vertical pair is only meaningful for fonts with vertical
    // metrics; without `vmtx` they are both zero, which is what fontTools does too.
    let left_side_x = f64::from(x_min) - f64::from(lsb);
    let right_side_x = left_side_x + f64::from(advance);
    let (top_side_y, bottom_side_y) = match (font.vmtx(), font.vhea()) {
        (Ok(vmtx), Ok(_)) => {
            let v_advance = vmtx.advance(gid).unwrap_or(0);
            let tsb = vmtx.side_bearing(gid).unwrap_or(0);
            let top = f64::from(tsb) + f64::from(y_max);
            (top, top - f64::from(v_advance))
        }
        _ => (0.0, 0.0),
    };
    coords.push((left_side_x, 0.0));
    coords.push((right_side_x, 0.0));
    coords.push((0.0, top_side_y));
    coords.push((0.0, bottom_side_y));

    Ok(GlyphPoints {
        shape,
        coords,
        end_pts,
    })
}

/// The scalar one tuple contributes at `location`.
///
/// Returns zero when the tuple does not apply at all, in which case its deltas can be
/// skipped entirely.
pub fn tuple_scalar(peak: &[f64], intermediate: Option<(&[f64], &[f64])>, location: &[f64]) -> f64 {
    let mut scalar = 1.0;
    for (axis, &peak_value) in peak.iter().enumerate() {
        // A peak of zero means this axis does not participate in the tuple.
        if peak_value == 0.0 {
            continue;
        }
        let coord = location.get(axis).copied().unwrap_or(0.0);
        if coord == peak_value {
            continue;
        }

        let (start, end) = match intermediate {
            Some((starts, ends)) => (
                starts.get(axis).copied().unwrap_or(0.0),
                ends.get(axis).copied().unwrap_or(0.0),
            ),
            None => {
                // Without an explicit intermediate region the tuple ramps from the
                // origin up to the peak and back.
                if peak_value > 0.0 {
                    (0.0, peak_value)
                } else {
                    (peak_value, 0.0)
                }
            }
        };

        if coord <= start || coord >= end {
            return 0.0;
        }
        if coord < peak_value {
            scalar *= (coord - start) / (peak_value - start);
        } else {
            scalar *= (end - coord) / (end - peak_value);
        }
    }
    scalar
}

/// Add every applicable `gvar` delta for one glyph to its points.
///
/// Returns the number of tuples that contributed, which callers use to tell "this glyph
/// does not vary" from "this glyph varies but not at this location".
pub fn apply_gvar_deltas(
    font: &FontRef,
    gid: GlyphId,
    location: &NormalizedLocation,
    points: &mut GlyphPoints,
) -> Result<usize, SliceError> {
    let Ok(gvar) = font.gvar() else {
        return Ok(0);
    };
    let Some(data) = gvar.glyph_variation_data(gid).ok().flatten() else {
        return Ok(0);
    };

    let point_count = points.coords.len();
    let mut applied = 0usize;

    // Every tuple interpolates its missing deltas against the glyph's *original*
    // coordinates. Using the running total instead would make each tuple's
    // interpolation depend on the ones before it, which shows up as soon as two
    // tuples apply at once and either is sparse.
    let original = points.coords.clone();

    for tuple in data.tuples() {
        let peak: Vec<f64> = tuple
            .peak()
            .values()
            .iter()
            .map(|v| v.get().to_f64())
            .collect();
        let start: Option<Vec<f64>> = tuple
            .intermediate_start()
            .map(|t| t.values().iter().map(|v| v.get().to_f64()).collect());
        let end: Option<Vec<f64>> = tuple
            .intermediate_end()
            .map(|t| t.values().iter().map(|v| v.get().to_f64()).collect());
        let intermediate = match (&start, &end) {
            (Some(s), Some(e)) => Some((s.as_slice(), e.as_slice())),
            _ => None,
        };

        let scalar = tuple_scalar(&peak, intermediate, &location.coords);
        if scalar == 0.0 {
            continue;
        }

        // Collect this tuple's explicit deltas, indexed by point.
        let mut deltas: Vec<MaybeDelta> = vec![None; point_count];
        for delta in tuple.deltas() {
            let index = delta.position as usize;
            if index < point_count {
                deltas[index] = Some((f64::from(delta.x_delta), f64::from(delta.y_delta)));
            }
        }

        let resolved = interpolate_missing(&deltas, &original, &points.end_pts);
        for (point, delta) in points.coords.iter_mut().zip(resolved) {
            point.0 += delta.0 * scalar;
            point.1 += delta.1 * scalar;
        }
        applied += 1;
    }

    Ok(applied)
}

/// Fill in deltas for points the tuple did not mention.
///
/// Exposed for the partial instancer, which needs the same interpolation when it reads
/// tuples apart from the outline.
pub fn interpolate_missing_public(
    deltas: &[MaybeDelta],
    coords: &[(f64, f64)],
    end_pts: &[u16],
) -> Vec<(f64, f64)> {
    interpolate_missing(deltas, coords, end_pts)
}

/// Fill in deltas for points the tuple did not mention.
///
/// Contours are interpolated with IUP. The four phantom points are each treated as their
/// own single-point contour, which means an unmentioned phantom point simply does not
/// move; that is what `fontTools.varLib.iup.iup_delta` does.
fn interpolate_missing(
    deltas: &[MaybeDelta],
    coords: &[(f64, f64)],
    end_pts: &[u16],
) -> Vec<(f64, f64)> {
    if deltas.iter().all(Option::is_some) {
        return deltas.iter().map(|d| d.unwrap()).collect();
    }

    let outline_len = coords.len() - PHANTOM_COUNT;
    let mut boundaries: Vec<usize> = end_pts.iter().map(|&e| e as usize + 1).collect();
    // A composite glyph's component offsets are not a contour, and cannot be
    // interpolated between: an unmentioned component simply does not move.
    if boundaries.is_empty() && outline_len > 0 {
        boundaries.extend(1..=outline_len);
    }
    boundaries.extend((outline_len + 1)..=coords.len());

    let mut out = Vec::with_capacity(coords.len());
    let mut start = 0usize;
    for end in boundaries {
        if end > start && end <= coords.len() {
            out.extend(iup_contour(&deltas[start..end], &coords[start..end]));
            start = end;
        }
    }
    // Anything left over (a malformed end_pts array) simply does not move.
    while out.len() < coords.len() {
        out.push((0.0, 0.0));
    }
    out
}

/// Turn a point set back into a glyph, rounding coordinates to integers.
pub fn build_glyph(points: &GlyphPoints) -> wglyf::Glyph {
    let outline_len = points.outline_len();
    match &points.shape {
        GlyphShape::Empty => wglyf::Glyph::Empty,
        GlyphShape::Simple {
            end_pts,
            on_curve,
            instructions,
            overlaps,
        } => {
            let mut contours: Vec<wglyf::Contour> = Vec::with_capacity(end_pts.len());
            let mut start = 0usize;
            for &end in end_pts {
                let end = (end as usize + 1).min(outline_len);
                if end <= start {
                    continue;
                }
                let contour: Vec<CurvePoint> = (start..end)
                    .map(|i| {
                        let (x, y) = points.coords[i];
                        CurvePoint::new(
                            ot_round(x) as i16,
                            ot_round(y) as i16,
                            on_curve.get(i).copied().unwrap_or(true),
                        )
                    })
                    .collect();
                contours.push(contour.into());
                start = end;
            }
            if contours.is_empty() {
                return wglyf::Glyph::Empty;
            }
            let mut glyph = wglyf::SimpleGlyph {
                bbox: Default::default(),
                contours,
                instructions: instructions.clone(),
                overlaps: *overlaps,
            };
            glyph.recompute_bounding_box();
            wglyf::Glyph::Simple(glyph)
        }
        GlyphShape::Composite {
            components,
            instructions,
        } => {
            let built: Vec<wglyf::Component> = components
                .iter()
                .enumerate()
                .map(|(i, component)| {
                    let anchor = if component.point_matched {
                        match component.anchor {
                            Anchor::Point { base, component: c } => {
                                wglyf::Anchor::Point { base, component: c }
                            }
                            Anchor::Offset { x, y } => wglyf::Anchor::Offset { x, y },
                        }
                    } else {
                        let (x, y) = points.coords[i];
                        wglyf::Anchor::Offset {
                            x: ot_round(x) as i16,
                            y: ot_round(y) as i16,
                        }
                    };
                    wglyf::Component {
                        glyph: component.glyph,
                        anchor,
                        flags: component.flags,
                        transform: component.transform,
                    }
                })
                .collect();

            let mut iter = built.into_iter();
            let Some(first) = iter.next() else {
                return wglyf::Glyph::Empty;
            };
            // The bounding box is recomputed by the caller once every component glyph
            // is final, since a composite's bounds depend on the glyphs it references.
            let mut composite = wglyf::CompositeGlyph::new(first, wglyf::Bbox::default());
            for component in iter {
                composite.add_component(component, wglyf::Bbox::default());
            }
            if !instructions.is_empty() {
                composite.set_instructions(instructions);
            }
            wglyf::Glyph::Composite(composite)
        }
    }
}

fn convert_component_flags(
    flags: read_fonts::tables::glyf::CompositeGlyphFlags,
) -> wglyf::ComponentFlags {
    use read_fonts::tables::glyf::CompositeGlyphFlags as F;
    wglyf::ComponentFlags {
        round_xy_to_grid: flags.contains(F::ROUND_XY_TO_GRID),
        use_my_metrics: flags.contains(F::USE_MY_METRICS),
        scaled_component_offset: flags.contains(F::SCALED_COMPONENT_OFFSET),
        unscaled_component_offset: flags.contains(F::UNSCALED_COMPONENT_OFFSET),
        overlap_compound: flags.contains(F::OVERLAP_COMPOUND),
    }
}

fn convert_transform(transform: read_fonts::tables::glyf::Transform) -> wglyf::Transform {
    wglyf::Transform {
        xx: F2Dot14::from_bits(transform.xx.to_bits()),
        yx: F2Dot14::from_bits(transform.yx.to_bits()),
        xy: F2Dot14::from_bits(transform.xy.to_bits()),
        yy: F2Dot14::from_bits(transform.yy.to_bits()),
    }
}

/// Fill in the bounding box of every composite glyph, resolving nesting.
///
/// A composite's bounds depend on the glyphs it references, so they cannot be known when
/// the composite itself is built -- the components may not have been instanced yet, and a
/// component may itself be a composite. `write-fonts` leaves the field alone, so without
/// this pass every composite ships with (0, 0, 0, 0). That is a real defect and not only
/// a cosmetic one: rasterizers use the glyph bbox to size caches and to cull, `head`'s
/// font-wide bbox is the union of the per-glyph ones and comes out too small, and
/// `hmtx`-derived side bearings computed from it are wrong.
///
/// The bounds are taken over transformed *points*, not over transformed child boxes: for
/// a rotated or skewed component the box of the transformed box is strictly larger than
/// the box of the transformed points, and `head` would then claim more than the outlines
/// occupy.
///
/// Point-anchored components (`ARGS_ARE_XY_VALUES` clear) are resolved as a zero offset.
/// Computing their true placement means matching a point in the component against one in
/// the glyph built so far, which depends on the order components are composed in; the
/// arrangement is rare, and a zero offset is what the component's own coordinates already
/// describe.
pub fn fill_composite_bboxes(glyphs: &mut [wglyf::Glyph]) {
    // Depth-first with memoisation. `resolving` breaks reference cycles, which a
    // malformed font can contain and which would otherwise recurse until the stack ends.
    let mut points: Vec<Option<Vec<(f64, f64)>>> = vec![None; glyphs.len()];
    let mut resolving = vec![false; glyphs.len()];
    for index in 0..glyphs.len() {
        collect_points(glyphs, index, &mut points, &mut resolving);
    }

    for (index, glyph) in glyphs.iter_mut().enumerate() {
        let wglyf::Glyph::Composite(composite) = glyph else {
            continue;
        };
        let Some(collected) = points[index].as_ref().filter(|p| !p.is_empty()) else {
            continue;
        };
        let (mut x_min, mut y_min) = (f64::MAX, f64::MAX);
        let (mut x_max, mut y_max) = (f64::MIN, f64::MIN);
        for &(x, y) in collected {
            x_min = x_min.min(x);
            y_min = y_min.min(y);
            x_max = x_max.max(x);
            y_max = y_max.max(y);
        }
        composite.bbox = wglyf::Bbox {
            x_min: saturating_round(x_min.floor()),
            y_min: saturating_round(y_min.floor()),
            x_max: saturating_round(x_max.ceil()),
            y_max: saturating_round(y_max.ceil()),
        };
    }
}

fn collect_points(
    glyphs: &[wglyf::Glyph],
    index: usize,
    points: &mut Vec<Option<Vec<(f64, f64)>>>,
    resolving: &mut Vec<bool>,
) -> Vec<(f64, f64)> {
    if let Some(done) = points.get(index).and_then(|p| p.clone()) {
        return done;
    }
    if index >= glyphs.len() || resolving[index] {
        return Vec::new();
    }
    resolving[index] = true;

    let collected = match &glyphs[index] {
        wglyf::Glyph::Empty => Vec::new(),
        wglyf::Glyph::Simple(simple) => simple
            .contours
            .iter()
            .flat_map(|contour| contour.iter())
            .map(|point| (f64::from(point.x), f64::from(point.y)))
            .collect(),
        wglyf::Glyph::Composite(composite) => {
            let mut out = Vec::new();
            for component in composite.components() {
                let child =
                    collect_points(glyphs, component.glyph.to_u32() as usize, points, resolving);
                let (dx, dy) = match component.anchor {
                    wglyf::Anchor::Offset { x, y } => (f64::from(x), f64::from(y)),
                    wglyf::Anchor::Point { .. } => (0.0, 0.0),
                };
                let t = component.transform;
                let (xx, yx) = (f64::from(t.xx.to_f32()), f64::from(t.yx.to_f32()));
                let (xy, yy) = (f64::from(t.xy.to_f32()), f64::from(t.yy.to_f32()));
                out.extend(
                    child
                        .into_iter()
                        .map(|(x, y)| (xx * x + xy * y + dx, yx * x + yy * y + dy)),
                );
            }
            out
        }
    };

    resolving[index] = false;
    points[index] = Some(collected.clone());
    collected
}

fn saturating_round(value: f64) -> i16 {
    value.clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16
}

#[cfg(test)]
mod tests {

    /// A square 0..400, a dot 0..120, a composite of both, and a composite of *that*
    /// plus another dot: the nesting is what makes this worth a test, since a one-level
    /// implementation gets `squaredot` right and `double` wrong.
    #[test]
    fn composite_bounds_resolve_through_nesting() {
        use write_fonts::tables::glyf as g;
        use write_fonts::types::GlyphId16;

        fn square(size: i16) -> g::Glyph {
            let contour = g::Contour::from(vec![
                CurvePoint::on_curve(0, 0),
                CurvePoint::on_curve(size, 0),
                CurvePoint::on_curve(size, size),
                CurvePoint::on_curve(0, size),
            ]);
            g::Glyph::Simple(g::SimpleGlyph {
                bbox: g::Bbox {
                    x_min: 0,
                    y_min: 0,
                    x_max: size,
                    y_max: size,
                },
                contours: vec![contour],
                instructions: Vec::new(),
                overlaps: false,
            })
        }

        fn composite(parts: &[(u16, i16, i16)]) -> g::Glyph {
            let make = |&(gid, x, y): &(u16, i16, i16)| g::Component {
                glyph: GlyphId16::new(gid),
                anchor: Anchor::Offset { x, y },
                flags: Default::default(),
                transform: Default::default(),
            };
            let mut iter = parts.iter();
            let mut out = g::CompositeGlyph::new(make(iter.next().unwrap()), g::Bbox::default());
            for part in iter {
                out.add_component(make(part), g::Bbox::default());
            }
            g::Glyph::Composite(out)
        }

        let mut glyphs = vec![
            square(400),                          // 0: square
            square(120),                          // 1: dot
            composite(&[(0, 0, 0), (1, 0, 520)]), // 2: squaredot
            composite(&[(2, 0, 0), (1, 500, 0)]), // 3: double
        ];
        fill_composite_bboxes(&mut glyphs);

        let bbox = |i: usize| match &glyphs[i] {
            g::Glyph::Composite(c) => c.bbox,
            _ => panic!("glyph {i} is not a composite"),
        };
        // squaredot: the square to 400, the dot lifted to 520..640.
        assert_eq!(
            bbox(2),
            g::Bbox {
                x_min: 0,
                y_min: 0,
                x_max: 400,
                y_max: 640
            }
        );
        // double: squaredot resolved one level down, plus a dot out at x=500..620.
        assert_eq!(
            bbox(3),
            g::Bbox {
                x_min: 0,
                y_min: 0,
                x_max: 620,
                y_max: 640
            }
        );
    }

    /// A malformed font can have a composite reference itself. The pass must finish.
    #[test]
    fn a_reference_cycle_does_not_recurse_forever() {
        use write_fonts::tables::glyf as g;
        use write_fonts::types::GlyphId16;
        let component = g::Component {
            glyph: GlyphId16::new(0),
            anchor: Anchor::Offset { x: 0, y: 0 },
            flags: Default::default(),
            transform: Default::default(),
        };
        let mut glyphs = vec![g::Glyph::Composite(g::CompositeGlyph::new(
            component,
            g::Bbox::default(),
        ))];
        fill_composite_bboxes(&mut glyphs);
    }

    use super::*;

    fn font_bytes() -> &'static [u8] {
        crate::testdata::recursive_vf()
    }

    #[test]
    fn ot_round_breaks_ties_upward() {
        // The distinguishing cases are the negative halves, where Rust's own round()
        // would go the other way.
        assert_eq!(ot_round(0.5), 1);
        assert_eq!(ot_round(-0.5), 0);
        assert_eq!(ot_round(-1.5), -1);
        assert_eq!(ot_round(2.5), 3);
        assert_eq!(ot_round(-2.4), -2);
        assert_eq!(ot_round(1.4), 1);
    }

    #[test]
    fn every_glyph_reads_back_with_four_phantom_points() {
        let font = FontRef::new(font_bytes()).unwrap();
        let count = font.maxp().unwrap().num_glyphs();
        for gid in 0..count {
            let points = read_glyph(&font, GlyphId::new(gid as u32)).unwrap();
            assert!(
                points.coords.len() >= PHANTOM_COUNT,
                "glyph {gid} has no phantom points"
            );
        }
    }

    #[test]
    fn phantom_points_reproduce_the_original_metrics() {
        let font = FontRef::new(font_bytes()).unwrap();
        let hmtx = font.hmtx().unwrap();
        let loca = font.loca(None).unwrap();
        let glyf = font.glyf().unwrap();
        let count = font.maxp().unwrap().num_glyphs();

        for gid in 0..count {
            let gid = GlyphId::new(gid as u32);
            let points = read_glyph(&font, gid).unwrap();
            let x_min = match loca.get_glyf(gid, &glyf).unwrap() {
                Some(ReadGlyph::Simple(g)) => g.x_min(),
                Some(ReadGlyph::Composite(g)) => g.x_min(),
                None => 0,
            };
            let (advance, lsb) = points.metrics(x_min);
            assert_eq!(
                advance,
                hmtx.advance(gid).unwrap_or(0),
                "advance for {gid:?}"
            );
            assert_eq!(
                lsb,
                hmtx.side_bearing(gid).unwrap_or(0),
                "side bearing for {gid:?}"
            );
        }
    }

    #[test]
    fn a_tuple_peaks_at_its_own_location() {
        // A tuple with peak 1.0 on one axis contributes fully at 1.0, half at 0.5, and
        // nothing at or below 0.
        let peak = [1.0];
        assert_eq!(tuple_scalar(&peak, None, &[1.0]), 1.0);
        assert_eq!(tuple_scalar(&peak, None, &[0.5]), 0.5);
        assert_eq!(tuple_scalar(&peak, None, &[0.0]), 0.0);
        assert_eq!(tuple_scalar(&peak, None, &[-0.5]), 0.0);
    }

    #[test]
    fn a_zero_peak_axis_does_not_constrain_the_tuple() {
        // Axis 0 does not participate, so only axis 1 matters.
        let peak = [0.0, 1.0];
        assert_eq!(tuple_scalar(&peak, None, &[0.7, 1.0]), 1.0);
        assert_eq!(tuple_scalar(&peak, None, &[-1.0, 1.0]), 1.0);
    }

    #[test]
    fn an_intermediate_region_narrows_where_a_tuple_applies() {
        let peak = [0.5];
        let start = [0.25];
        let end = [0.75];
        let inter = Some((start.as_slice(), end.as_slice()));
        assert_eq!(tuple_scalar(&peak, inter, &[0.5]), 1.0);
        assert_eq!(tuple_scalar(&peak, inter, &[0.25]), 0.0);
        assert_eq!(tuple_scalar(&peak, inter, &[0.75]), 0.0);
        assert_eq!(tuple_scalar(&peak, inter, &[0.375]), 0.5);
        // Outside the region entirely.
        assert_eq!(tuple_scalar(&peak, inter, &[0.9]), 0.0);
    }

    #[test]
    fn applying_deltas_at_the_default_changes_nothing() {
        let font = FontRef::new(font_bytes()).unwrap();
        let slice_font = crate::SliceFont::load(font_bytes().to_vec()).unwrap();
        let axes = slice_font.axes().unwrap();
        let defaults: Vec<f64> = axes.iter().map(|a| a.default).collect();
        let location = super::super::normalize_location(&font, &axes, &defaults);

        let count = font.maxp().unwrap().num_glyphs();
        for gid in 0..count {
            let gid = GlyphId::new(gid as u32);
            let before = read_glyph(&font, gid).unwrap();
            let mut after = before.clone();
            apply_gvar_deltas(&font, gid, &location, &mut after).unwrap();
            for (i, (a, b)) in before.coords.iter().zip(&after.coords).enumerate() {
                assert!(
                    (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9,
                    "glyph {gid:?} point {i} moved at the default location: {a:?} -> {b:?}"
                );
            }
        }
    }

    #[test]
    fn applying_deltas_away_from_the_default_moves_something() {
        let font = FontRef::new(font_bytes()).unwrap();
        let slice_font = crate::SliceFont::load(font_bytes().to_vec()).unwrap();
        let axes = slice_font.axes().unwrap();
        let mut user: Vec<f64> = axes.iter().map(|a| a.default).collect();
        // wght is axis 2 in this font; take it to its maximum.
        user[2] = axes[2].max;
        let location = super::super::normalize_location(&font, &axes, &user);

        let count = font.maxp().unwrap().num_glyphs();
        let mut moved = 0;
        for gid in 0..count {
            let gid = GlyphId::new(gid as u32);
            let before = read_glyph(&font, gid).unwrap();
            let mut after = before.clone();
            apply_gvar_deltas(&font, gid, &location, &mut after).unwrap();
            if before
                .coords
                .iter()
                .zip(&after.coords)
                .any(|(a, b)| (a.0 - b.0).abs() > 1e-9 || (a.1 - b.1).abs() > 1e-9)
            {
                moved += 1;
            }
        }
        assert!(
            moved > 0,
            "taking wght to its maximum should move at least one glyph"
        );
    }
}
