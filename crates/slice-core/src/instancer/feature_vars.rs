//! Resolving `FeatureVariations` at the new design space.
//!
//! A variable font can swap which lookups a feature runs depending on where you are in
//! the design space. Recursive uses it for exactly the thing it is famous for: at high
//! `CRSV` the `rvrn` feature substitutes `a` for the single-storey cursive `a`.
//!
//! Those substitutions are described by condition sets over `fvar` axis *indices* and
//! normalized coordinates, so slicing invalidates them twice over. Pin every axis and
//! there is no `fvar` left to evaluate a condition against, so the substitution silently
//! never fires — ask for a cursive instance and get a font with the wrong `a`. Pin only
//! some axes and it is worse: the surviving conditions still hold the old axis indices,
//! which now point at whatever axis moved into that slot.
//!
//! So the conditions have to be evaluated here, at slicing time. A record whose
//! conditions all hold at the new default has its substitutions folded into the feature
//! list unconditionally; a record that cannot hold any more is dropped; a record that
//! still depends on a surviving axis is kept, with its indices remapped and its ranges
//! renormalized onto the new axis extent.
//!
//! This is a port of `fontTools.varLib.instancer.featureVars`.

use std::collections::{HashMap, HashSet};

use read_fonts::{FontRef, TableProvider};
use write_fonts::tables::layout::{
    Condition, ConditionFormat1, ConditionSet, FeatureTableSubstitution, FeatureVariationRecord,
    FeatureVariations,
};
use write_fonts::types::{F2Dot14, Tag};
use write_fonts::{from_obj::ToOwnedTable, FontBuilder};

use super::partial::AxisPlan;
use crate::solver::AxisTriple;
use crate::SliceError;

/// Resolve `FeatureVariations` in `GSUB` and `GPOS` against the new axis limits.
pub fn instantiate_feature_variations(
    bytes: &[u8],
    plans: &[AxisPlan],
) -> Result<Vec<u8>, SliceError> {
    let font = FontRef::new(bytes).map_err(|e| SliceError::Read(e.to_string()))?;
    let mut builder = FontBuilder::new();
    let mut touched = false;

    if let Ok(gsub) = font.gsub() {
        let mut gsub: write_fonts::tables::gsub::Gsub = gsub.to_owned_table();
        if gsub.feature_variations.is_some() {
            let records = instantiate(
                gsub.feature_variations.as_ref().unwrap(),
                &mut gsub.feature_list,
                plans,
            );
            gsub.feature_variations = records.map(FeatureVariations::new).into();
            builder
                .add_table(&gsub)
                .map_err(|e| SliceError::Write(e.to_string()))?;
            touched = true;
        }
    }

    if let Ok(gpos) = font.gpos() {
        let mut gpos: write_fonts::tables::gpos::Gpos = gpos.to_owned_table();
        if gpos.feature_variations.is_some() {
            let records = instantiate(
                gpos.feature_variations.as_ref().unwrap(),
                &mut gpos.feature_list,
                plans,
            );
            gpos.feature_variations = records.map(FeatureVariations::new).into();
            builder
                .add_table(&gpos)
                .map_err(|e| SliceError::Write(e.to_string()))?;
            touched = true;
        }
    }

    if !touched {
        return Ok(bytes.to_vec());
    }
    super::statics::copy_remaining_tables(&mut builder, &font, &[]);
    Ok(builder.build())
}

/// Work through one table's records, mutating its feature list and returning whatever
/// records still describe a reachable condition.
fn instantiate(
    variations: &FeatureVariations,
    feature_list: &mut write_fonts::tables::layout::FeatureList,
    plans: &[AxisPlan],
) -> Option<Vec<FeatureVariationRecord>> {
    // Surviving axes keep their relative order, so a condition on one of them has to be
    // renumbered to its position in the new fvar.
    let mut axis_index_map: HashMap<usize, u16> = HashMap::new();
    let mut next = 0u16;
    for (index, plan) in plans.iter().enumerate() {
        if !plan.is_pinned() {
            axis_index_map.insert(index, next);
            next += 1;
        }
    }

    let mut applied = false;
    let mut default_substitutions: Option<FeatureTableSubstitution> = None;
    let mut new_records: Vec<FeatureVariationRecord> = Vec::new();
    let mut seen: HashSet<Vec<(u16, i16, i16)>> = HashSet::new();
    let mut hit_universal = false;

    for record in &variations.feature_variation_records {
        let mut record = record.clone();
        let (applies, keep, universal) = instantiate_record(&mut record, plans, &axis_index_map);

        if keep && is_unique(&record, &mut seen) {
            new_records.push(record.clone());
        }

        // The first record that holds at the new default becomes the font's ordinary
        // behaviour, and the features it replaces are remembered so a catch-all can put
        // them back for locations the surviving records do not cover.
        if applies && !applied {
            if let Some(substitution) = record.feature_table_substitution.as_ref() {
                let mut defaults = substitution.clone();
                for (slot, replacement) in defaults
                    .substitutions
                    .iter_mut()
                    .zip(substitution.substitutions.iter())
                {
                    let index = replacement.feature_index as usize;
                    if let Some(target) = feature_list.feature_records.get_mut(index) {
                        // The two carry the same Feature behind offsets of different
                        // widths (16-bit in the feature list, 32-bit in a substitution),
                        // so they are swapped through the value rather than the marker.
                        let previous = target.feature.as_ref().clone();
                        let incoming = replacement.alternate_feature.as_ref().clone();
                        slot.alternate_feature = previous.into();
                        target.feature = incoming.into();
                    }
                }
                default_substitutions = Some(defaults);
                applied = true;
            }
        }

        // Nothing after a record that always applies can ever be reached.
        if universal {
            hit_universal = true;
            break;
        }
    }

    if applied && !new_records.is_empty() && !hit_universal {
        if let Some(defaults) = default_substitutions {
            new_records.push(FeatureVariationRecord {
                // An empty condition set matches everywhere, which is what puts the
                // original features back for locations the surviving records miss.
                condition_set: Some(ConditionSet::new(Vec::new())).into(),
                feature_table_substitution: Some(defaults).into(),
            });
        }
    }

    (!new_records.is_empty()).then_some(new_records)
}

/// Evaluate one record against the new limits.
///
/// Returns `(applies, keep, universal)`: whether its conditions hold at the new default,
/// whether the record survives at all, and whether it now holds everywhere.
fn instantiate_record(
    record: &mut FeatureVariationRecord,
    plans: &[AxisPlan],
    axis_index_map: &HashMap<usize, u16>,
) -> (bool, bool, bool) {
    let mut applies = true;
    let mut should_keep = false;
    let mut new_conditions: Option<Vec<Condition>> = Some(Vec::new());

    let existing: Vec<Condition> = record
        .condition_set
        .as_ref()
        .map(|set| {
            set.conditions
                .iter()
                .map(|offset| offset.as_ref().clone())
                .collect()
        })
        .unwrap_or_default();

    for condition in existing {
        let Condition::Format1AxisRange(range) = &condition else {
            // Formats 2 to 5 arrived with a later revision of the specification and
            // describe conditions this cannot evaluate. Keep them untouched and assume
            // the record does not apply, which is what fontTools does.
            applies = false;
            if let Some(list) = new_conditions.as_mut() {
                list.push(condition.clone());
            }
            continue;
        };

        let axis = range.axis_index as usize;
        let min_value = range.filter_range_min_value.to_f64();
        let max_value = range.filter_range_max_value.to_f64();

        // An axis nobody restricted keeps its full normalized extent, which is what
        // fontTools assumes for axes absent from the limits.
        let triple = match plans.get(axis) {
            Some(plan) if !plan.is_untouched() => plan.normalized,
            _ => AxisTriple::new(-1.0, 0.0, 1.0),
        };

        if !(min_value <= triple.default && triple.default <= max_value) {
            applies = false;
        }

        // The condition can never hold anywhere in the new range.
        if triple.min > max_value || triple.max < min_value {
            new_conditions = None;
            break;
        }

        let Some(&new_index) = axis_index_map.get(&axis) else {
            // The axis is pinned. Its answer is now fixed and folded into `applies`, so
            // the condition itself has nothing left to say.
            continue;
        };

        if min_value > max_value || min_value > triple.max || max_value < triple.min {
            new_conditions = None;
            break;
        }

        let low = triple.renormalize_value(min_value.clamp(triple.min, triple.max));
        let high = triple.renormalize_value(max_value.clamp(triple.min, triple.max));
        should_keep = true;

        // A condition spanning the whole axis is always true and need not be written.
        if low != -1.0 || high != 1.0 {
            if let Some(list) = new_conditions.as_mut() {
                list.push(Condition::Format1AxisRange(ConditionFormat1 {
                    axis_index: new_index,
                    filter_range_min_value: F2Dot14::from_f32(low as f32),
                    filter_range_max_value: F2Dot14::from_f32(high as f32),
                }));
            }
        }
    }

    match new_conditions {
        Some(conditions) if should_keep => {
            let universal = conditions.is_empty();
            record.condition_set = if conditions.is_empty() {
                None.into()
            } else {
                Some(ConditionSet::new(conditions)).into()
            };
            (applies, true, universal)
        }
        _ => (applies, false, false),
    }
}

/// Has an identical condition set already been kept?
///
/// Limiting several records onto a smaller axis can collapse them onto the same
/// condition; keeping the duplicates would be harmless but wasteful, and fontTools drops
/// them.
fn is_unique(record: &FeatureVariationRecord, seen: &mut HashSet<Vec<(u16, i16, i16)>>) -> bool {
    let mut key = Vec::new();
    if let Some(set) = record.condition_set.as_ref() {
        for offset in &set.conditions {
            match offset.as_ref() {
                Condition::Format1AxisRange(range) => key.push((
                    range.axis_index,
                    range.filter_range_min_value.to_bits(),
                    range.filter_range_max_value.to_bits(),
                )),
                // A condition this cannot compare is assumed distinct.
                _ => return true,
            }
        }
    }
    seen.insert(key)
}

/// True when the font has feature variations that slicing would invalidate.
pub fn has_feature_variations(font: &FontRef) -> bool {
    let gsub = font
        .gsub()
        .map(|t| t.feature_variations().is_some())
        .unwrap_or(false);
    let gpos = font
        .gpos()
        .map(|t| t.feature_variations().is_some())
        .unwrap_or(false);
    gsub || gpos
}

/// Tables this pass rewrites.
pub const REWRITTEN: &[Tag] = &[Tag::new(b"GSUB"), Tag::new(b"GPOS")];
