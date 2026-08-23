//! Compare a font's variation data before and after partial instancing.
//!
//! # Question this answers
//!
//! When a partially instanced font draws differently from the original, is the problem
//! in which tuples survived, in the tents they were rebased onto, or in the deltas
//! themselves? This prints the axis extents, the `avar` segment maps and the `gvar`
//! tuples for one glyph, for the input and the output side by side.
//!
//! It is the tool that found the gvar entry array being positional: the tents in the
//! output were right and the deltas belonged to a different glyph, which is a very
//! specific signature and hard to see any other way.
//!
//! # Running it
//!
//! ```sh
//! cargo run -p slice-core --features testdata --example probe_partial
//! ```
//!
//! It pins every axis except `wght`, which is the simplest case that should be a no-op
//! for the surviving axis: every tuple that mentions only pinned axes should disappear,
//! and every `wght` tuple should come through with its deltas untouched.
use read_fonts::{FontRef, TableProvider};
use write_fonts::types::GlyphId;
use slice_core::axes::AxisLimit;
use slice_core::instancer::{instantiate_partial, plan_axes};
use slice_core::SliceFont;

fn dump_gvar(label: &str, font: &FontRef, gid: GlyphId) {
    println!("--- {label} ---");
    match font.fvar() {
        Ok(fvar) => {
            for a in fvar.axes().unwrap().iter() {
                println!("  axis {} {} .. {} [{}]", a.axis_tag(), a.min_value(), a.max_value(), a.default_value());
            }
        }
        Err(_) => println!("  no fvar"),
    }
    if let Ok(avar) = font.avar() {
        for (i, m) in avar.axis_segment_maps().iter().enumerate() {
            let m = m.unwrap();
            let pairs: Vec<String> = m.axis_value_maps().iter().map(|p| format!("{:.4}->{:.4}", p.from_coordinate().to_f64(), p.to_coordinate().to_f64())).collect();
            println!("  avar[{i}]: {}", pairs.join(" "));
        }
    } else { println!("  no avar"); }
    let Ok(gvar) = font.gvar() else { println!("  no gvar"); return };
    let Ok(Some(data)) = gvar.glyph_variation_data(gid) else { println!("  no data for gid"); return };
    for (i, t) in data.tuples().enumerate() {
        let peak: Vec<f64> = t.peak().values().iter().map(|v| v.get().to_f64()).collect();
        let s: Option<Vec<f64>> = t.intermediate_start().map(|x| x.values().iter().map(|v| v.get().to_f64()).collect());
        let e: Option<Vec<f64>> = t.intermediate_end().map(|x| x.values().iter().map(|v| v.get().to_f64()).collect());
        let d: Vec<String> = t.deltas().take(4).map(|d| format!("{}:({},{})", d.position, d.x_delta, d.y_delta)).collect();
        println!("  tuple {i}: peak={peak:?} inter={s:?}/{e:?} dense={} first={}", t.has_deltas_for_all_points(), d.join(" "));
    }
}

fn main() {
    let bytes = include_bytes!("../../../testdata/fonts/Recursive-VF.subset.ttf").to_vec();
    let sf = SliceFont::load(bytes).unwrap();
    let font = sf.font_ref().unwrap();
    let axes = sf.axes().unwrap();

    let limits: Vec<AxisLimit> = axes.iter().map(|a| match a.tag.as_str() {
        "wght" => AxisLimit::Full,
        _ => AxisLimit::Pin(a.default),
    }).collect();
    let plans = plan_axes(&font, &axes, &limits);
    for p in &plans {
        println!("plan {} limit {:?} normalized {:?}", p.spec.tag, p.limit, p.normalized);
    }
    let out = instantiate_partial(&font, &plans).unwrap();
    let sliced = FontRef::new(&out).unwrap();

    let gid = GlyphId::new(1);
    dump_gvar("original", &font, gid);
    dump_gvar("sliced", &sliced, gid);
}
