//! A `STAT` axis value naming an axis that does not exist must not survive a slice.
//!
//! Found by sweeping google/fonts rather than by any hand-written case:
//! `axisregistry/tests/data/OpenSansCondensed-Italic[wght].ttf` carries a two-axis STAT
//! with an `AxisValue` whose `AxisIndex` is 2. fontTools 4.62.1 does not survive it at
//! all -- `instantiateVariableFont` raises `IndexError: list index out of range` from
//! `designAxes[axisValueTable.AxisIndex].AxisTag` -- and this program did survive it, by
//! copying the record straight through. Surviving where the reference crashes is only
//! worth something if the corruption is removed rather than passed on.
//!
//! The fixture here is built in-process rather than committed, because the point is the
//! shape of the defect and not any particular font.

use read_fonts::{FontRef, TableProvider};
use write_fonts::tables::stat as wstat;
use write_fonts::tables::stat::AxisValueTableFlags;
use write_fonts::types::{Fixed, NameId, Tag};
use write_fonts::FontBuilder;

/// A STAT with two design axes and three axis values, the last naming axis index 2.
fn stat_with_a_dangling_axis_index() -> Vec<u8> {
    let axes = vec![
        wstat::AxisRecord {
            axis_tag: Tag::new(b"wght"),
            axis_name_id: NameId::new(256),
            axis_ordering: 0,
        },
        wstat::AxisRecord {
            axis_tag: Tag::new(b"ital"),
            axis_name_id: NameId::new(257),
            axis_ordering: 1,
        },
    ];
    let values = vec![
        wstat::AxisValue::format_1(
            0,
            AxisValueTableFlags::empty(),
            NameId::new(258),
            Fixed::from_f64(400.0),
        ),
        wstat::AxisValue::format_1(
            1,
            AxisValueTableFlags::empty(),
            NameId::new(259),
            Fixed::from_f64(1.0),
        ),
        // Index 2, in a font whose design-axis array has two entries.
        wstat::AxisValue::format_1(
            2,
            AxisValueTableFlags::empty(),
            NameId::new(260),
            Fixed::from_f64(700.0),
        ),
    ];
    let stat = wstat::Stat::new(axes, values, NameId::new(2));
    let mut builder = FontBuilder::new();
    builder.add_table(&stat).unwrap();
    builder.build()
}

fn dangling_axis_values(font: &FontRef) -> usize {
    let stat = font.stat().expect("the output should still have a STAT");
    let axis_count = stat.design_axes().map(|a| a.len()).unwrap_or(0);
    let Some(Ok(subtables)) = stat.offset_to_axis_values() else {
        return 0;
    };
    subtables
        .axis_values()
        .iter()
        .flatten()
        .filter(|value| {
            use read_fonts::tables::stat::AxisValue;
            let index = match value {
                AxisValue::Format1(v) => v.axis_index(),
                AxisValue::Format2(v) => v.axis_index(),
                AxisValue::Format3(v) => v.axis_index(),
                // Format 4 carries several; any one dangling condemns the record.
                AxisValue::Format4(v) => v
                    .axis_values()
                    .iter()
                    .map(|r| r.axis_index())
                    .max()
                    .unwrap_or(0),
            };
            usize::from(index) >= axis_count
        })
        .count()
}

#[test]
fn the_source_really_does_carry_the_defect() {
    // Otherwise the test below could pass by testing nothing.
    let bytes = stat_with_a_dangling_axis_index();
    let font = FontRef::new(&bytes).unwrap();
    assert_eq!(
        dangling_axis_values(&font),
        1,
        "the hand-built fixture was supposed to contain exactly one dangling record"
    );
}

#[test]
fn a_dangling_axis_index_does_not_reach_the_output() {
    let bytes = stat_with_a_dangling_axis_index();
    let font = FontRef::new(&bytes).unwrap();

    // No fvar here, so every axis STAT names is one the plan says nothing about, which is
    // the case that used to keep the record: an unknown tag is left alone, and an
    // unresolvable *index* was being treated the same way.
    let rebuilt = slice_core::instancer::partial::build_stat(&font, &[])
        .expect("building a STAT from a valid table should not fail")
        .expect("a STAT with design axes should produce one");

    let mut builder = FontBuilder::new();
    builder.add_table(&rebuilt).unwrap();
    let out = builder.build();
    let out = FontRef::new(&out).unwrap();

    assert_eq!(
        dangling_axis_values(&out),
        0,
        "an AxisValue naming an axis the font does not have was copied through"
    );
    // The two well-formed records must survive; dropping everything would also pass the
    // assertion above and would be a different bug.
    let stat = out.stat().unwrap();
    let kept = stat
        .offset_to_axis_values()
        .and_then(|s| s.ok())
        .map(|s| s.axis_values().iter().flatten().count())
        .unwrap_or(0);
    assert_eq!(kept, 2, "the two valid axis values should have been kept");
}
