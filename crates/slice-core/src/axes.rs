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
    Range { min: f64, max: f64 },
}

impl AxisLimit {
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
            AxisLimit::Range { min, max } => write!(f, "{min}:{max}"),
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

    let start = parse_number(start_text.trim()).ok_or_else(|| SliceError::AxisValue {
        value: start_text.trim().to_string(),
        axis: axis_tag.to_string(),
    })?;
    let end = parse_number(end_text.trim()).ok_or_else(|| SliceError::AxisValue {
        value: end_text.trim().to_string(),
        axis: axis_tag.to_string(),
    })?;

    if explicit_default.is_some() {
        return Err(SliceError::DefaultMoveUnsupported {
            axis: axis_tag.to_string(),
        });
    }

    // Slice sorts the pair, so `800:400` means the same as `400:800`.
    let (min, max) = if start <= end { (start, end) } else { (end, start) };
    Ok(AxisLimit::Range { min, max })
}

/// Accept exactly what Slice's regular expression accepts: an optionally signed decimal
/// number. Notably this rejects `1e3`, `inf` and `NaN`, all of which `str::parse::<f64>`
/// would happily take and none of which are meaningful axis coordinates.
fn parse_number(text: &str) -> Option<f64> {
    if text.is_empty() {
        return None;
    }
    let body = text.strip_prefix('-').unwrap_or(text);
    let mut parts = body.splitn(2, '.');
    let int_part = parts.next()?;
    if int_part.is_empty() || !int_part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if let Some(frac) = parts.next() {
        if frac.is_empty() || !frac.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
    }
    text.parse::<f64>().ok()
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
            AxisLimit::Range { min, max } => {
                if min < self.min || max > self.max {
                    return Err(SliceError::AxisRangeOutOfRange {
                        axis: self.tag.clone(),
                        min,
                        max,
                        axis_min: self.min,
                        axis_max: self.max,
                    });
                }
                // Level 3 sub-spacing: the compiler cannot move the default axis
                // location, so a restricted range has to contain it.
                if self.default < min || self.default > max {
                    return Err(SliceError::DefaultOutsideRange {
                        axis: self.tag.clone(),
                        min,
                        max,
                        default: self.default,
                    });
                }
                Ok(limit)
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
        assert_eq!(parse_axis_limit("400", "wght").unwrap(), AxisLimit::Pin(400.0));
        assert_eq!(
            parse_axis_limit("400.5", "wght").unwrap(),
            AxisLimit::Pin(400.5)
        );
        assert_eq!(parse_axis_limit("-15", "slnt").unwrap(), AxisLimit::Pin(-15.0));
    }

    #[test]
    fn colon_pair_restricts_the_axis() {
        assert_eq!(
            parse_axis_limit("200:700", "wght").unwrap(),
            AxisLimit::Range {
                min: 200.0,
                max: 700.0
            }
        );
        assert_eq!(
            parse_axis_limit(" 200 : 700 ", "wght").unwrap(),
            AxisLimit::Range {
                min: 200.0,
                max: 700.0
            }
        );
    }

    #[test]
    fn reversed_range_is_sorted_like_the_original() {
        assert_eq!(
            parse_axis_limit("800:400", "wght").unwrap(),
            AxisLimit::Range {
                min: 400.0,
                max: 800.0
            }
        );
    }

    #[test]
    fn non_numeric_entry_is_rejected() {
        for bad in ["abc", "40o", "1e3", "NaN", "inf", "4..0", "."] {
            assert!(
                parse_axis_limit(bad, "wght").is_err(),
                "{bad:?} should not parse"
            );
        }
    }

    #[test]
    fn explicit_default_is_rejected_rather_than_silently_dropped() {
        let err = parse_axis_limit("200:700[400]", "wght").unwrap_err();
        assert!(
            matches!(err, SliceError::DefaultMoveUnsupported { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn restricted_range_must_contain_the_default() {
        let a = axis();
        // wght default is 300, so a 400:700 range cannot be compiled.
        let err = a
            .validate(AxisLimit::Range {
                min: 400.0,
                max: 700.0,
            })
            .unwrap_err();
        assert!(matches!(err, SliceError::DefaultOutsideRange { .. }), "got {err:?}");

        // A range that does contain it is fine.
        assert!(a
            .validate(AxisLimit::Range {
                min: 300.0,
                max: 700.0
            })
            .is_ok());
    }

    #[test]
    fn limits_must_stay_inside_the_original_extent() {
        let a = axis();
        assert!(a.validate(AxisLimit::Pin(1200.0)).is_err());
        assert!(a.validate(AxisLimit::Pin(100.0)).is_err());
        assert!(a
            .validate(AxisLimit::Range {
                min: 100.0,
                max: 700.0
            })
            .is_err());
        assert!(a.validate(AxisLimit::Pin(1000.0)).is_ok());
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
