//! Show how one glyph's points are computed at one location.
//!
//! # Question this answers
//!
//! When an instanced glyph does not land where a renderer says it should, which tuple is
//! responsible? This prints the normalized location, every `gvar` tuple for the glyph
//! with the scalar it contributes there, and the before/after coordinate of each point,
//! alongside what skrifa independently computes.
//!
//! It is the tool that found the two bugs the skrifa oracle first caught: IUP being
//! interpolated against already-modified coordinates, and normalized coordinates not
//! being quantized to F2Dot14.
//!
//! # Running it
//!
//! ```sh
//! cargo run -p slice-core --features testdata --example probe_glyph -- <gid> [tag=value ...]
//!
//! # the case that exposed the IUP bug:
//! cargo run -p slice-core --features testdata --example probe_glyph -- 2 wght=800 CASL=1
//! ```
//!
//! Reading the output: a point whose "after" value sits close to an integer got its
//! delta explicitly; one that lands mid-way between integers got it from IUP. Scalars
//! that are almost-but-not-quite 1 mean the location is missing a tuple peak it was
//! meant to hit, which is the signature of a quantization problem.

use read_fonts::TableProvider;
use skrifa::instance::Size;
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::MetadataProvider;
use write_fonts::types::GlyphId;

use slice_core::instancer::glyphs::{apply_gvar_deltas, ot_round, read_glyph, tuple_scalar};
use slice_core::instancer::normalize_location;
use slice_core::SliceFont;

const FONT: &[u8] = include_bytes!("../../../testdata/fonts/Recursive-VF.subset.ttf");

#[derive(Default)]
struct Dump(Vec<String>);

impl OutlinePen for Dump {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.push(format!("M {x} {y}"));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.push(format!("L {x} {y}"));
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.0.push(format!("Q {cx} {cy} {x} {y}"));
    }
    fn curve_to(&mut self, a: f32, b: f32, c: f32, d: f32, x: f32, y: f32) {
        self.0.push(format!("C {a} {b} {c} {d} {x} {y}"));
    }
    fn close(&mut self) {
        self.0.push("Z".into());
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let gid_num: u32 = args
        .next()
        .unwrap_or_else(|| "2".into())
        .parse()
        .expect("first argument should be a glyph id");
    let requested: Vec<(String, f64)> = args
        .map(|arg| {
            let (tag, value) = arg
                .split_once('=')
                .expect("axis settings look like wght=800");
            (tag.to_string(), value.parse().expect("axis value"))
        })
        .collect();

    let slice_font = SliceFont::load(FONT.to_vec()).unwrap();
    let font = slice_font.font_ref().unwrap();
    let axes = slice_font.axes().unwrap();
    let gid = GlyphId::new(gid_num);

    match font.avar() {
        Ok(avar) => println!("avar: version {:?}", avar.version()),
        Err(_) => println!("avar: absent"),
    }

    let user: Vec<f64> = axes
        .iter()
        .map(|axis| {
            requested
                .iter()
                .find(|(tag, _)| *tag == axis.tag)
                .map(|(_, v)| *v)
                .unwrap_or(axis.default)
        })
        .collect();
    for (axis, value) in axes.iter().zip(&user) {
        println!("  {} = {value}   (range {})", axis.tag, axis.range_label());
    }

    let location = normalize_location(&font, &axes, &user);
    println!("normalized (ours):   {:?}", location.coords);

    let skrifa_font = skrifa::FontRef::new(slice_font.data()).unwrap();
    let skrifa_location = skrifa_font.axes().location(
        requested
            .iter()
            .map(|(tag, value)| (tag.as_str(), *value as f32))
            .collect::<Vec<_>>(),
    );
    println!(
        "normalized (skrifa): {:?}",
        skrifa_location
            .coords()
            .iter()
            .map(|v| v.to_f32())
            .collect::<Vec<_>>()
    );

    println!("\ngvar tuples for glyph {gid_num}:");
    if let Ok(gvar) = font.gvar() {
        if let Ok(Some(data)) = gvar.glyph_variation_data(gid) {
            for (i, tuple) in data.tuples().enumerate() {
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
                println!(
                    "  tuple {i:2}: scalar {scalar:<20} peak {peak:?}{}{}",
                    if tuple.has_deltas_for_all_points() {
                        " dense"
                    } else {
                        " SPARSE"
                    },
                    if intermediate.is_some() {
                        " intermediate"
                    } else {
                        ""
                    },
                );
            }
        }
    }

    let before = read_glyph(&font, gid).unwrap();
    let mut after = before.clone();
    apply_gvar_deltas(&font, gid, &location, &mut after).unwrap();

    println!("\npoints (original -> resolved -> rounded):");
    for (i, (b, a)) in before.coords.iter().zip(&after.coords).enumerate() {
        let phantom = if i >= before.outline_len() {
            format!(" [phantom {}]", i - before.outline_len() + 1)
        } else {
            String::new()
        };
        let fractional = (a.0 - a.0.round()).abs() > 0.01 || (a.1 - a.1.round()).abs() > 0.01;
        println!(
            "  {i:3}: ({:7.1},{:7.1}) -> ({:12.4},{:12.4}) -> ({:5},{:5}){}{}",
            b.0,
            b.1,
            a.0,
            a.1,
            ot_round(a.0),
            ot_round(a.1),
            phantom,
            if fractional { "  <- interpolated" } else { "" },
        );
    }

    let mut pen = Dump::default();
    if let Some(glyph) = skrifa_font.outline_glyphs().get(gid) {
        glyph
            .draw(
                DrawSettings::unhinted(Size::unscaled(), &skrifa_location),
                &mut pen,
            )
            .unwrap();
    }
    println!("\nskrifa's drawing of the variable font at this location:");
    for op in &pen.0 {
        println!("  {op}");
    }
}
