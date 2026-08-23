//! Does a sliced font still substitute the glyphs the design space said it would?
//!
//! Recursive's `rvrn` feature swaps `a` for a single-storey cursive `a` when `CRSV` is
//! high. That swap is described by a condition set over `fvar` axis indices and
//! normalized coordinates, so slicing invalidates it: pin every axis and there is no
//! `fvar` left to evaluate the condition against, and the substitution silently stops
//! happening. Someone asks Slice for a cursive instance and gets a font with the wrong
//! `a`, with nothing anywhere to say so.
//!
//! Outline comparisons cannot catch this. The glyphs themselves are all still correct
//! and in the right places; what changes is which glyph the shaper picks. So this looks
//! at the feature list directly.

use read_fonts::{FontRef, TableProvider};

use slice_core::axes::AxisLimit;
use slice_core::{OutputFormat, SliceFont, SliceJob};

const RECURSIVE_VF: &[u8] = include_bytes!("../../../testdata/fonts/Recursive-VF.subset.ttf");

/// Slice the fixture with the given per-axis settings, by tag.
fn slice(settings: &[(&str, AxisLimit)]) -> SliceFont {
    let font = SliceFont::load(RECURSIVE_VF.to_vec()).unwrap();
    let axes = font.axes().unwrap();
    let limits: Vec<AxisLimit> = axes
        .iter()
        .map(|axis| {
            settings
                .iter()
                .find(|(tag, _)| *tag == axis.tag)
                .map(|(_, limit)| *limit)
                .unwrap_or(AxisLimit::Pin(axis.default))
        })
        .collect();

    let job = SliceJob {
        limits,
        names: font.name_edits(),
        bits: font.bit_flags(),
        remove_overlaps: false,
        format: OutputFormat::Sfnt,
    };
    SliceFont::load(job.run(&font).expect("slicing should succeed").bytes).unwrap()
}

/// How many lookups each GSUB feature runs, by tag.
fn gsub_features(font: &FontRef) -> Vec<(String, usize)> {
    let Ok(gsub) = font.gsub() else {
        return Vec::new();
    };
    let Ok(list) = gsub.feature_list() else {
        return Vec::new();
    };
    list.feature_records()
        .iter()
        .map(|record| {
            let count = record
                .feature(list.offset_data())
                .map(|f| f.lookup_list_indices().len())
                .unwrap_or(0);
            (record.feature_tag().to_string(), count)
        })
        .collect()
}

fn has_feature_variations(font: &FontRef) -> bool {
    font.gsub()
        .map(|g| g.feature_variations().is_some())
        .unwrap_or(false)
}

#[test]
fn the_cursive_substitution_is_baked_in_when_it_applies() {
    // CRSV=1 is inside the condition that swaps in the cursive 'a'.
    let sliced = slice(&[("CRSV", AxisLimit::Pin(1.0))]);
    let font = sliced.font_ref().unwrap();

    assert!(!sliced.is_variable(), "every axis was pinned");
    assert!(
        !has_feature_variations(&font),
        "with no fvar left there is nothing to evaluate conditions against, so the \
         feature variations must have been resolved away rather than carried through"
    );

    let features = gsub_features(&font);
    let rvrn = features
        .iter()
        .find(|(tag, _)| tag == "rvrn")
        .unwrap_or_else(|| panic!("rvrn should still be present, found {features:?}"));
    assert!(
        rvrn.1 > 0,
        "rvrn runs no lookups, so the cursive substitution was lost: {features:?}"
    );
}

#[test]
fn the_substitution_stays_out_when_it_does_not_apply() {
    // CRSV at its default is outside the condition, so nothing should be substituted.
    let sliced = slice(&[("CRSV", AxisLimit::Pin(0.5))]);
    let font = sliced.font_ref().unwrap();

    assert!(!has_feature_variations(&font));
    let features = gsub_features(&font);
    for (tag, lookups) in &features {
        if tag == "rvrn" {
            assert_eq!(
                *lookups, 0,
                "rvrn should run nothing at the default CRSV: {features:?}"
            );
        }
    }
}

#[test]
fn conditions_on_a_surviving_axis_are_kept_and_renumbered() {
    // Keep CRSV variable and pin the rest. The conditions that mention CRSV have to
    // survive, and their axis index has to be renumbered: CRSV was axis 4 of five, and
    // is now the only axis, so every condition must point at index 0.
    let sliced = slice(&[("CRSV", AxisLimit::Full)]);
    let font = sliced.font_ref().unwrap();

    assert!(sliced.is_variable());
    assert_eq!(sliced.axes().unwrap().len(), 1, "only CRSV should survive");

    let gsub = font.gsub().unwrap();
    let Some(Ok(variations)) = gsub.feature_variations() else {
        panic!("the conditions on CRSV should have been kept");
    };

    let mut conditions = 0;
    for record in variations.feature_variation_records().iter() {
        let Some(Ok(set)) = record.condition_set(variations.offset_data()) else {
            continue;
        };
        for condition in set.conditions().iter().flatten() {
            if let read_fonts::tables::layout::Condition::Format1AxisRange(range) = condition {
                conditions += 1;
                assert_eq!(
                    range.axis_index(),
                    0,
                    "a condition still points at an axis index the output does not have"
                );
            }
        }
    }
    assert!(conditions > 0, "expected conditions on the surviving axis");
}
