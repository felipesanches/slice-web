//! Axis definitions as the user types them into the Axis Editor.
//!
//! The accepted syntax is the one the original Slice documents:
//!
//! | what the user wants | what they type | example |
//! |---|---|---|
//! | keep the full original axis range | nothing | |
//! | pin the axis to one location | a number | `400.0` |
//! | restrict the axis to a smaller range | `min:max` | `200:700` |
//!
//! A trailing `[default]` group (`200:700[400]`) parses but is rejected: moving the
//! default axis location is Level 4 sub-spacing, which we do not implement yet. The
//! original Slice accepts the same group in its regular expression and then throws the
//! value away, so this is a deliberate tightening — silently ignoring a default the user
//! asked for produces a font that is not what they requested.

use std::fmt;

use crate::SliceError;

/// What the user asked to happen to one axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AxisLimit {
    /// Editor cell left blank: the axis survives with its original extent.
    Full,
    /// A single numeric value: the axis is pinned and disappears from `fvar`.
    Pin(f64),
    /// A `min:max` range: the axis survives with a smaller extent.
    ///
    /// `stated_default` carries a `[default]` group the user wrote after the range. It
    /// is honoured only when it agrees with the axis's existing default; moving a
    /// default is Level 4 sub-spacing, which is not implemented.
    Range {
        min: f64,
        max: f64,
        stated_default: Option<f64>,
    },
}

impl AxisLimit {
    /// A range with no `[default]` group, which is the ordinary case.
    pub const fn range(min: f64, max: f64) -> Self {
        AxisLimit::Range {
            min,
            max,
            stated_default: None,
        }
    }

    /// True when this limit leaves the axis present in the output `fvar`.
    pub fn keeps_axis(&self) -> bool {
        !matches!(self, AxisLimit::Pin(_))
    }

    /// True when this limit asks for something other than the original design space.
    pub fn is_restriction(&self) -> bool {
        !matches!(self, AxisLimit::Full)
    }
}

impl fmt::Display for AxisLimit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AxisLimit::Full => Ok(()),
            AxisLimit::Pin(v) => write!(f, "{v}"),
            AxisLimit::Range { min, max, .. } => write!(f, "{min}:{max}"),
        }
    }
}

/// Parse one Axis Editor cell.
///
/// `axis_tag` is only used to build error messages that name the offending axis, the way
/// the original Slice's error dialogs do.
pub fn parse_axis_limit(input: &str, axis_tag: &str) -> Result<AxisLimit, SliceError> {
    let text = input.trim();
    if text.is_empty() {
        return Ok(AxisLimit::Full);
    }

    let Some(colon) = text.find(':') else {
        // A bare number pins the axis.
        let value = parse_number(text).ok_or_else(|| SliceError::AxisValue {
            value: text.to_string(),
            axis: axis_tag.to_string(),
        })?;
        return Ok(AxisLimit::Pin(value));
    };

    let (start_text, rest) = text.split_at(colon);
    let rest = &rest[1..];

    // An optional `[default]` suffix. Level 4 sub-spacing; parsed so we can reject it
    // with a message that explains why, rather than a generic parse failure.
    let (end_text, explicit_default) = match rest.find('[') {
        Some(bracket) => {
            let (end, tail) = rest.split_at(bracket);
            let tail = tail.trim();
            let inner = tail
                .strip_prefix('[')
                .and_then(|t| t.strip_suffix(']'))
                .ok_or_else(|| SliceError::AxisRange {
                    value: text.to_string(),
                    axis: axis_tag.to_string(),
                })?;
            (end, Some(inner.trim().to_string()))
        }
        None => (rest, None),
    };

    let start = parse_range_bound(start_text).ok_or_else(|| SliceError::AxisRange {
        value: text.to_string(),
        axis: axis_tag.to_string(),
    })?;
    let end = parse_range_bound(end_text).ok_or_else(|| SliceError::AxisRange {
        value: text.to_string(),
        axis: axis_tag.to_string(),
    })?;

    // A `[default]` group is carried through rather than refused here. Whether it can be
    // honoured depends on the axis, which parsing does not know about, so that decision
    // belongs in `AxisSpec::validate`.
    let stated_default = match explicit_default {
        Some(raw) => Some(
            parse_range_bound(&raw).ok_or_else(|| SliceError::AxisRange {
                value: text.to_string(),
                axis: axis_tag.to_string(),
            })?,
        ),
        None => None,
    };

    // Slice sorts the pair, so `800:400` means the same as `400:800`.
    let (min, max) = if start <= end {
        (start, end)
    } else {
        (end, start)
    };

    // `400:400` names one coordinate, not a span. An fvar axis whose min, default and
    // max all coincide varies over nothing, so this is a pin however it was spelled.
    if min == max {
        if let Some(stated) = stated_default {
            if stated != min {
                return Err(SliceError::AxisRange {
                    value: text.to_string(),
                    axis: axis_tag.to_string(),
                });
            }
        }
        return Ok(AxisLimit::Pin(min));
    }

    Ok(AxisLimit::Range {
        min,
        max,
        stated_default,
    })
}

/// A decimal number, optionally signed, optionally with an exponent.
///
/// Used for a pinned value. An exponent is allowed because `1e3` denotes 1000
/// unambiguously and is exactly representable in the `Fixed` 16.16 format axis
/// coordinates are stored in, so there is nothing to gain by refusing it.
///
/// `nan`, `inf` and `1_000` are rejected, which `str::parse::<f64>` alone would not do.
/// The first two have no representation in `Fixed` at all, so no such coordinate can
/// ever be written to a font; the third is Python and Rust literal syntax leaking into
/// a text field, and reading it as 1000 would be a guess.
fn parse_number(text: &str) -> Option<f64> {
    parse_decimal(text, true)
}

/// The bound of a range: the same grammar as a pinned value.
///
/// The original's range grammar is a regular expression with no production for an
/// exponent and none for a leading decimal point, but it is applied with `re.search`, so
/// rather than refusing what it cannot describe it matches a prefix and reads something
/// else: `300:1e3` becomes `300:1`, which sorts to `1:300`, clamps to `300:300` and
/// deletes the weight axis; `.25:1` becomes `25:1`. Neither is a reading of what was
/// typed.
///
/// Both bounds are read here the way fontTools' `parseLimits` reads them, since it is
/// fontTools' instancer that the result is handed to and a limit string learned from
/// `fonttools varLib.instancer --help` must not mean something different in this
/// program. Measured against fontTools 4.62.1: `wght=300:1e3` -> `(300, 1000)` and
/// `wght=.5:900` -> `(0.5, 900)`. It also matches the pin parser above, so a value does
/// not change meaning when a colon is typed after it.
fn parse_range_bound(text: &str) -> Option<f64> {
    parse_decimal(text, true)
}

fn parse_decimal(text: &str, allow_exponent: bool) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }

    let (mantissa, exponent) = match text.find(['e', 'E']) {
        Some(at) if allow_exponent => (&text[..at], Some(&text[at + 1..])),
        Some(_) => return None,
        None => (text, None),
    };

    if !is_plain_decimal(mantissa) {
        return None;
    }
    if let Some(exponent) = exponent {
        let digits = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
    }

    let value = text.parse::<f64>().ok()?;
    // Belt and braces: the grammar above already excludes them, but a coordinate that
    // is not finite must never reach the solver.
    value.is_finite().then_some(value)
}

/// `123`, `-1.5`, `.5` or `2.` — digits with at most one decimal point and no other
/// characters at all, which is what excludes `1_000` and `nan`.
fn is_plain_decimal(text: &str) -> bool {
    let body = text.strip_prefix(['+', '-']).unwrap_or(text);
    if body.is_empty() {
        return false;
    }
    let mut parts = body.splitn(2, '.');
    let integer = parts.next().unwrap_or("");
    let fraction = parts.next().unwrap_or("");
    if integer.is_empty() && fraction.is_empty() {
        return false;
    }
    integer.bytes().all(|b| b.is_ascii_digit()) && fraction.bytes().all(|b| b.is_ascii_digit())
}

/// One axis as the font declares it in `fvar`.
#[derive(Clone, Debug, PartialEq)]
pub struct AxisSpec {
    pub tag: String,
    pub min: f64,
    pub default: f64,
    pub max: f64,
    /// The axis name from the `name` table, when the font provides one.
    pub name: Option<String>,
    /// True when the axis is flagged hidden in `fvar`.
    pub hidden: bool,
}

impl AxisSpec {
    /// The read-only left-hand column of the Axis Editor: `min : max [default]`.
    pub fn range_label(&self) -> String {
        format!(
            "{} : {} [{}]",
            fmt_coord(self.min),
            fmt_coord(self.max),
            fmt_coord(self.default)
        )
    }

    /// The tooltip Slice shows on the axis row: a human-readable axis name.
    ///
    /// Registered axes and the Google Fonts axis registry names take precedence over the
    /// font's own `name` record, matching the original's `get_axis_name_string`.
    pub fn display_name(&self) -> Option<String> {
        well_known_axis_name(&self.tag)
            .map(str::to_string)
            .or_else(|| self.name.clone())
    }

    /// Validate one user entry against this axis, returning the limit to apply.
    pub fn validate(&self, limit: AxisLimit) -> Result<AxisLimit, SliceError> {
        match limit {
            AxisLimit::Full => Ok(limit),
            AxisLimit::Pin(value) => {
                if value < self.min || value > self.max {
                    return Err(SliceError::AxisValueOutOfRange {
                        axis: self.tag.clone(),
                        value,
                        min: self.min,
                        max: self.max,
                    });
                }
                Ok(limit)
            }
            AxisLimit::Range {
                min,
                max,
                stated_default,
            } => {
                // A stated default that agrees with the axis is a description of what
                // is already true, so it costs nothing to honour. One that disagrees is
                // Level 4 sub-spacing, which needs the default location moved.
                if let Some(stated) = stated_default {
                    if stated != self.default {
                        return Err(SliceError::DefaultMoveUnsupported {
                            axis: self.tag.clone(),
                        });
                    }
                }

                // Judged as typed, before any clamping. A range that excludes the
                // default cannot be compiled at all (Level 3), and saying so is more
                // use than reporting the extent it also happens to overshoot.
                if self.default < min || self.default > max {
                    return Err(SliceError::DefaultOutsideRange {
                        axis: self.tag.clone(),
                        min,
                        max,
                        default: self.default,
                    });
                }

                // A restriction is an intersection with the design space the font
                // already has, so overshooting its edge asks for nothing the user does
                // not get. Contrast a pin, above, which is an assertion that a location
                // exists and is refused when it does not.
                let min = min.max(self.min);
                let max = max.min(self.max);

                // An axis admitting exactly one coordinate is not an axis; it is a pin,
                // and writing it out would leave variation data that can never vary.
                if min == max {
                    return Ok(AxisLimit::Pin(min));
                }

                // A range that survives clamping unchanged asks for the axis the font
                // already has. Reporting it as `Full` keeps the "did the user actually
                // restrict anything?" question answerable by inspection.
                if min == self.min && max == self.max {
                    return Ok(AxisLimit::Full);
                }

                // The stated default has now been checked against this axis and agreed
                // with it; dropping it here stops it being re-judged downstream.
                Ok(AxisLimit::Range {
                    min,
                    max,
                    stated_default: None,
                })
            }
        }
    }
}

/// Format an axis coordinate the way `fvar` values are conventionally shown: as a plain
/// decimal, with a single trailing `.0` for whole numbers so `wght 300` reads as a
/// continuous coordinate rather than an index.
pub fn fmt_coord(value: f64) -> String {
    if value == value.trunc() && value.abs() < 1e15 {
        format!("{value:.1}")
    } else {
        let mut s = format!("{value:.4}");
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.push('0');
        }
        s
    }
}

/// Registered OpenType axes plus the Google Fonts axis registry entries that the
/// original Slice knows about.
fn well_known_axis_name(tag: &str) -> Option<&'static str> {
    Some(match tag {
        "ital" => "Italic",
        "opsz" => "Optical size",
        "slnt" => "Slant",
        "wdth" => "Width",
        "wght" => "Weight",
        "CASL" => "Casual",
        "CRSV" => "Cursive",
        "XPRN" => "Expression",
        "GRAD" => "Grade",
        "MONO" => "Monospace",
        "SOFT" => "Softness",
        "WONK" => "Wonky",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn axis() -> AxisSpec {
        AxisSpec {
            tag: "wght".into(),
            min: 300.0,
            default: 300.0,
            max: 1000.0,
            name: Some("Weight".into()),
            hidden: false,
        }
    }

    #[test]
    fn blank_entry_keeps_the_full_axis() {
        assert_eq!(parse_axis_limit("", "wght").unwrap(), AxisLimit::Full);
        assert_eq!(parse_axis_limit("   ", "wght").unwrap(), AxisLimit::Full);
    }

    #[test]
    fn bare_number_pins_the_axis() {
        assert_eq!(
            parse_axis_limit("400", "wght").unwrap(),
            AxisLimit::Pin(400.0)
        );
        assert_eq!(
            parse_axis_limit("400.5", "wght").unwrap(),
            AxisLimit::Pin(400.5)
        );
        assert_eq!(
            parse_axis_limit("-15", "slnt").unwrap(),
            AxisLimit::Pin(-15.0)
        );
    }

    #[test]
    fn colon_pair_restricts_the_axis() {
        assert_eq!(
            parse_axis_limit("200:700", "wght").unwrap(),
            AxisLimit::range(200.0, 700.0)
        );
        assert_eq!(
            parse_axis_limit(" 200 : 700 ", "wght").unwrap(),
            AxisLimit::range(200.0, 700.0)
        );
    }

    #[test]
    fn reversed_range_is_sorted_like_the_original() {
        assert_eq!(
            parse_axis_limit("800:400", "wght").unwrap(),
            AxisLimit::range(400.0, 800.0)
        );
    }

    #[test]
    fn non_numeric_entry_is_rejected() {
        for bad in ["abc", "40o", "NaN", "inf", "-inf", "4..0", "."] {
            assert!(
                parse_axis_limit(bad, "wght").is_err(),
                "{bad:?} should not parse"
            );
        }
    }

    #[test]
    fn scientific_notation_is_a_number_like_any_other() {
        // A pin is a single float, and `1e3` is how a great many tools -- Python's
        // float(), strtod, JSON -- spell one thousand. Refusing it would be refusing a
        // value the user can legitimately mean, so it pins at 1000.
        assert_eq!(
            parse_axis_limit("1e3", "wght").unwrap(),
            AxisLimit::Pin(1000.0)
        );
        assert_eq!(
            parse_axis_limit("3.5E2", "wght").unwrap(),
            AxisLimit::Pin(350.0)
        );
    }

    #[test]
    fn a_range_bound_reads_like_a_pinned_value() {
        // fontTools' `parseLimits` -- the parser for the instancer this program hands
        // its plan to -- reads `wght=300:1e3` as (300, 1000) and `wght=.5:900` as
        // (0.5, 900). Reading them any other way would make a limit string mean one
        // thing in `fonttools varLib.instancer` and another here.
        assert_eq!(
            parse_axis_limit("300:1e3", "wght").unwrap(),
            AxisLimit::range(300.0, 1000.0)
        );
        assert_eq!(
            parse_axis_limit("3e2:7e2", "wght").unwrap(),
            AxisLimit::range(300.0, 700.0)
        );
        assert_eq!(
            parse_axis_limit(".25:1", "CRSV").unwrap(),
            AxisLimit::range(0.25, 1.0)
        );
        // What is never acceptable is reading the digits and dropping the rest, which
        // is what the original's `re.search` does to all three.
        assert_ne!(
            parse_axis_limit("300:1e3", "wght").unwrap(),
            AxisLimit::range(1.0, 300.0)
        );
    }

    #[test]
    fn a_degenerate_range_is_a_pin() {
        // `400:400` asks for a range of zero width. That is a location, and an fvar
        // axis whose min, default and max coincide is not a meaningful axis.
        assert_eq!(
            parse_axis_limit("400:400", "wght").unwrap(),
            AxisLimit::Pin(400.0)
        );
        assert_eq!(
            parse_axis_limit("0:0", "slnt").unwrap(),
            AxisLimit::Pin(0.0)
        );
    }

    #[test]
    fn a_stated_default_that_agrees_with_the_font_is_accepted() {
        // fonttools' `min:max[default]` syntax lets the caller restate the default. When
        // the value restated is the one the font already has, nothing is being asked for
        // beyond the range itself, so honour it rather than refusing a no-op.
        let a = axis(); // wght 300 / 300 / 1000
        let limit = parse_axis_limit("300:700[300]", "wght").unwrap();
        assert_eq!(
            limit,
            AxisLimit::Range {
                min: 300.0,
                max: 700.0,
                stated_default: Some(300.0),
            }
        );
        assert_eq!(a.validate(limit).unwrap(), AxisLimit::range(300.0, 700.0));
    }

    #[test]
    fn a_stated_default_that_differs_is_refused_rather_than_silently_dropped() {
        // Moving the default is a real feature (fonttools implements it by rebuilding
        // every tuple around the new origin) and this program does not have it. Dropping
        // the bracket silently would hand back a font that is not what was asked for.
        let a = axis();
        let limit = parse_axis_limit("300:700[500]", "wght").unwrap();
        let err = a.validate(limit).unwrap_err();
        assert!(
            matches!(err, SliceError::DefaultMoveUnsupported { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn restricted_range_must_contain_the_default() {
        let a = axis();
        // wght default is 300, so a 400:700 range cannot be compiled.
        let err = a.validate(AxisLimit::range(400.0, 700.0)).unwrap_err();
        assert!(
            matches!(err, SliceError::DefaultOutsideRange { .. }),
            "got {err:?}"
        );

        // A range that does contain it is fine.
        assert!(a.validate(AxisLimit::range(300.0, 700.0)).is_ok());
    }

    #[test]
    fn a_pin_outside_the_original_extent_is_refused() {
        // A pin is an assertion about where the instance sits. Asking for wght=1200 on an
        // axis that stops at 1000 cannot be honoured, and quietly substituting 1000 would
        // produce a font whose weight is not the one requested.
        let a = axis();
        assert!(a.validate(AxisLimit::Pin(1200.0)).is_err());
        assert!(a.validate(AxisLimit::Pin(100.0)).is_err());
        assert!(a.validate(AxisLimit::Pin(1000.0)).is_ok());
    }

    #[test]
    fn a_range_overshooting_the_extent_is_clamped_to_it() {
        // A range is an intersection, not an assertion: "keep wght between 100 and 700"
        // is fully satisfiable on a 300..1000 axis by keeping 300..700. fonttools'
        // instancer clamps exactly this way, and refusing would make the common idiom of
        // typing `0:700` to mean "everything up to 700" an error.
        let a = axis();
        assert_eq!(
            a.validate(AxisLimit::range(100.0, 700.0)).unwrap(),
            AxisLimit::range(300.0, 700.0)
        );
        assert_eq!(
            a.validate(AxisLimit::range(100.0, 5000.0)).unwrap(),
            AxisLimit::Full
        );
    }

    #[test]
    fn a_range_clamped_down_to_a_single_point_becomes_a_pin() {
        // wght starts at 300, so `0:300` leaves exactly one reachable location.
        let a = axis();
        assert_eq!(
            a.validate(AxisLimit::range(0.0, 300.0)).unwrap(),
            AxisLimit::Pin(300.0)
        );
    }

    #[test]
    fn range_label_matches_the_original_axis_editor_column() {
        assert_eq!(axis().range_label(), "300.0 : 1000.0 [300.0]");
    }

    #[test]
    fn axis_names_prefer_the_registry_over_the_font() {
        let a = AxisSpec {
            tag: "MONO".into(),
            min: 0.0,
            default: 0.0,
            max: 1.0,
            name: Some("Monospacyness".into()),
            hidden: false,
        };
        assert_eq!(a.display_name().as_deref(), Some("Monospace"));

        let b = AxisSpec {
            tag: "XYZW".into(),
            name: Some("Bespoke".into()),
            ..a.clone()
        };
        assert_eq!(b.display_name().as_deref(), Some("Bespoke"));
    }
}
