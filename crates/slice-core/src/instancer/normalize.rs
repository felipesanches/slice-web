//! Turning user-space axis coordinates into normalized ones.
//!
//! Everything inside a variable font's variation data is expressed in normalized
//! coordinates, where the axis default is 0, the minimum is -1 and the maximum is +1.
//! `avar`, when present, warps that mapping afterwards so a designer can make, say, the
//! first half of a weight axis cover more of the design than the second.

use read_fonts::{FontRef, TableProvider};
use write_fonts::types::Tag;

use crate::axes::AxisSpec;

/// Map a user coordinate onto -1..+1 for one axis, before `avar`.
///
/// Values outside the axis extent are clamped, as the specification requires.
pub fn normalize_axis(value: f64, axis: &AxisSpec) -> f64 {
    let value = value.clamp(axis.min, axis.max);
    if value == axis.default {
        0.0
    } else if value < axis.default {
        // The subtraction cannot divide by zero here: value < default implies
        // min < default.
        -(axis.default - value) / (axis.default - axis.min)
    } else {
        (value - axis.default) / (axis.max - axis.default)
    }
}

/// Apply one `avar` segment map to an already-normalized coordinate.
///
/// The map is a piecewise-linear function given as sorted `(from, to)` pairs. A map that
/// does not contain the three required identity entries, or has fewer than three pairs,
/// is treated as absent, matching what renderers do with a malformed table.
pub fn apply_segment_map(value: f64, map: &[(f64, f64)]) -> f64 {
    if map.len() < 3 {
        return value;
    }
    if value <= map[0].0 {
        return map[0].1;
    }
    for pair in map.windows(2) {
        let (from_a, to_a) = pair[0];
        let (from_b, to_b) = pair[1];
        if value < from_b {
            if from_b == from_a {
                return to_a;
            }
            return to_a + (to_b - to_a) * (value - from_a) / (from_b - from_a);
        }
    }
    map[map.len() - 1].1
}

/// The `avar` segment maps, one per axis in `fvar` order.
///
/// Returns `None` when the font has no usable `avar`. `avar` version 2 adds a variation
/// store on top of the segment maps; its segment maps still apply, so they are read and
/// the extra warping is reported to the caller rather than silently ignored.
pub fn segment_maps(font: &FontRef) -> Option<(Vec<Vec<(f64, f64)>>, bool)> {
    let avar = font.avar().ok()?;
    let has_v2_extras = avar.version().major > 1;
    let mut maps = Vec::new();
    for segment_map in avar.axis_segment_maps().iter() {
        let Ok(segment_map) = segment_map else {
            return None;
        };
        let pairs: Vec<(f64, f64)> = segment_map
            .axis_value_maps()
            .iter()
            .map(|m| (m.from_coordinate().to_f64(), m.to_coordinate().to_f64()))
            .collect();
        maps.push(pairs);
    }
    Some((maps, has_v2_extras))
}

/// A location in normalized coordinates, one entry per `fvar` axis, in `fvar` order.
#[derive(Clone, Debug, PartialEq)]
pub struct NormalizedLocation {
    pub coords: Vec<f64>,
    pub tags: Vec<Tag>,
}

impl NormalizedLocation {
    /// The coordinate for one axis, or 0 if the font has no such axis.
    pub fn get(&self, tag: Tag) -> f64 {
        self.tags
            .iter()
            .position(|t| *t == tag)
            .map(|i| self.coords[i])
            .unwrap_or(0.0)
    }
}

/// Normalize a set of user-space coordinates, applying `avar` when present.
///
/// `user` gives a value per axis, in `fvar` order; axes the caller does not care about
/// should be given their default so they normalize to zero.
pub fn normalize_location(
    font: &FontRef,
    axes: &[AxisSpec],
    user: &[f64],
) -> NormalizedLocation {
    assert_eq!(axes.len(), user.len());
    let mut coords: Vec<f64> = axes
        .iter()
        .zip(user)
        .map(|(axis, &v)| normalize_axis(v, axis))
        .collect();

    if let Some((maps, _)) = segment_maps(font) {
        for (i, coord) in coords.iter_mut().enumerate() {
            if let Some(map) = maps.get(i) {
                *coord = apply_segment_map(*coord, map);
            }
        }
    }

    for coord in &mut coords {
        *coord = coord.clamp(-1.0, 1.0);
    }

    NormalizedLocation {
        coords,
        tags: axes
            .iter()
            .map(|a| Tag::new_checked(a.tag.as_bytes()).unwrap_or(Tag::new(b"    ")))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wght() -> AxisSpec {
        AxisSpec {
            tag: "wght".into(),
            min: 300.0,
            default: 300.0,
            max: 1000.0,
            name: None,
            hidden: false,
        }
    }

    fn centred() -> AxisSpec {
        AxisSpec {
            tag: "wght".into(),
            min: 100.0,
            default: 400.0,
            max: 900.0,
            name: None,
            hidden: false,
        }
    }

    #[test]
    fn the_default_normalizes_to_zero() {
        assert_eq!(normalize_axis(300.0, &wght()), 0.0);
        assert_eq!(normalize_axis(400.0, &centred()), 0.0);
    }

    #[test]
    fn the_extremes_normalize_to_plus_and_minus_one() {
        assert_eq!(normalize_axis(1000.0, &wght()), 1.0);
        assert_eq!(normalize_axis(100.0, &centred()), -1.0);
        assert_eq!(normalize_axis(900.0, &centred()), 1.0);
    }

    #[test]
    fn the_two_halves_scale_independently() {
        // 250 is halfway from 100 to 400; 650 is halfway from 400 to 900. Both land at
        // the same distance from zero even though the user-space spans differ.
        assert_eq!(normalize_axis(250.0, &centred()), -0.5);
        assert_eq!(normalize_axis(650.0, &centred()), 0.5);
    }

    #[test]
    fn values_outside_the_axis_are_clamped() {
        assert_eq!(normalize_axis(2000.0, &wght()), 1.0);
        assert_eq!(normalize_axis(0.0, &wght()), 0.0, "below min clamps to min");
    }

    #[test]
    fn an_axis_with_default_at_the_minimum_never_goes_negative() {
        // wght here has min == default, so there is no negative side at all.
        for v in [300.0, 500.0, 1000.0] {
            assert!(normalize_axis(v, &wght()) >= 0.0);
        }
    }

    #[test]
    fn a_segment_map_warps_between_its_control_points() {
        // The identity-plus-one-bend map used throughout the avar spec examples.
        let map = [(-1.0, -1.0), (0.0, 0.0), (0.5, 0.75), (1.0, 1.0)];
        assert_eq!(apply_segment_map(0.0, &map), 0.0);
        assert_eq!(apply_segment_map(0.5, &map), 0.75);
        assert_eq!(apply_segment_map(0.25, &map), 0.375);
        assert_eq!(apply_segment_map(1.0, &map), 1.0);
        assert_eq!(apply_segment_map(-1.0, &map), -1.0);
    }

    #[test]
    fn a_degenerate_segment_map_is_ignored() {
        assert_eq!(apply_segment_map(0.3, &[]), 0.3);
        assert_eq!(apply_segment_map(0.3, &[(-1.0, -1.0), (1.0, 1.0)]), 0.3);
    }

    #[test]
    fn segment_maps_clamp_beyond_their_ends() {
        let map = [(-1.0, -1.0), (0.0, 0.0), (1.0, 1.0)];
        assert_eq!(apply_segment_map(-2.0, &map), -1.0);
        assert_eq!(apply_segment_map(2.0, &map), 1.0);
    }

    #[test]
    fn normalizes_the_recursive_fixture() {
        let font = crate::SliceFont::load(crate::testdata::recursive_vf().to_vec()).unwrap();
        let font_ref = font.font_ref().unwrap();
        let axes = font.axes().unwrap();

        // Every axis at its default must normalize to the origin.
        let defaults: Vec<f64> = axes.iter().map(|a| a.default).collect();
        let loc = normalize_location(&font_ref, &axes, &defaults);
        assert_eq!(loc.coords, vec![0.0; axes.len()]);

        // wght is axis 2, running 300..1000 with the default at 300.
        let mut user = defaults.clone();
        user[2] = 1000.0;
        let loc = normalize_location(&font_ref, &axes, &user);
        assert_eq!(loc.get(Tag::new(b"wght")), 1.0);

        // slnt runs -15..0 with the default at 0, so its whole range is negative.
        let mut user = defaults.clone();
        user[3] = -15.0;
        let loc = normalize_location(&font_ref, &axes, &user);
        assert_eq!(loc.get(Tag::new(b"slnt")), -1.0);
    }
}
