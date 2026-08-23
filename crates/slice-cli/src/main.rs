//! A command-line front end for the Slice engine.
//!
//! The browser is the product; this exists so the engine can be driven without one. It
//! is what the test harnesses shell out to, what makes a bug reproducible in a sentence
//! someone can paste, and what lets the engine be used in a build script.
//!
//! ```sh
//! slice info Recursive-VF.ttf
//! slice cut Recursive-VF.ttf out.ttf --axis wght=800 --axis CASL=1 --remove-overlaps
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use slice_core::axes::{fmt_coord, parse_axis_limit, AxisLimit};
use slice_core::names::{row_label, NAME_EDITOR_IDS};
use slice_core::{OutputFormat, SliceFont, SliceJob};

#[derive(Parser)]
#[command(
    name = "slice",
    about = "Build custom design sub-spaces from variable fonts",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Report what the editors would show for a font: axes, names, bit flags.
    Info {
        font: PathBuf,
        /// Print every name record, not just the ones the Name Editor exposes.
        #[arg(long)]
        all_names: bool,
    },

    /// Slice a font.
    Cut {
        font: PathBuf,
        output: PathBuf,

        /// An axis setting, repeatable. Uses the Axis Editor syntax:
        /// `wght=400` pins, `wght=200:700` restricts, omitting an axis keeps it whole.
        #[arg(long = "axis", value_name = "TAG=VALUE")]
        axes: Vec<String>,

        /// Merge overlapping contours. Needs every axis pinned.
        #[arg(long)]
        remove_overlaps: bool,

        /// Set a name record, repeatable: `--name 1='Family Name'`.
        #[arg(long = "name", value_name = "ID=TEXT")]
        names: Vec<String>,

        /// Set or clear an OS/2.fsSelection bit, repeatable: `--fs-selection 5=on`.
        #[arg(long = "fs-selection", value_name = "BIT=on|off")]
        fs_selection: Vec<String>,

        /// Set or clear a head.macStyle bit, repeatable: `--mac-style 0=on`.
        #[arg(long = "mac-style", value_name = "BIT=on|off")]
        mac_style: Vec<String>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Info { font, all_names } => info(font, all_names),
        Command::Cut {
            font,
            output,
            axes,
            remove_overlaps,
            names,
            fs_selection,
            mac_style,
        } => cut(
            font,
            output,
            axes,
            remove_overlaps,
            names,
            fs_selection,
            mac_style,
        ),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn load(path: &PathBuf) -> Result<SliceFont, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    SliceFont::load(bytes).map_err(|e| e.to_string())
}

fn info(path: PathBuf, all_names: bool) -> Result<(), String> {
    let font = load(&path)?;

    println!("{}", path.display());
    if let Some(family) = font.family_name() {
        print!("  {family}");
        if let Some(version) = font.version() {
            print!("  {version}");
        }
        println!();
    }
    println!(
        "  {} glyphs, {} units per em, {} outlines",
        font.glyph_count(),
        font.units_per_em(),
        if font.is_truetype() { "TrueType" } else { "CFF" }
    );

    // Only the Axis Editor needs a variable font. The name records and bit flags are
    // just as real in a static one, so they are still worth showing.
    if font.is_variable() {
        println!("\nAxis Editor");
        let axes = font.axes().map_err(|e| e.to_string())?;
        for axis in &axes {
            let name = axis.display_name().unwrap_or_default();
            let hidden = if axis.hidden { "  (hidden)" } else { "" };
            println!(
                "  {:<6} {:<28} {name}{hidden}",
                axis.tag,
                axis.range_label()
            );
        }
    } else {
        println!("\n  Not a variable font: no fvar table, so there is nothing to slice.");
    }

    println!("\nName Editor");
    let edits = font.name_edits();
    for &id in NAME_EDITOR_IDS {
        println!("  {:<20} {}", row_label(id), edits.get_or_empty(id));
    }

    if all_names {
        println!("\nAll name records");
        for id in 0..=25u16 {
            if let Some(text) = font.name_string(write_fonts_name_id(id)) {
                println!("  {id:>3}  {text}");
            }
        }
    }

    println!("\nBit Flag Editor");
    let bits = font.bit_flags();
    println!(
        "  OS/2.fsSelection  {}  ({})",
        slice_core::BitFlags::binary(bits.fs_selection),
        describe_bits(
            bits.fs_selection,
            slice_core::bits::OS2_FS_SELECTION.iter().map(|b| (b.offset, b.label))
        )
    );
    println!(
        "  head.macStyle     {}  ({})",
        slice_core::BitFlags::binary(bits.mac_style),
        describe_bits(
            bits.mac_style,
            slice_core::bits::HEAD_MAC_STYLE.iter().map(|b| (b.offset, b.label))
        )
    );
    for warning in bits.warnings() {
        println!("  warning: {warning}");
    }

    Ok(())
}

fn write_fonts_name_id(id: u16) -> write_fonts::types::NameId {
    write_fonts::types::NameId::new(id)
}

fn describe_bits<'a>(
    value: u16,
    definitions: impl Iterator<Item = (u8, &'a str)>,
) -> String {
    let set: Vec<&str> = definitions
        .filter(|(offset, _)| value & (1 << offset) != 0)
        .map(|(_, label)| label)
        .collect();
    if set.is_empty() {
        "none of the editable bits set".to_string()
    } else {
        set.join(", ")
    }
}

#[allow(clippy::too_many_arguments)]
fn cut(
    font_path: PathBuf,
    output_path: PathBuf,
    axis_args: Vec<String>,
    remove_overlaps: bool,
    name_args: Vec<String>,
    fs_selection_args: Vec<String>,
    mac_style_args: Vec<String>,
) -> Result<(), String> {
    let font = load(&font_path)?;
    if !font.is_variable() {
        return Err("this is not a variable font: it has no fvar table".into());
    }

    let axes = font.axes().map_err(|e| e.to_string())?;
    let mut job = SliceJob::new(&font).map_err(|e| e.to_string())?;

    for arg in &axis_args {
        let (tag, value) = arg
            .split_once('=')
            .ok_or_else(|| format!("--axis wants TAG=VALUE, got {arg:?}"))?;
        let index = axes
            .iter()
            .position(|a| a.tag == tag)
            .ok_or_else(|| format!("the font has no {tag} axis"))?;
        job.limits[index] = parse_axis_limit(value, tag).map_err(|e| e.to_string())?;
    }

    for arg in &name_args {
        let (id, text) = arg
            .split_once('=')
            .ok_or_else(|| format!("--name wants ID=TEXT, got {arg:?}"))?;
        let id: u16 = id
            .trim()
            .parse()
            .map_err(|_| format!("{id:?} is not a name ID"))?;
        job.names.set(id, text);
    }

    for arg in &fs_selection_args {
        let (bit, state) = parse_bit_arg(arg, "--fs-selection")?;
        job.bits.set_fs_selection_bit(bit, state);
    }
    for arg in &mac_style_args {
        let (bit, state) = parse_bit_arg(arg, "--mac-style")?;
        job.bits.set_mac_style_bit(bit, state);
    }

    job.remove_overlaps = remove_overlaps;
    job.format = OutputFormat::from_filename(&output_path.to_string_lossy());

    // Echo the request back, so a transcript says what was actually asked for.
    println!("Slicing {}", font_path.display());
    for (axis, limit) in axes.iter().zip(&job.limits) {
        let description = match limit {
            AxisLimit::Full => "whole original range".to_string(),
            AxisLimit::Pin(v) => format!("pinned at {}", fmt_coord(*v)),
            AxisLimit::Range { min, max } => {
                format!("restricted to {}:{}", fmt_coord(*min), fmt_coord(*max))
            }
        };
        println!("  {:<6} {description}", axis.tag);
    }

    let output = job.run(&font).map_err(|e| e.to_string())?;
    for note in &output.notes {
        println!("  {note}");
    }
    if let Some(report) = &output.overlaps {
        for (gid, reason) in &report.failed {
            eprintln!("  warning: glyph {gid} kept as it was: {reason}");
        }
    }

    std::fs::write(&output_path, &output.bytes)
        .map_err(|e| format!("could not write {}: {e}", output_path.display()))?;
    println!(
        "Wrote {} ({} bytes)",
        output_path.display(),
        output.bytes.len()
    );
    Ok(())
}

fn parse_bit_arg(arg: &str, flag: &str) -> Result<(u8, bool), String> {
    let (bit, state) = arg
        .split_once('=')
        .ok_or_else(|| format!("{flag} wants BIT=on|off, got {arg:?}"))?;
    let bit: u8 = bit
        .trim()
        .parse()
        .map_err(|_| format!("{bit:?} is not a bit number"))?;
    let state = match state.trim().to_ascii_lowercase().as_str() {
        "on" | "true" | "1" | "yes" => true,
        "off" | "false" | "0" | "no" => false,
        other => return Err(format!("{other:?} is not on or off")),
    };
    Ok((bit, state))
}
