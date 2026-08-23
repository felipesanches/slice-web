//! Does a partially instanced font still draw the same shapes?
//!
//! Same idea as `static_instance_matches_skrifa`, but harder, because the output is
//! still variable. The property is:
//!
//! > for any location L inside the restricted design space,
//! > the sliced font drawn at L must match the original drawn at L.
//!
//! User-space axis coordinates survive slicing — `wght=600` means the same thing before
//! and after — so the two can be compared at the same coordinates directly. Pinned axes
//! are held at their pinned values on the original side.
//!
//! This is the check that the sub-space solver, the tuple rebasing, the `avar` rewrite
//! and the `fvar` rewrite all agree with each other. Any one of them being wrong shows
//! up as outlines that drift as you move along the surviving axis, which is exactly what
//! sampling several locations inside the range is for.
//!
//! Run with `cargo test -p slice-core --test partial_instance_matches_skrifa -- --nocapture`
//! to see the largest deviation observed.

use read_fonts::TableProvider;
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::prelude::*;
use skrifa::{FontRef, MetadataProvider};

use slice_core::axes::AxisLimit;
use slice_core::instancer::{instantiate_partial, plan_axes};
use slice_core::SliceFont;

const RECURSIVE_VF: &[u8] = include_bytes!("../../../testdata/fonts/Recursive-VF.subset.ttf");

/// Deltas are rounded to integers in `gvar`, and the outline they are added to is
/// rounded too, so a unit of drift is expected where a static instance would be exact.
const TOLERANCE: f32 = 1.0001;

#[derive(Default, PartialEq, Debug)]
struct Recorder {
    ops: Vec<(&'static str, Vec<f32>)>,
}

impl OutlinePen for Recorder {
    fn move_to(&mut self, x: f32, y: f32) {
        self.ops.push(("move", vec![x, y]));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.ops.push(("line", vec![x, y]));
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.ops.push(("quad", vec![cx, cy, x, y]));
    }
    fn curve_to(&mut self, a: f32, b: f32, c: f32, d: f32, x: f32, y: f32) {
        self.ops.push(("curve", vec![a, b, c, d, x, y]));
    }
    fn close(&mut self) {
        self.ops.push(("close", Vec::new()));
    }
}

fn draw(font: &FontRef, gid: GlyphId, location: LocationRef) -> Recorder {
    let mut pen = Recorder::default();
    if let Some(glyph) = font.outline_glyphs().get(gid) {
        glyph
            .draw(DrawSettings::unhinted(Size::unscaled(), location), &mut pen)
            .expect("glyph should draw");
    }
    pen
}

/// One case: what to ask for, and where to check the result.
struct Case {
    name: &'static str,
    /// Axis settings, by tag. Anything not listed keeps its whole range.
    limits: &'static [(&'static str, AxisLimit)],
    /// Locations to compare at, in user space.
    probes: &'static [&'static [(&'static str, f64)]],
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "pin everything except wght, which keeps its whole range",
            limits: &[
                ("MONO", AxisLimit::Pin(0.0)),
                ("CASL", AxisLimit::Pin(0.0)),
                ("slnt", AxisLimit::Pin(0.0)),
                ("CRSV", AxisLimit::Pin(0.5)),
            ],
            probes: &[
                &[("wght", 300.0)],
                &[("wght", 400.0)],
                &[("wght", 650.0)],
                &[("wght", 800.0)],
                &[("wght", 1000.0)],
            ],
        },
        Case {
            name: "restrict wght to 300:700, pin the rest",
            limits: &[
                ("MONO", AxisLimit::Pin(0.0)),
                ("CASL", AxisLimit::Pin(0.0)),
                ("slnt", AxisLimit::Pin(0.0)),
                ("CRSV", AxisLimit::Pin(0.5)),
                (
                    "wght",
                    AxisLimit::Range {
                        min: 300.0,
                        max: 700.0,
                    },
                ),
            ],
            probes: &[
                &[("wght", 300.0)],
                &[("wght", 375.0)],
                &[("wght", 500.0)],
                &[("wght", 612.0)],
                &[("wght", 700.0)],
            ],
        },
        Case {
            // A restricted range has to contain the axis default -- that is the Level 3
            // rule -- so this narrows the top end while keeping 300.
            name: "restrict wght with CASL pinned at its far end",
            limits: &[
                ("MONO", AxisLimit::Pin(0.0)),
                ("CASL", AxisLimit::Pin(1.0)),
                ("slnt", AxisLimit::Pin(0.0)),
                ("CRSV", AxisLimit::Pin(0.5)),
                (
                    "wght",
                    AxisLimit::Range {
                        min: 300.0,
                        max: 900.0,
                    },
                ),
            ],
            probes: &[
                &[("wght", 300.0), ("CASL", 1.0)],
                &[("wght", 550.0), ("CASL", 1.0)],
                &[("wght", 900.0), ("CASL", 1.0)],
            ],
        },
        Case {
            name: "two axes survive, one of them restricted",
            limits: &[
                ("MONO", AxisLimit::Pin(0.0)),
                ("slnt", AxisLimit::Pin(0.0)),
                ("CRSV", AxisLimit::Pin(0.5)),
                (
                    "wght",
                    AxisLimit::Range {
                        min: 300.0,
                        max: 800.0,
                    },
                ),
            ],
            probes: &[
                &[("wght", 300.0), ("CASL", 0.0)],
                &[("wght", 800.0), ("CASL", 0.0)],
                &[("wght", 300.0), ("CASL", 1.0)],
                &[("wght", 800.0), ("CASL", 1.0)],
                &[("wght", 550.0), ("CASL", 0.5)],
            ],
        },
        Case {
            name: "pin the slant axis, whose whole range is negative",
            limits: &[
                ("MONO", AxisLimit::Pin(0.0)),
                ("CASL", AxisLimit::Pin(0.0)),
                ("slnt", AxisLimit::Pin(-15.0)),
                ("CRSV", AxisLimit::Pin(1.0)),
            ],
            probes: &[
                &[("wght", 300.0), ("slnt", -15.0), ("CRSV", 1.0)],
                &[("wght", 700.0), ("slnt", -15.0), ("CRSV", 1.0)],
                &[("wght", 1000.0), ("slnt", -15.0), ("CRSV", 1.0)],
            ],
        },
    ]
}

/// Build the partial instance for a case.
fn slice(case: &Case) -> Vec<u8> {
    let font = SliceFont::load(RECURSIVE_VF.to_vec()).unwrap();
    let font_ref = font.font_ref().unwrap();
    let axes = font.axes().unwrap();

    let limits: Vec<AxisLimit> = axes
        .iter()
        .map(|axis| {
            case.limits
                .iter()
                .find(|(tag, _)| *tag == axis.tag)
                .map(|(_, limit)| *limit)
                .unwrap_or(AxisLimit::Full)
        })
        .collect();

    let plans = plan_axes(&font_ref, &axes, &limits);
    instantiate_partial(&font_ref, &plans).expect("partial instancing should succeed")
}

/// The full location to draw the *original* at, given a probe and the case's pins.
fn original_location(case: &Case, probe: &[(&str, f64)]) -> Vec<(String, f32)> {
    let font = SliceFont::load(RECURSIVE_VF.to_vec()).unwrap();
    let axes = font.axes().unwrap();

    axes.iter()
        .map(|axis| {
            // A probe value wins; otherwise a pin; otherwise the axis default.
            let value = probe
                .iter()
                .find(|(tag, _)| *tag == axis.tag)
                .map(|(_, v)| *v)
                .or_else(|| {
                    case.limits.iter().find_map(|(tag, limit)| {
                        if *tag != axis.tag {
                            return None;
                        }
                        match limit {
                            AxisLimit::Pin(v) => Some(*v),
                            _ => None,
                        }
                    })
                })
                .unwrap_or(axis.default);
            (axis.tag.clone(), value as f32)
        })
        .collect()
}

#[test]
fn outlines_match_skrifa_across_the_restricted_space() {
    let original = FontRef::new(RECURSIVE_VF).unwrap();
    let glyph_count = original.maxp().unwrap().num_glyphs();
    let mut worst = 0.0f32;
    let mut worst_where = String::new();

    for case in cases() {
        let sliced_bytes = slice(&case);
        let sliced = FontRef::new(&sliced_bytes).expect("the result should parse");

        // The output must still be variable, with only the surviving axes.
        let surviving: Vec<String> = sliced
            .axes()
            .iter()
            .map(|axis| axis.tag().to_string())
            .collect();
        let expected: Vec<String> = SliceFont::load(RECURSIVE_VF.to_vec())
            .unwrap()
            .axes()
            .unwrap()
            .iter()
            .filter(|axis| {
                !case
                    .limits
                    .iter()
                    .any(|(tag, limit)| *tag == axis.tag && matches!(limit, AxisLimit::Pin(_)))
            })
            .map(|axis| axis.tag.clone())
            .collect();
        assert_eq!(surviving, expected, "{}: wrong axes survived", case.name);

        for probe in case.probes {
            let sliced_location = sliced.axes().location(
                probe
                    .iter()
                    .map(|(tag, value)| (*tag, *value as f32))
                    .collect::<Vec<_>>(),
            );
            let original_settings = original_location(&case, probe);
            let original_loc = original.axes().location(
                original_settings
                    .iter()
                    .map(|(tag, value)| (tag.as_str(), *value))
                    .collect::<Vec<_>>(),
            );

            for gid in 0..glyph_count {
                let gid = GlyphId::new(gid as u32);
                let reference = draw(&original, gid, (&original_loc).into());
                let actual = draw(&sliced, gid, (&sliced_location).into());

                assert_eq!(
                    reference.ops.len(),
                    actual.ops.len(),
                    "{}: at {probe:?} glyph {} has a different number of operations",
                    case.name,
                    gid.to_u32()
                );

                for (index, (r, a)) in reference.ops.iter().zip(&actual.ops).enumerate() {
                    assert_eq!(
                        r.0,
                        a.0,
                        "{}: at {probe:?} glyph {} operation {index} differs in kind",
                        case.name,
                        gid.to_u32()
                    );
                    for (rc, ac) in r.1.iter().zip(a.1.iter()) {
                        let diff = (rc - ac).abs();
                        if diff > worst {
                            worst = diff;
                            worst_where =
                                format!("{} at {probe:?} glyph {}", case.name, gid.to_u32());
                        }
                        assert!(
                            diff <= TOLERANCE,
                            "{}: at {probe:?} glyph {} operation {index} differs by {diff} \
                             units\n  original: {r:?}\n  sliced:   {a:?}",
                            case.name,
                            gid.to_u32()
                        );
                    }
                }
            }
        }
    }

    println!("largest outline deviation: {worst} font units (at {worst_where})");
}

#[test]
fn advance_widths_match_across_the_restricted_space() {
    let original = FontRef::new(RECURSIVE_VF).unwrap();
    let glyph_count = original.maxp().unwrap().num_glyphs();
    let mut worst = 0.0f32;

    for case in cases() {
        let sliced_bytes = slice(&case);
        let sliced = FontRef::new(&sliced_bytes).unwrap();

        // HVAR is dropped on the way through, so this also checks the claim that the
        // phantom points in gvar carry the advance variation on their own.
        assert!(
            sliced.hvar().is_err(),
            "{}: HVAR should have been dropped",
            case.name
        );

        for probe in case.probes {
            let sliced_location = sliced.axes().location(
                probe
                    .iter()
                    .map(|(tag, value)| (*tag, *value as f32))
                    .collect::<Vec<_>>(),
            );
            let original_settings = original_location(&case, probe);
            let original_loc = original.axes().location(
                original_settings
                    .iter()
                    .map(|(tag, value)| (tag.as_str(), *value))
                    .collect::<Vec<_>>(),
            );

            let reference = original.glyph_metrics(Size::unscaled(), &original_loc);
            let actual = sliced.glyph_metrics(Size::unscaled(), &sliced_location);

            for gid in 0..glyph_count {
                let gid = GlyphId::new(gid as u32);
                let a = reference.advance_width(gid).unwrap_or(0.0);
                let b = actual.advance_width(gid).unwrap_or(0.0);
                let diff = (a - b).abs();
                worst = worst.max(diff);
                assert!(
                    diff <= TOLERANCE,
                    "{}: at {probe:?} glyph {} advance {a} vs {b}",
                    case.name,
                    gid.to_u32()
                );
            }
        }
    }

    println!("largest advance deviation: {worst} font units");
}

#[test]
fn a_restricted_axis_reports_its_new_extent() {
    let case = &cases()[1];
    let bytes = slice(case);
    let font = SliceFont::load(bytes).unwrap();
    let axes = font.axes().unwrap();

    assert_eq!(axes.len(), 1);
    assert_eq!(axes[0].tag, "wght");
    assert_eq!(
        (axes[0].min, axes[0].default, axes[0].max),
        (300.0, 300.0, 700.0)
    );
}

#[test]
fn named_instances_outside_the_new_range_are_dropped() {
    // Any named instance that survives must sit inside the extent the output declares,
    // otherwise it names a place the font can no longer reach.
    let case = &cases()[1];
    let bytes = slice(case);
    let font = FontRef::new(&bytes).unwrap();
    let Ok(fvar) = font.fvar() else { return };

    let axes = fvar.axes().unwrap();
    for instance in fvar.instances().unwrap().iter() {
        let Ok(instance) = instance else { continue };
        for (axis, coord) in axes.iter().zip(instance.coordinates.iter()) {
            let value = coord.get().to_f64();
            assert!(
                value >= axis.min_value().to_f64() && value <= axis.max_value().to_f64(),
                "instance sits at {} on {}, outside {}..{}",
                value,
                axis.axis_tag(),
                axis.min_value(),
                axis.max_value()
            );
        }
    }
}
