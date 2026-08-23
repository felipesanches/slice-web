//! Partial instancing: narrowing the design space while the font stays variable.
//!
//! This is the case the original Slice is really for. Pinning every axis gives a static
//! font, which [`super::statics`] handles; here some axes survive, either with their
//! whole original extent or restricted to a smaller one, and the variation data has to
//! be rewritten to describe the same shapes over the smaller space.
//!
//! The mathematics is in [`crate::solver`]. What is here is the bookkeeping around it:
//! taking each glyph's tuple variations apart, rebasing every axis of every tuple,
//! recombining what comes back, folding whatever no longer depends on any axis into the
//! outline itself, and rebuilding `fvar`, `avar` and `STAT` to match.
//!
//! # What is carried over, and what is not
//!
//! | table | what happens |
//! |---|---|
//! | `glyf` / `gvar` | rebased; tuples that lost all their axes are baked into the outlines |
//! | `hmtx` | recomputed from the phantom points, as always |
//! | `fvar` | pinned axes removed, restricted axes narrowed, named instances filtered |
//! | `avar` | segment maps renormalized onto the new axis extents |
//! | `STAT` | design axis records for pinned axes removed, axis values re-indexed |
//! | `HVAR` / `VVAR` | **dropped.** For TrueType outlines this loses nothing: advance widths vary through the phantom points in `gvar`, which are rebased along with everything else, and a renderer falls back to them when `HVAR` is absent. |
//! | `MVAR` | applied at the new default and then dropped, so the metrics are right there but no longer vary across whatever range is left. |
//! | `GDEF` / `GPOS` variation stores | **refused.** Their regions describe the old axis space, so leaving them gives wrong positioning and removing them dangles the indices that point into them. Rather than ship either, a font that has them is rejected with an explanation. |

use std::collections::BTreeMap;

use read_fonts::{FontRef, TableProvider};
use write_fonts::tables::avar::{Avar, AxisValueMap, SegmentMaps};
use write_fonts::tables::fvar::InstanceRecord;
use write_fonts::tables::fvar::{AxisInstanceArrays, Fvar, VariationAxisRecord};
use write_fonts::tables::glyf::{GlyfLocaBuilder, Glyph as WGlyph};
use write_fonts::tables::gvar::{GlyphDelta, GlyphDeltas, GlyphVariations, Gvar};
use write_fonts::tables::variations::Tent;
use write_fonts::types::{F2Dot14, Fixed, GlyphId, Tag};
use write_fonts::{from_obj::ToOwnedTable, FontBuilder};

use super::glyphs::{build_glyph, ot_round, read_glyph, GlyphPoints};
use super::normalize::{apply_segment_map, normalize_axis, quantize, segment_maps};
use crate::axes::{AxisLimit, AxisSpec};
use crate::solver::{rebase_tent, AxisTriple, Tent as SolverTent};
use crate::SliceError;

/// What is to become of one axis.
#[derive(Clone, Debug)]
pub struct AxisPlan {
    pub spec: AxisSpec,
    pub limit: AxisLimit,
    /// The limit in normalized coordinates, with the pre-normalization distances the
    /// solver needs to renormalize across an off-centre default.
    pub normalized: AxisTriple,
}

impl AxisPlan {
    pub fn is_pinned(&self) -> bool {
        !self.limit.keeps_axis()
    }

    /// True when the axis comes through untouched.
    pub fn is_untouched(&self) -> bool {
        matches!(self.limit, AxisLimit::Full)
    }

    /// The axis extent to write into the output `fvar`, in user space.
    pub fn output_extent(&self) -> (f64, f64, f64) {
        match self.limit {
            AxisLimit::Full => (self.spec.min, self.spec.default, self.spec.max),
            AxisLimit::Range { min, max } => (min, self.spec.default, max),
            // Pinned axes are not written out at all.
            AxisLimit::Pin(v) => (v, v, v),
        }
    }
}

/// Work out, for each axis, what the request means in normalized coordinates.
pub fn plan_axes(font: &FontRef, axes: &[AxisSpec], limits: &[AxisLimit]) -> Vec<AxisPlan> {
    let maps = segment_maps(font).map(|(maps, _)| maps);

    axes.iter()
        .zip(limits)
        .enumerate()
        .map(|(index, (spec, limit))| {
            let map = maps.as_ref().and_then(|m| m.get(index));
            let normalize = |value: f64| {
                let normalized = quantize(normalize_axis(value, spec));
                match map {
                    Some(map) => quantize(apply_segment_map(normalized, map)),
                    None => normalized,
                }
            };

            let (min, default, max) = match limit {
                AxisLimit::Full => (spec.min, spec.default, spec.max),
                AxisLimit::Pin(v) => (*v, *v, *v),
                // Level 3 sub-spacing requires the range to contain the default, and
                // `AxisSpec::validate` enforces that before anything gets here. Clamp
                // anyway: the solver asserts on an unsorted triple, and a panic deep in
                // the engine is a poor way to report a caller's mistake.
                AxisLimit::Range { min, max } => (*min, spec.default.clamp(*min, *max), *max),
            };

            AxisPlan {
                spec: spec.clone(),
                limit: *limit,
                normalized: AxisTriple::with_distances(
                    normalize(min),
                    normalize(default),
                    normalize(max),
                    // The pre-normalization widths of the two halves of the *original*
                    // axis. Without these, renormalizing a value that crosses the
                    // default is wrong whenever the default is not centred.
                    spec.default - spec.min,
                    spec.max - spec.default,
                ),
            }
        })
        .collect()
}

/// One tuple variation, in a form that can be taken apart and put back together.
#[derive(Clone, Debug)]
struct TupleVar {
    /// Support per *input* axis index. An axis that is absent does not participate.
    axes: BTreeMap<usize, SolverTent>,
    /// One delta per point, including the four phantom points. Always dense: sparse
    /// tuples have their missing deltas interpolated on the way in, because scaling and
    /// splitting a tuple are both linear and so commute with that interpolation.
    deltas: Vec<(f64, f64)>,
}

impl TupleVar {
    /// A key that identifies the tuple's support, for merging tuples that ended up
    /// covering the same region.
    ///
    /// Quantized to F2Dot14 because that is the precision the tents will be written at;
    /// two tents that differ only below it are the same tent in the file.
    fn support_key(&self) -> Vec<(usize, i16, i16, i16)> {
        self.axes
            .iter()
            .map(|(index, (start, peak, end))| {
                (
                    *index,
                    F2Dot14::from_f32(*start as f32).to_bits(),
                    F2Dot14::from_f32(*peak as f32).to_bits(),
                    F2Dot14::from_f32(*end as f32).to_bits(),
                )
            })
            .collect()
    }

    fn scale(&mut self, scalar: f64) {
        for delta in &mut self.deltas {
            delta.0 *= scalar;
            delta.1 *= scalar;
        }
    }
}

/// Apply one axis's limit to one tuple, which may drop it, keep it, or split it.
fn limit_on_axis(var: &TupleVar, axis: usize, limit: AxisTriple) -> Vec<TupleVar> {
    let Some(&tent) = var.axes.get(&axis) else {
        // The axis does not participate; nothing to do.
        return vec![var.clone()];
    };
    let (lower, peak, upper) = tent;

    if peak == 0.0 {
        // An axis explicitly present but peaking at zero contributes nothing and can be
        // dropped outright (fontTools issue #3453).
        let mut out = var.clone();
        out.axes.remove(&axis);
        return vec![out];
    }

    // A malformed tent -- out of order, or straddling the default -- has no defensible
    // meaning, and the solver asserts on it. Drop the tuple.
    if !(lower <= peak && peak <= upper) || (lower < 0.0 && upper > 0.0) {
        return Vec::new();
    }

    rebase_tent(tent, limit)
        .into_iter()
        .map(|(scalar, new_tent)| {
            let mut out = var.clone();
            match new_tent {
                None => {
                    out.axes.remove(&axis);
                }
                Some(t) => {
                    out.axes.insert(axis, t);
                }
            }
            out.scale(scalar);
            out
        })
        .collect()
}

/// Rebase every tuple onto the new axis extents.
fn limit_tuples(mut tuples: Vec<TupleVar>, plans: &[AxisPlan]) -> Vec<TupleVar> {
    for (index, plan) in plans.iter().enumerate() {
        // An axis nobody asked to change needs no rebasing.
        if plan.is_untouched() {
            continue;
        }
        let mut next = Vec::with_capacity(tuples.len());
        for var in &tuples {
            next.extend(limit_on_axis(var, index, plan.normalized));
        }
        tuples = next;
    }
    tuples
}

/// Combine tuples covering the same region, and separate out the part that no longer
/// depends on any axis.
///
/// The returned deltas apply unconditionally and belong in the outline itself.
fn merge(tuples: Vec<TupleVar>, point_count: usize) -> (Vec<(f64, f64)>, Vec<TupleVar>) {
    let mut merged: BTreeMap<Vec<(usize, i16, i16, i16)>, TupleVar> = BTreeMap::new();
    for var in tuples {
        let key = var.support_key();
        match merged.get_mut(&key) {
            Some(existing) => {
                for (slot, delta) in existing.deltas.iter_mut().zip(&var.deltas) {
                    slot.0 += delta.0;
                    slot.1 += delta.1;
                }
            }
            None => {
                merged.insert(key, var);
            }
        }
    }

    let default = merged
        .remove(&Vec::new())
        .map(|var| var.deltas)
        .unwrap_or_else(|| vec![(0.0, 0.0); point_count]);

    (default, merged.into_values().collect())
}

/// Read a glyph's tuples, with sparse deltas interpolated so every point has one.
fn read_tuples(
    font: &FontRef,
    gid: GlyphId,
    points: &GlyphPoints,
) -> Result<Vec<TupleVar>, SliceError> {
    let Ok(gvar) = font.gvar() else {
        return Ok(Vec::new());
    };
    let Some(data) = gvar.glyph_variation_data(gid).ok().flatten() else {
        return Ok(Vec::new());
    };

    let point_count = points.coords.len();
    let mut out = Vec::new();

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

        let mut axes = BTreeMap::new();
        for (index, &peak_value) in peak.iter().enumerate() {
            if peak_value == 0.0 {
                continue;
            }
            let (lower, upper) = match (&start, &end) {
                (Some(s), Some(e)) => (
                    s.get(index).copied().unwrap_or(0.0),
                    e.get(index).copied().unwrap_or(0.0),
                ),
                _ => {
                    if peak_value > 0.0 {
                        (0.0, peak_value)
                    } else {
                        (peak_value, 0.0)
                    }
                }
            };
            axes.insert(index, (lower, peak_value, upper));
        }

        let mut sparse: Vec<Option<(f64, f64)>> = vec![None; point_count];
        for delta in tuple.deltas() {
            let index = delta.position as usize;
            if index < point_count {
                sparse[index] = Some((f64::from(delta.x_delta), f64::from(delta.y_delta)));
            }
        }
        let deltas =
            super::glyphs::interpolate_missing_public(&sparse, &points.coords, &points.end_pts);

        out.push(TupleVar { axes, deltas });
    }

    Ok(out)
}

/// Build a partially instanced font.
pub fn instantiate_partial(font: &FontRef, plans: &[AxisPlan]) -> Result<Vec<u8>, SliceError> {
    if font.glyf().is_err() {
        return Err(SliceError::Unsupported(
            "Only TrueType outlines (a 'glyf' table) can be instanced at the moment. \
             This font uses CFF outlines."
                .into(),
        ));
    }
    refuse_unsupported_variation_tables(font)?;

    let num_glyphs = font.maxp()?.num_glyphs();

    // Which input axes survive, and where they land in the output.
    let kept: Vec<usize> = (0..plans.len())
        .filter(|i| !plans[*i].is_pinned())
        .collect();
    let output_index: BTreeMap<usize, usize> = kept
        .iter()
        .enumerate()
        .map(|(new, old)| (*old, new))
        .collect();

    let mut shapes: Vec<WGlyph> = Vec::with_capacity(num_glyphs as usize);
    let mut metrics: Vec<(u16, i16)> = Vec::with_capacity(num_glyphs as usize);
    let mut variations: Vec<GlyphVariations> = Vec::new();
    let mut any_variations = false;

    for gid in 0..num_glyphs {
        let gid = GlyphId::new(gid as u32);
        let mut points = read_glyph(font, gid)?;
        let point_count = points.coords.len();

        let tuples = read_tuples(font, gid, &points)?;
        let limited = limit_tuples(tuples, plans);
        let (default_deltas, remaining) = merge(limited, point_count);

        // Whatever no longer depends on an axis moves into the outline.
        for (point, delta) in points.coords.iter_mut().zip(&default_deltas) {
            point.0 += delta.0;
            point.1 += delta.1;
        }

        let phantoms = points.phantoms();
        let (pp1, pp2) = (phantoms[0].0, phantoms[1].0);

        let glyph = build_glyph(&points);
        let x_min = match &glyph {
            WGlyph::Simple(simple) => simple.bbox.x_min,
            _ => 0,
        };
        let advance = ot_round(pp2 - pp1).max(0) as u16;
        let lsb = ot_round(f64::from(x_min) - pp1) as i16;
        metrics.push((advance, lsb));
        shapes.push(glyph);

        // Whatever is left stays in gvar, expressed over the surviving axes.
        let deltas: Vec<GlyphDeltas> = remaining
            .iter()
            .filter_map(|var| {
                let mut tents = vec![Tent::new(F2Dot14::ZERO, None); kept.len()];
                for (index, (start, peak, end)) in &var.axes {
                    let Some(&slot) = output_index.get(index) else {
                        // A pinned axis should have been removed by the limiting; if one
                        // survived, the tuple cannot be expressed and is dropped rather
                        // than written against the wrong axis.
                        return None;
                    };
                    tents[slot] = Tent::new(
                        F2Dot14::from_f32(*peak as f32),
                        Some((
                            F2Dot14::from_f32(*start as f32),
                            F2Dot14::from_f32(*end as f32),
                        )),
                    );
                }

                let rounded: Vec<GlyphDelta> = var
                    .deltas
                    .iter()
                    .map(|(x, y)| GlyphDelta::required(ot_round(*x) as i16, ot_round(*y) as i16))
                    .collect();
                if rounded.iter().all(|d| d.x == 0 && d.y == 0) {
                    // A tuple that moves nothing is not worth writing.
                    return None;
                }
                Some(GlyphDeltas::new(tents, rounded))
            })
            .collect();

        // Every glyph needs an entry, including the ones with nothing to say: the
        // offsets array in gvar is positional, so a missing entry does not mean "this
        // glyph does not vary", it shifts every later glyph's data onto the wrong glyph.
        any_variations |= !deltas.is_empty();
        variations.push(GlyphVariations::new(gid, deltas));
    }

    // Assemble.
    let mut builder = GlyfLocaBuilder::new();
    for glyph in &shapes {
        builder
            .add_glyph(glyph)
            .map_err(|e| SliceError::Write(e.to_string()))?;
    }
    let (glyf, loca, loca_format) = builder.build();

    let mut out = FontBuilder::new();
    out.add_table(&glyf)
        .map_err(|e| SliceError::Write(e.to_string()))?;
    out.add_table(&loca)
        .map_err(|e| SliceError::Write(e.to_string()))?;
    out.add_table(&super::statics::build_hmtx_public(&metrics))
        .map_err(|e| SliceError::Write(e.to_string()))?;

    let mut head: write_fonts::tables::head::Head = font.head()?.to_owned_table();
    head.index_to_loc_format = loca_format as i16;
    out.add_table(&head)
        .map_err(|e| SliceError::Write(e.to_string()))?;

    let mut hhea: write_fonts::tables::hhea::Hhea = font.hhea()?.to_owned_table();
    hhea.number_of_h_metrics = super::statics::long_metric_count_public(&metrics) as u16;
    out.add_table(&hhea)
        .map_err(|e| SliceError::Write(e.to_string()))?;

    if any_variations {
        let gvar = Gvar::new(variations, kept.len() as u16)
            .map_err(|e| SliceError::Write(e.to_string()))?;
        out.add_table(&gvar)
            .map_err(|e| SliceError::Write(e.to_string()))?;
    }

    out.add_table(&build_fvar(font, plans)?)
        .map_err(|e| SliceError::Write(e.to_string()))?;

    if let Some(avar) = build_avar(font, plans)? {
        out.add_table(&avar)
            .map_err(|e| SliceError::Write(e.to_string()))?;
    }

    if let Some(stat) = build_stat(font, plans)? {
        out.add_table(&stat)
            .map_err(|e| SliceError::Write(e.to_string()))?;
    }

    // MVAR is applied at the new default and then dropped; see the module docs.
    let location = super::normalize::NormalizedLocation {
        coords: plans
            .iter()
            .map(|plan| {
                if plan.is_pinned() {
                    plan.normalized.default
                } else {
                    0.0
                }
            })
            .collect(),
        tags: plans
            .iter()
            .map(|plan| Tag::new_checked(plan.spec.tag.as_bytes()).unwrap_or(Tag::new(b"    ")))
            .collect(),
    };
    let adjustments = super::mvar::metric_adjustments(font, &location);
    if let Ok(os2) = font.os2() {
        let mut os2: write_fonts::tables::os2::Os2 = os2.to_owned_table();
        super::mvar::apply_to_os2(&mut os2, &adjustments);
        out.add_table(&os2)
            .map_err(|e| SliceError::Write(e.to_string()))?;
    }
    if let Ok(post) = font.post() {
        let mut post: write_fonts::tables::post::Post = post.to_owned_table();
        super::mvar::apply_to_post(&mut post, &adjustments);
        out.add_table(&post)
            .map_err(|e| SliceError::Write(e.to_string()))?;
    }

    // Tables whose contents describe the old axis space and are rebuilt or dropped here.
    const REPLACED: &[Tag] = &[
        Tag::new(b"fvar"),
        Tag::new(b"avar"),
        Tag::new(b"gvar"),
        Tag::new(b"cvar"),
        Tag::new(b"STAT"),
        // Dropped: for glyf outlines the advance variation lives in the gvar phantom
        // points, which have been rebased along with everything else.
        Tag::new(b"HVAR"),
        Tag::new(b"VVAR"),
        // Applied above, then dropped.
        Tag::new(b"MVAR"),
    ];
    super::statics::copy_remaining_tables(&mut out, font, REPLACED);

    Ok(out.build())
}

/// Refuse fonts whose variation data this cannot rewrite.
///
/// Producing a font that is subtly wrong is worse than producing none, and both
/// alternatives here are subtly wrong: leaving these tables keeps regions that describe
/// an axis space that no longer exists, and removing them leaves the indices that point
/// into them dangling.
fn refuse_unsupported_variation_tables(font: &FontRef) -> Result<(), SliceError> {
    if let Ok(gdef) = font.gdef() {
        if gdef.item_var_store().is_some() {
            return Err(SliceError::Unsupported(
                "This font stores variable positioning data (a GDEF item variation \
                 store, used for variable kerning and anchors). Restricting an axis \
                 would leave that data describing the old design space, and this build \
                 cannot rewrite it yet. Pin every axis instead, which produces a static \
                 instance."
                    .into(),
            ));
        }
    }
    Ok(())
}

/// Rebuild `fvar` over the surviving axes.
fn build_fvar(font: &FontRef, plans: &[AxisPlan]) -> Result<Fvar, SliceError> {
    let source = font.fvar()?;

    let mut axes = Vec::new();
    for (index, plan) in plans.iter().enumerate() {
        if plan.is_pinned() {
            continue;
        }
        let record = source
            .axes()?
            .get(index)
            .ok_or_else(|| SliceError::Read("fvar axis disappeared while instancing".into()))?;
        let (min, default, max) = plan.output_extent();
        axes.push(VariationAxisRecord {
            axis_tag: record.axis_tag(),
            min_value: Fixed::from_f64(min),
            default_value: Fixed::from_f64(default),
            max_value: Fixed::from_f64(max),
            flags: record.flags(),
            axis_name_id: record.axis_name_id(),
        });
    }

    // Named instances survive only if they sit at the pinned location and inside the
    // new ranges; otherwise they would name a place the font can no longer reach.
    let mut instances = Vec::new();
    for instance in source.instances()?.iter() {
        let Ok(instance) = instance else { continue };
        let coords = instance.coordinates;

        let mut keep = true;
        let mut kept_coords = Vec::new();
        for (index, plan) in plans.iter().enumerate() {
            let value = coords
                .get(index)
                .map(|v| v.get().to_f64())
                .unwrap_or(plan.spec.default);
            match plan.limit {
                AxisLimit::Pin(pinned) => {
                    if value != pinned {
                        keep = false;
                        break;
                    }
                }
                AxisLimit::Range { min, max } => {
                    if value < min || value > max {
                        keep = false;
                        break;
                    }
                    kept_coords.push(Fixed::from_f64(value));
                }
                AxisLimit::Full => kept_coords.push(Fixed::from_f64(value)),
            }
        }
        if keep {
            instances.push(InstanceRecord {
                subfamily_name_id: instance.subfamily_name_id,
                flags: instance.flags,
                coordinates: kept_coords,
                post_script_name_id: instance.post_script_name_id,
            });
        }
    }

    Ok(Fvar::new(AxisInstanceArrays::new(axes, instances)))
}

/// Rebuild `avar` over the surviving axes.
///
/// A segment map is a function on normalized coordinates, and the normalized coordinates
/// have just changed meaning, so each retained mapping has to be expressed against the
/// new extents: its input renormalized through the axis's new range, its output through
/// the range that same map produces.
fn build_avar(font: &FontRef, plans: &[AxisPlan]) -> Result<Option<Avar>, SliceError> {
    let Some((maps, has_v2)) = segment_maps(font) else {
        return Ok(None);
    };
    if has_v2 {
        return Err(SliceError::Unsupported(
            "This font has an avar version 2 table, whose extra variation store this \
             build cannot rewrite. Pin every axis instead, which produces a static \
             instance."
                .into(),
        ));
    }

    let mut out = Vec::new();
    for (index, plan) in plans.iter().enumerate() {
        if plan.is_pinned() {
            continue;
        }
        let Some(map) = maps.get(index) else {
            out.push(SegmentMaps::new(identity_segment_map()));
            continue;
        };
        if plan.is_untouched() || map.len() < 3 {
            out.push(SegmentMaps::new(
                map.iter()
                    .map(|(from, to)| AxisValueMap {
                        from_coordinate: F2Dot14::from_f32(*from as f32),
                        to_coordinate: F2Dot14::from_f32(*to as f32),
                    })
                    .collect(),
            ));
            continue;
        }

        // The axis range *before* avar, which is what the map's inputs are stated in.
        let plain = AxisTriple::with_distances(
            quantize(normalize_axis(
                match plan.limit {
                    AxisLimit::Range { min, .. } => min,
                    _ => plan.spec.min,
                },
                &plan.spec,
            )),
            quantize(normalize_axis(plan.spec.default, &plan.spec)),
            quantize(normalize_axis(
                match plan.limit {
                    AxisLimit::Range { max, .. } => max,
                    _ => plan.spec.max,
                },
                &plan.spec,
            )),
            plan.spec.default - plan.spec.min,
            plan.spec.max - plan.spec.default,
        );

        // And the range the map turns that into, which is what its outputs are in.
        let mapped = AxisTriple::with_distances(
            quantize(apply_segment_map(plain.min, map)),
            quantize(apply_segment_map(plain.default, map)),
            quantize(apply_segment_map(plain.max, map)),
            plain.distance_negative,
            plain.distance_positive,
        );

        let mut pairs: BTreeMap<i16, i16> = BTreeMap::new();
        for (from, to) in map {
            if *from < plain.min || *from > plain.max {
                continue;
            }
            let new_from = quantize(plain.renormalize_value(*from));
            let new_to = quantize(mapped.renormalize_value(*to));
            pairs.insert(
                F2Dot14::from_f32(new_from as f32).to_bits(),
                F2Dot14::from_f32(new_to as f32).to_bits(),
            );
        }
        // The three fixed points every segment map must contain.
        for (from, to) in [(-1.0f64, -1.0f64), (0.0, 0.0), (1.0, 1.0)] {
            pairs.insert(
                F2Dot14::from_f32(from as f32).to_bits(),
                F2Dot14::from_f32(to as f32).to_bits(),
            );
        }

        out.push(SegmentMaps::new(
            pairs
                .into_iter()
                .map(|(from, to)| AxisValueMap {
                    from_coordinate: F2Dot14::from_bits(from),
                    to_coordinate: F2Dot14::from_bits(to),
                })
                .collect(),
        ));
    }

    if out.is_empty() {
        return Ok(None);
    }
    Ok(Some(Avar::new(out)))
}

fn identity_segment_map() -> Vec<AxisValueMap> {
    [(-1.0f32, -1.0f32), (0.0, 0.0), (1.0, 1.0)]
        .into_iter()
        .map(|(from, to)| AxisValueMap {
            from_coordinate: F2Dot14::from_f32(from),
            to_coordinate: F2Dot14::from_f32(to),
        })
        .collect()
}

/// Rebuild `STAT` without the pinned axes.
///
/// A variable font is required to have `STAT`, so it cannot simply be dropped; but its
/// design axis records are referenced by index from the axis value tables, so removing
/// one means renumbering the rest.
fn build_stat(
    font: &FontRef,
    plans: &[AxisPlan],
) -> Result<Option<write_fonts::tables::stat::Stat>, SliceError> {
    use write_fonts::tables::stat as wstat;

    let Ok(source) = font.stat() else {
        return Ok(None);
    };

    // STAT lists design axes in its own order, which need not match fvar's.
    let dropped: Vec<Tag> = plans
        .iter()
        .filter(|plan| plan.is_pinned())
        .filter_map(|plan| Tag::new_checked(plan.spec.tag.as_bytes()).ok())
        .collect();

    let Ok(design_axes) = source.design_axes() else {
        return Ok(None);
    };

    let mut records = Vec::new();
    // Old STAT axis index -> new index, or None when the axis is going away.
    let mut remap: Vec<Option<u16>> = Vec::new();
    for axis in design_axes.iter() {
        if dropped.contains(&axis.axis_tag()) {
            remap.push(None);
        } else {
            remap.push(Some(records.len() as u16));
            records.push(wstat::AxisRecord {
                axis_tag: axis.axis_tag(),
                axis_name_id: axis.axis_name_id(),
                axis_ordering: axis.axis_ordering(),
            });
        }
    }

    if records.is_empty() {
        // Every axis STAT described is gone; the caller is producing something with no
        // axes left for it to describe.
        return Ok(None);
    }

    let mut values: Vec<wstat::AxisValue> = Vec::new();
    if let Some(Ok(subtables)) = source.offset_to_axis_values() {
        for subtable in subtables.axis_values().iter() {
            let Ok(subtable) = subtable else { continue };
            if let Some(value) = remap_axis_value(&subtable, &remap) {
                values.push(value);
            }
        }
    }

    // A missing elidedFallbackNameID means STAT version 1.0, which predates the field;
    // nameID 2 (Subfamily) is the conventional stand-in.
    let fallback = source
        .elided_fallback_name_id()
        .unwrap_or(write_fonts::types::NameId::new(2));
    Ok(Some(wstat::Stat::new(records, values, fallback)))
}

/// Renumber one STAT axis value, or drop it if it refers to an axis that is going away.
fn remap_axis_value(
    value: &read_fonts::tables::stat::AxisValue,
    remap: &[Option<u16>],
) -> Option<write_fonts::tables::stat::AxisValue> {
    use read_fonts::tables::stat::AxisValue as R;
    use write_fonts::tables::stat as w;

    let index_of = |old: u16| -> Option<u16> { *remap.get(old as usize)? };

    match value {
        R::Format1(v) => {
            let index = index_of(v.axis_index())?;
            Some(w::AxisValue::format_1(
                index,
                v.flags(),
                v.value_name_id(),
                v.value(),
            ))
        }
        R::Format2(v) => {
            let index = index_of(v.axis_index())?;
            Some(w::AxisValue::format_2(
                index,
                v.flags(),
                v.value_name_id(),
                v.nominal_value(),
                v.range_min_value(),
                v.range_max_value(),
            ))
        }
        R::Format3(v) => {
            let index = index_of(v.axis_index())?;
            Some(w::AxisValue::format_3(
                index,
                v.flags(),
                v.value_name_id(),
                v.value(),
                v.linked_value(),
            ))
        }
        R::Format4(v) => {
            // A format 4 record names a point in several axes at once. If any of them is
            // going away the record no longer describes a reachable location, so it goes
            // too rather than being silently reduced to something else.
            let mut records = Vec::new();
            for record in v.axis_values() {
                let index = index_of(record.axis_index())?;
                records.push(w::AxisValueRecord {
                    axis_index: index,
                    value: record.value(),
                });
            }
            if records.is_empty() {
                return None;
            }
            Some(w::AxisValue::format_4(
                v.flags(),
                v.value_name_id(),
                records,
            ))
        }
    }
}
