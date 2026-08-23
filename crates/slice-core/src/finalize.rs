//! The bookkeeping a font needs after its outlines have changed.
//!
//! fontTools does most of this inside `TTFont.save()`, which is why the original Slice
//! never had to think about it: `maxp`, the `head` bounding box and `hhea`'s extremes are
//! all recalculated from `glyf` on the way out, and `instantiateVariableFont` sets the
//! weight and width classes from the location it pinned. Writing tables directly, as this
//! does, means doing all of it deliberately.
//!
//! Skipping it is not cosmetic. `maxp.maxPoints` is what a rasteriser sizes its point
//! buffers from, and removing overlaps *raises* a glyph's point count — a font whose
//! `maxp` was tight before would come out under-reporting. `OS/2.usWeightClass` is what
//! the operating system reads to place a face in a weight menu, so an instance pinned at
//! `wght=1000` that still says 300 is filed as Light.

use std::collections::BTreeSet;

use read_fonts::tables::glyf::Glyph as ReadGlyph;
use read_fonts::{FontRef, TableProvider};
use write_fonts::types::{Fixed, GlyphId, NameId, Tag};
use write_fonts::{from_obj::ToOwnedTable, FontBuilder};

use crate::SliceError;

/// What `finalize` needs to know that it cannot work out from the bytes alone.
#[derive(Clone, Debug, Default)]
pub struct Finalize {
    /// The location the output sits at, in user space, as `(tag, value)`.
    ///
    /// Only `wght`, `wdth` and `slnt` are consulted, and only for axes that ended up
    /// pinned or at a known default.
    pub location: Vec<(String, f64)>,
    /// nameIDs the *input* font's `fvar` and `STAT` referred to.
    ///
    /// Anything in here that the output no longer refers to is dead weight and is
    /// dropped. Collected with [`variation_name_ids`] before slicing starts.
    pub variation_name_ids_before: BTreeSet<u16>,
}

/// Per-glyph facts gathered in one pass over `glyf`.
#[derive(Default)]
struct GlyfStats {
    x_min: i16,
    y_min: i16,
    x_max: i16,
    y_max: i16,
    any_contours: bool,
    max_points: u16,
    max_contours: u16,
    max_composite_points: u16,
    max_composite_contours: u16,
    max_component_elements: u16,
    max_component_depth: u16,
    all_lsb_equals_x_min: bool,
    advance_width_max: u16,
    min_lsb: i16,
    min_rsb: i16,
    x_max_extent: i16,
    avg_char_width: i32,
}

/// The nameIDs a font's `fvar` and `STAT` refer to.
///
/// Only IDs above 255 are reported: everything below is reserved by the specification
/// for meanings that have nothing to do with variation, and must never be pruned.
pub fn variation_name_ids(font: &FontRef) -> BTreeSet<u16> {
    let mut used = BTreeSet::new();
    let mut note = |id: NameId| {
        if id.to_u16() > 255 {
            used.insert(id.to_u16());
        }
    };

    if let Ok(fvar) = font.fvar() {
        if let Ok(axes) = fvar.axes() {
            for axis in axes.iter() {
                note(axis.axis_name_id());
            }
        }
        if let Ok(instances) = fvar.instances() {
            for instance in instances.iter().flatten() {
                note(instance.subfamily_name_id);
                if let Some(id) = instance.post_script_name_id {
                    // 0xFFFF is the "no PostScript name" sentinel, not a real record.
                    if id.to_u16() != 0xFFFF {
                        note(id);
                    }
                }
            }
        }
    }

    if let Ok(stat) = font.stat() {
        if let Ok(axes) = stat.design_axes() {
            for axis in axes.iter() {
                note(axis.axis_name_id());
            }
        }
        if let Some(Ok(values)) = stat.offset_to_axis_values() {
            for value in values.axis_values().iter().flatten() {
                use read_fonts::tables::stat::AxisValue;
                note(match &value {
                    AxisValue::Format1(v) => v.value_name_id(),
                    AxisValue::Format2(v) => v.value_name_id(),
                    AxisValue::Format3(v) => v.value_name_id(),
                    AxisValue::Format4(v) => v.value_name_id(),
                });
            }
        }
        if let Some(id) = stat.elided_fallback_name_id() {
            note(id);
        }
    }

    used
}

/// Convert a font's `post` table for writing, repairing a legal shape write-fonts trips on.
///
/// A version 2.0 `post` carries a string pool for glyph names that are not in the
/// standard Macintosh set. A font all of whose glyphs *are* in that set has a
/// zero-length pool, which is perfectly legal and common. read-fonts reports an
/// empty range as `None`, and write-fonts then panics writing the table back out,
/// because for version 2.0 it requires the field to be present.
///
/// Found by the conformance corpus: every fixture with no extra glyph names crashed
/// this, and every fixture with at least one did not.
pub fn owned_post(font: &FontRef) -> Option<write_fonts::tables::post::Post> {
    let mut post: write_fonts::tables::post::Post = font.post().ok()?.to_owned_table();
    // `num_glyphs` being present is what marks this as a 2.0-shaped table; an absent
    // string pool alongside it means "no names beyond the standard set", not "missing".
    if post.num_glyphs.is_some() && post.string_data.is_none() {
        post.string_data = Some(Vec::new());
    }
    Some(post)
}

/// Recalculate everything that depends on the final outlines, and drop what is now dead.
pub fn finalize(bytes: &[u8], options: &Finalize) -> Result<Vec<u8>, SliceError> {
    let font = FontRef::new(bytes).map_err(|e| SliceError::Read(e.to_string()))?;
    let mut builder = FontBuilder::new();

    let stats = collect_glyf_stats(&font)?;

    if let Some(stats) = &stats {
        // maxp version 1.0 is the only one with the fields worth recalculating; the
        // 0.5 form used by CFF fonts carries nothing but the glyph count.
        let mut maxp: write_fonts::tables::maxp::Maxp = font.maxp()?.to_owned_table();
        if maxp.max_points.is_some() {
            maxp.max_points = Some(stats.max_points);
            maxp.max_contours = Some(stats.max_contours);
            maxp.max_composite_points = Some(stats.max_composite_points);
            maxp.max_composite_contours = Some(stats.max_composite_contours);
            maxp.max_component_elements = Some(stats.max_component_elements);
            maxp.max_component_depth = Some(stats.max_component_depth);
        }
        builder
            .add_table(&maxp)
            .map_err(|e| SliceError::Write(e.to_string()))?;

        let mut head: write_fonts::tables::head::Head = font.head()?.to_owned_table();
        if stats.any_contours {
            head.x_min = stats.x_min;
            head.y_min = stats.y_min;
            head.x_max = stats.x_max;
            head.y_max = stats.y_max;
        }
        // head flags bit 1: every glyph's left side bearing equals its xMin, which lets
        // a rasteriser assume the outline starts at the origin.
        let lsb_at_x_zero = write_fonts::tables::head::Flags::from_bits_truncate(0x0002);
        if stats.all_lsb_equals_x_min {
            head.flags |= lsb_at_x_zero;
        } else {
            head.flags &= !lsb_at_x_zero;
        }
        builder
            .add_table(&head)
            .map_err(|e| SliceError::Write(e.to_string()))?;

        let mut hhea: write_fonts::tables::hhea::Hhea = font.hhea()?.to_owned_table();
        hhea.advance_width_max = write_fonts::types::UfWord::new(stats.advance_width_max);
        hhea.min_left_side_bearing = write_fonts::types::FWord::new(stats.min_lsb);
        hhea.min_right_side_bearing = write_fonts::types::FWord::new(stats.min_rsb);
        hhea.x_max_extent = write_fonts::types::FWord::new(stats.x_max_extent);
        builder
            .add_table(&hhea)
            .map_err(|e| SliceError::Write(e.to_string()))?;
    }

    // OS/2 and post carry the location's identity.
    if let Ok(os2) = font.os2() {
        let mut os2: write_fonts::tables::os2::Os2 = os2.to_owned_table();
        if let Some(stats) = &stats {
            os2.x_avg_char_width = stats.avg_char_width as i16;
        }
        if let Some(weight) = axis_value(&options.location, "wght") {
            os2.us_weight_class = weight.clamp(1.0, 1000.0).round() as u16;
        }
        if let Some(width) = axis_value(&options.location, "wdth") {
            os2.us_width_class = width_class(width);
        }
        builder
            .add_table(&os2)
            .map_err(|e| SliceError::Write(e.to_string()))?;
    }

    // Only rewritten when there is something to change; a table left alone is copied
    // through verbatim, which is both faster and impossible to get wrong.
    if let Some(slant) = axis_value(&options.location, "slnt") {
        if let Some(mut post) = owned_post(&font) {
            post.italic_angle = Fixed::from_f64(slant.clamp(-90.0, 90.0));
            builder
                .add_table(&post)
                .map_err(|e| SliceError::Write(e.to_string()))?;
        }
    }

    // Name records that only existed to name axes and instances the output no longer
    // has. fontTools compares the variation name IDs before and after rather than
    // pruning anything merely unreferenced, and so does this: a name reached from
    // somewhere else entirely, such as a stylistic set's UI label, must survive.
    let dead: BTreeSet<u16> = options
        .variation_name_ids_before
        .difference(&variation_name_ids(&font))
        .copied()
        .collect();
    if !dead.is_empty() {
        if let Ok(name) = font.name() {
            let mut name: write_fonts::tables::name::Name = name.to_owned_table();
            name.name_record
                .retain(|record| !dead.contains(&record.name_id.to_u16()));
            builder
                .add_table(&name)
                .map_err(|e| SliceError::Write(e.to_string()))?;
        }
    }

    // A digital signature signs bytes that no longer exist.
    crate::instancer::statics::copy_remaining_tables(&mut builder, &font, &[Tag::new(b"DSIG")]);
    Ok(builder.build())
}

fn axis_value(location: &[(String, f64)], tag: &str) -> Option<f64> {
    location
        .iter()
        .find(|(name, _)| name == tag)
        .map(|(_, value)| *value)
}

/// Map a `wdth` value onto `OS/2.usWidthClass`, interpolating between the nine
/// registered widths the way fontTools does.
fn width_class(width: f64) -> u16 {
    const TABLE: &[(f64, f64)] = &[
        (50.0, 1.0),
        (62.5, 2.0),
        (75.0, 3.0),
        (87.5, 4.0),
        (100.0, 5.0),
        (112.5, 6.0),
        (125.0, 7.0),
        (150.0, 8.0),
        (200.0, 9.0),
    ];
    let width = width.clamp(50.0, 200.0);
    let mut mapped = TABLE[TABLE.len() - 1].1;
    for pair in TABLE.windows(2) {
        let (from_a, to_a) = pair[0];
        let (from_b, to_b) = pair[1];
        if width <= from_a {
            mapped = to_a;
            break;
        }
        if width < from_b {
            mapped = to_a + (to_b - to_a) * (width - from_a) / (from_b - from_a);
            break;
        }
    }
    (mapped + 0.5).floor().clamp(1.0, 9.0) as u16
}

/// One pass over `glyf` collecting everything `maxp`, `head` and `hhea` need.
fn collect_glyf_stats(font: &FontRef) -> Result<Option<GlyfStats>, SliceError> {
    let (Ok(glyf), Ok(loca), Ok(hmtx), Ok(maxp)) =
        (font.glyf(), font.loca(None), font.hmtx(), font.maxp())
    else {
        // No outlines to measure; a CFF font keeps whatever it came with.
        return Ok(None);
    };

    let num_glyphs = maxp.num_glyphs();
    let mut stats = GlyfStats {
        x_min: i16::MAX,
        y_min: i16::MAX,
        x_max: i16::MIN,
        y_max: i16::MIN,
        all_lsb_equals_x_min: true,
        min_lsb: i16::MAX,
        min_rsb: i16::MAX,
        x_max_extent: i16::MIN,
        ..Default::default()
    };

    let mut advance_total: i64 = 0;
    let mut advance_count: i64 = 0;

    for gid in 0..num_glyphs {
        let gid = GlyphId::new(gid as u32);
        let advance = hmtx.advance(gid).unwrap_or(0);
        let lsb = hmtx.side_bearing(gid).unwrap_or(0);

        stats.advance_width_max = stats.advance_width_max.max(advance);
        if advance > 0 {
            advance_total += i64::from(advance);
            advance_count += 1;
        }

        let glyph = loca.get_glyf(gid, &glyf).ok().flatten();
        let Some(glyph) = glyph else {
            // An empty glyph has no outline and contributes nothing but its advance.
            continue;
        };

        let (x_min, y_min, x_max, y_max) = match &glyph {
            ReadGlyph::Simple(g) => (g.x_min(), g.y_min(), g.x_max(), g.y_max()),
            ReadGlyph::Composite(g) => (g.x_min(), g.y_min(), g.x_max(), g.y_max()),
        };

        stats.any_contours = true;
        stats.x_min = stats.x_min.min(x_min);
        stats.y_min = stats.y_min.min(y_min);
        stats.x_max = stats.x_max.max(x_max);
        stats.y_max = stats.y_max.max(y_max);

        if lsb != x_min {
            stats.all_lsb_equals_x_min = false;
        }
        stats.min_lsb = stats.min_lsb.min(lsb);
        let extent = i32::from(lsb) + (i32::from(x_max) - i32::from(x_min));
        stats.x_max_extent = stats.x_max_extent.max(extent.clamp(-32768, 32767) as i16);
        let rsb = i32::from(advance) - extent;
        stats.min_rsb = stats.min_rsb.min(rsb.clamp(-32768, 32767) as i16);

        match &glyph {
            ReadGlyph::Simple(g) => {
                let contours = g.number_of_contours().max(0) as u16;
                stats.max_points = stats.max_points.max(g.num_points() as u16);
                stats.max_contours = stats.max_contours.max(contours);
            }
            ReadGlyph::Composite(_) => {
                let (points, contours, depth) = composite_totals(&glyf, &loca, gid, 1, 0);
                stats.max_composite_points = stats.max_composite_points.max(points);
                stats.max_composite_contours = stats.max_composite_contours.max(contours);
                stats.max_component_depth = stats.max_component_depth.max(depth);
                if let Ok(Some(ReadGlyph::Composite(g))) = loca.get_glyf(gid, &glyf) {
                    stats.max_component_elements = stats
                        .max_component_elements
                        .max(g.components().count() as u16);
                }
            }
        }
    }

    if !stats.any_contours {
        stats.x_min = 0;
        stats.y_min = 0;
        stats.x_max = 0;
        stats.y_max = 0;
        stats.min_lsb = 0;
        stats.min_rsb = 0;
        stats.x_max_extent = 0;
    }
    stats.avg_char_width = if advance_count > 0 {
        ((advance_total as f64) / (advance_count as f64) + 0.5).floor() as i32
    } else {
        0
    };

    Ok(Some(stats))
}

/// Total points and contours reachable through a composite, and how deep it nests.
///
/// `guard` stops a malformed font whose composites reference each other in a cycle from
/// recursing forever; the depth cap is the one the specification imposes anyway.
fn composite_totals(
    glyf: &read_fonts::tables::glyf::Glyf,
    loca: &read_fonts::tables::loca::Loca,
    gid: GlyphId,
    depth: u16,
    guard: u16,
) -> (u16, u16, u16) {
    const MAX_DEPTH: u16 = 16;
    if guard >= MAX_DEPTH {
        return (0, 0, depth);
    }

    let Ok(Some(ReadGlyph::Composite(composite))) = loca.get_glyf(gid, glyf) else {
        return (0, 0, depth);
    };

    let mut points = 0u16;
    let mut contours = 0u16;
    let mut max_depth = depth;

    for component in composite.components() {
        let base = GlyphId::from(component.glyph);
        match loca.get_glyf(base, glyf) {
            Ok(Some(ReadGlyph::Simple(g))) => {
                points = points.saturating_add(g.num_points() as u16);
                contours = contours.saturating_add(g.number_of_contours().max(0) as u16);
            }
            Ok(Some(ReadGlyph::Composite(_))) => {
                let (p, c, d) = composite_totals(glyf, loca, base, depth + 1, guard + 1);
                points = points.saturating_add(p);
                contours = contours.saturating_add(c);
                max_depth = max_depth.max(d);
            }
            _ => {}
        }
    }

    (points, contours, max_depth)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_class_matches_the_registered_widths() {
        // The nine registered wdth values map to the nine width classes exactly.
        for (width, expected) in [
            (50.0, 1),
            (62.5, 2),
            (75.0, 3),
            (87.5, 4),
            (100.0, 5),
            (112.5, 6),
            (125.0, 7),
            (150.0, 8),
            (200.0, 9),
        ] {
            assert_eq!(width_class(width), expected, "wdth {width}");
        }
    }

    #[test]
    fn width_class_interpolates_and_clamps() {
        // Halfway between 100 and 112.5 rounds to the nearer class.
        assert_eq!(width_class(106.25), 6);
        assert_eq!(width_class(10.0), 1, "below the range clamps");
        assert_eq!(width_class(500.0), 9, "above the range clamps");
    }

    #[test]
    fn variation_name_ids_ignores_the_reserved_range() {
        let font = FontRef::new(crate::testdata::recursive_vf()).expect("the fixture should parse");
        let ids = variation_name_ids(&font);
        assert!(!ids.is_empty(), "the fixture has axis and instance names");
        assert!(
            ids.iter().all(|id| *id > 255),
            "IDs at or below 255 are reserved and must never be pruned"
        );
    }
}
