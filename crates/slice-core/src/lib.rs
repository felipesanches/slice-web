//! Slice engine: build custom design sub-spaces from variable fonts.
//!
//! This crate is deliberately free of browser-specific dependencies so that the whole
//! pipeline can be exercised from a native test binary. The browser UI ([`slice-web`])
//! and the command line ([`slice-cli`]) are both thin shells over the entry point here.
//!
//! The pipeline, in order:
//!
//! 1. [`SliceFont::load`] — read a font and report what the editors should show.
//! 2. [`axes`] — turn Axis Editor text into [`axes::AxisLimit`]s and validate them.
//! 3. [`instancer`] — restrict the design space (pin axes, narrow axis ranges).
//! 4. [`overlaps`] — merge overlapping contours, which the original Slice never did.
//! 5. [`names`] / [`bits`] — apply the Name Editor and Bit Flag Editor changes.
//! 6. [`SliceJob::run`] — drive all of the above and serialise the result.

pub mod axes;
pub mod bits;
pub mod finalize;
pub mod font;
pub mod instancer;
pub mod job;
pub mod names;
pub mod overlaps;
pub mod solver;

#[cfg(any(test, feature = "testdata"))]
pub mod testdata;

pub use axes::{AxisLimit, AxisSpec};
pub use bits::BitFlags;
pub use font::SliceFont;
pub use job::{OutputFormat, SliceJob, SliceOutput};
pub use names::NameEdits;
pub use overlaps::{remove_overlaps, OverlapReport};

use thiserror::Error;

/// Everything that can go wrong between opening a font and writing one out.
///
/// The wording of the axis-related variants deliberately tracks the original Slice's
/// error dialogs, because those messages are what existing users know.
#[derive(Debug, Error)]
pub enum SliceError {
    #[error("the file could not be read as a font: {0}")]
    Read(String),

    #[error("the font could not be written: {0}")]
    Write(String),

    #[error(
        "The font is missing the OpenType fvar table and is not recognized as a \
         variable font. Please try again with a font that includes the fvar table."
    )]
    NotVariable,

    #[error("'{value}' is not a valid {axis} axis value. Please enter a single numeric value and try again.")]
    AxisValue { value: String, axis: String },

    #[error("{value} is not a valid axis range definition for {axis}.")]
    AxisRange { value: String, axis: String },

    #[error(
        "The {axis} value {value} is outside the axis range {min} to {max} supported by \
         this font."
    )]
    AxisValueOutOfRange {
        axis: String,
        value: f64,
        min: f64,
        max: f64,
    },

    #[error(
        "The {axis} range {min}:{max} is not contained in the axis range {axis_min} to \
         {axis_max} supported by this font."
    )]
    AxisRangeOutOfRange {
        axis: String,
        min: f64,
        max: f64,
        axis_min: f64,
        axis_max: f64,
    },

    #[error(
        "The {axis} range {min}:{max} does not include the default axis value \
         ({default}).  This is currently a requirement."
    )]
    DefaultOutsideRange {
        axis: String,
        min: f64,
        max: f64,
        default: f64,
    },

    #[error(
        "Moving the default location of the {axis} axis (Level 4 sub-spacing) is not \
         supported yet. Drop the [default] part of the range and try again."
    )]
    DefaultMoveUnsupported { axis: String },

    #[error("the font has no axis named {axis}")]
    UnknownAxis { axis: String },

    #[error(
        "You requested the same design space that is supported in the font that you are \
         processing. Please define at least one axis location or restricted axis range."
    )]
    NothingToDo,

    #[error("failed to remove overlaps from glyph {glyph}: {reason}")]
    RemoveOverlaps { glyph: String, reason: String },

    #[error("{0}")]
    Unsupported(String),
}

impl From<read_fonts::ReadError> for SliceError {
    fn from(e: read_fonts::ReadError) -> Self {
        SliceError::Read(e.to_string())
    }
}
