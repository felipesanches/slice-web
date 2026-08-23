//! The two number encodings CFF2 uses, and the INDEX container.
//!
//! CFF2 stores numbers twice over, in two incompatible ways: charstrings use the Type 2
//! encoding, where byte 255 introduces a 16.16 fixed-point value, and DICTs use the CFF
//! encoding, where 255 is not a number at all and 29 introduces a 32-bit integer. Getting
//! them the wrong way round produces a file that parses into different numbers rather
//! than one that fails to parse, so they are kept in separate functions with the format
//! named in each.

use crate::SliceError;

/// The largest DICT or charstring nesting this will follow before calling the input
/// malformed. Real fonts stay far below it; a cyclic subroutine reference does not.
pub const MAX_DEPTH: usize = 10;

fn malformed(what: &str) -> SliceError {
    SliceError::Read(format!("the CFF2 table is malformed: {what}"))
}

// ---------------------------------------------------------------- charstring numbers

/// Read one Type 2 charstring operand starting at `data[pos]`.
///
/// Returns the value and the number of bytes consumed. The caller has already
/// established that `data[pos]` is an operand rather than an operator.
pub fn read_charstring_number(data: &[u8], pos: usize) -> Result<(f64, usize), SliceError> {
    let b0 = *data
        .get(pos)
        .ok_or_else(|| malformed("truncated operand"))?;
    let byte = |i: usize| -> Result<i32, SliceError> {
        data.get(pos + i)
            .map(|b| i32::from(*b))
            .ok_or_else(|| malformed("truncated operand"))
    };
    Ok(match b0 {
        28 => (f64::from(((byte(1)? << 8) | byte(2)?) as i16), 3),
        32..=246 => (f64::from(i32::from(b0) - 139), 1),
        247..=250 => (f64::from((i32::from(b0) - 247) * 256 + byte(1)? + 108), 2),
        251..=254 => (f64::from(-(i32::from(b0) - 251) * 256 - byte(1)? - 108), 2),
        // 16.16 fixed. Type 1 charstrings read this as a 32-bit integer instead, which
        // is why the format has to be known before a byte stream can be read at all.
        255 => {
            let bits = (byte(1)? << 24) | (byte(2)? << 16) | (byte(3)? << 8) | byte(4)?;
            (f64::from(bits) / 65536.0, 5)
        }
        _ => return Err(malformed(&format!("byte {b0} is not an operand"))),
    })
}

/// Append `value` to `out` in the Type 2 charstring encoding.
///
/// Whole numbers take the compact integer forms; anything else takes the five-byte 16.16
/// form, which is the only way a charstring can carry a fraction. Resolving a blend
/// routinely produces one — a delta scaled by a region's contribution is not an integer —
/// and rounding it away here would move the outline.
pub fn write_charstring_number(value: f64, out: &mut Vec<u8>) {
    let rounded = value.round();
    if rounded == value && (-32768.0..=32767.0).contains(&value) {
        let v = value as i32;
        match v {
            -107..=107 => out.push((v + 139) as u8),
            108..=1131 => {
                let v = v - 108;
                out.push((v / 256 + 247) as u8);
                out.push((v % 256) as u8);
            }
            -1131..=-108 => {
                let v = -v - 108;
                out.push((v / 256 + 251) as u8);
                out.push((v % 256) as u8);
            }
            _ => {
                out.push(28);
                out.extend_from_slice(&(v as i16).to_be_bytes());
            }
        }
        return;
    }
    // 16.16 saturates rather than wrapping: a coordinate outside ±32768 cannot be
    // represented, and wrapping would put the point on the opposite side of the glyph.
    let bits = (value * 65536.0).round().clamp(-2147483648.0, 2147483647.0) as i32;
    out.push(255);
    out.extend_from_slice(&bits.to_be_bytes());
}

// ---------------------------------------------------------------------- DICT numbers

/// One entry of a DICT: its operands and the operator that closed them.
#[derive(Clone, Debug, PartialEq)]
pub struct DictEntry {
    /// 0..=21 for a one-byte operator; `1200 + b1` for the two-byte `12 b1` form.
    pub operator: u16,
    pub operands: Vec<f64>,
    /// The entry exactly as it appeared, operands and operator together, so an entry
    /// that is passed through can be copied rather than re-encoded. Real numbers in a
    /// DICT are binary-coded decimal, and re-encoding one is a lossy round trip.
    pub raw: Vec<u8>,
}

/// Split DICT data into entries.
pub fn parse_dict(data: &[u8]) -> Result<Vec<DictEntry>, SliceError> {
    let mut out = Vec::new();
    let mut operands: Vec<f64> = Vec::new();
    let mut start = 0usize;
    let mut pos = 0usize;

    while pos < data.len() {
        let b0 = data[pos];
        // Operators run from 0 to 27, not to 21 as CFF 1.0's table suggests: CFF2 put
        // `vsindex` at 22, `blend` at 23 and the Top DICT's `VariationStore` at 24, in
        // the range CFF 1.0 left reserved. Stopping at 21 makes a CFF2 Top DICT
        // unreadable at its third entry.
        if b0 <= 27 {
            let (operator, len) = if b0 == 12 {
                let b1 = *data
                    .get(pos + 1)
                    .ok_or_else(|| malformed("truncated two-byte DICT operator"))?;
                (1200 + u16::from(b1), 2)
            } else {
                (u16::from(b0), 1)
            };
            out.push(DictEntry {
                operator,
                operands: std::mem::take(&mut operands),
                raw: data[start..pos + len].to_vec(),
            });
            pos += len;
            start = pos;
            continue;
        }
        let (value, len) = read_dict_number(data, pos)?;
        operands.push(value);
        pos += len;
    }

    if !operands.is_empty() {
        return Err(malformed("a DICT ended with operands and no operator"));
    }
    Ok(out)
}

/// Read one DICT operand starting at `data[pos]`.
fn read_dict_number(data: &[u8], pos: usize) -> Result<(f64, usize), SliceError> {
    let b0 = data[pos];
    let byte = |i: usize| -> Result<i32, SliceError> {
        data.get(pos + i)
            .map(|b| i32::from(*b))
            .ok_or_else(|| malformed("truncated DICT operand"))
    };
    Ok(match b0 {
        28 => (f64::from(((byte(1)? << 8) | byte(2)?) as i16), 3),
        29 => (
            f64::from((byte(1)? << 24) | (byte(2)? << 16) | (byte(3)? << 8) | byte(4)?),
            5,
        ),
        30 => read_dict_real(data, pos)?,
        32..=246 => (f64::from(i32::from(b0) - 139), 1),
        247..=250 => (f64::from((i32::from(b0) - 247) * 256 + byte(1)? + 108), 2),
        251..=254 => (f64::from(-(i32::from(b0) - 251) * 256 - byte(1)? - 108), 2),
        _ => return Err(malformed(&format!("byte {b0} is not a DICT operand"))),
    })
}

/// Read a binary-coded-decimal real from a DICT.
fn read_dict_real(data: &[u8], pos: usize) -> Result<(f64, usize), SliceError> {
    let mut text = String::new();
    let mut i = pos + 1;
    loop {
        let byte = *data
            .get(i)
            .ok_or_else(|| malformed("truncated DICT real number"))?;
        i += 1;
        for nibble in [byte >> 4, byte & 0x0f] {
            match nibble {
                0..=9 => text.push((b'0' + nibble) as char),
                0xa => text.push('.'),
                0xb => text.push('E'),
                0xc => text.push_str("E-"),
                0xe => text.push('-'),
                0xf => {
                    let value = text
                        .parse::<f64>()
                        .map_err(|_| malformed(&format!("{text:?} is not a number")))?;
                    return Ok((value, i - pos));
                }
                // 0xd is reserved and has no meaning; a font that uses it is malformed.
                _ => return Err(malformed("a DICT real used the reserved nibble 0xd")),
            }
        }
    }
}

/// Append `value` to `out` in the CFF DICT integer encoding, choosing the shortest form.
pub fn write_dict_integer(value: i32, out: &mut Vec<u8>) {
    match value {
        -107..=107 => out.push((value + 139) as u8),
        108..=1131 => {
            let v = value - 108;
            out.push((v / 256 + 247) as u8);
            out.push((v % 256) as u8);
        }
        -1131..=-108 => {
            let v = -value - 108;
            out.push((v / 256 + 251) as u8);
            out.push((v % 256) as u8);
        }
        -32768..=32767 => {
            out.push(28);
            out.extend_from_slice(&(value as i16).to_be_bytes());
        }
        _ => write_dict_offset(value, out),
    }
}

/// Append `value` to `out` in whichever DICT encoding can hold it.
///
/// DICTs have no 16.16 form, so a value that is not a whole number has to go in as a
/// binary-coded-decimal real. Resolving a blended `BlueScale` produces exactly that.
pub fn write_dict_number(value: f64, out: &mut Vec<u8>) {
    if value.fract() == 0.0 && (-2147483648.0..=2147483647.0).contains(&value) {
        write_dict_integer(value as i32, out);
    } else {
        write_dict_real(value, out);
    }
}

/// Append `value` as a binary-coded-decimal real.
pub fn write_dict_real(value: f64, out: &mut Vec<u8>) {
    let text = format!("{value}");
    let mut nibbles: Vec<u8> = Vec::with_capacity(text.len() + 2);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        nibbles.push(match c {
            '0'..='9' => c as u8 - b'0',
            '.' => 0xa,
            '-' => 0xe,
            'e' | 'E' => {
                if chars.peek() == Some(&'-') {
                    chars.next();
                    0xc
                } else {
                    if chars.peek() == Some(&'+') {
                        chars.next();
                    }
                    0xb
                }
            }
            // Rust's float formatting produces nothing else; anything that appears here
            // would be silently mis-encoded, so it is dropped rather than guessed at.
            _ => continue,
        });
    }
    nibbles.push(0xf);
    if nibbles.len() % 2 == 1 {
        nibbles.push(0xf);
    }
    out.push(30);
    for pair in nibbles.chunks(2) {
        out.push((pair[0] << 4) | pair[1]);
    }
}

/// Append `value` in the five-byte DICT integer form, whatever its magnitude.
///
/// Every offset this writer emits uses this rather than the shortest encoding, because
/// a DICT offset points at a position that depends on the size of the DICT holding it.
/// Fixing the width breaks that circle: the layout can be computed once, in one pass,
/// instead of being iterated to a fixed point.
pub fn write_dict_offset(value: i32, out: &mut Vec<u8>) {
    out.push(29);
    out.extend_from_slice(&value.to_be_bytes());
}

/// How many bytes [`write_dict_offset`] adds, including the operator that follows.
pub const DICT_OFFSET_OPERAND_LEN: usize = 5;

// ---------------------------------------------------------------------------- INDEX

/// Serialise `items` as a CFF2 INDEX.
///
/// The CFF2 INDEX differs from CFF 1.0's in its count field, which is 32 bits rather
/// than 16; everything else is the same. An empty index is the bare count.
pub fn write_index(items: &[Vec<u8>]) -> Vec<u8> {
    if items.is_empty() {
        return vec![0, 0, 0, 0];
    }
    let total: usize = items.iter().map(|item| item.len()).sum();
    // Offsets are one-based, so the last one is `total + 1` and that is the value the
    // width has to cover.
    let off_size = offset_size(total + 1);

    let mut out = Vec::with_capacity(4 + 1 + (items.len() + 1) * off_size + total);
    out.extend_from_slice(&(items.len() as u32).to_be_bytes());
    out.push(off_size as u8);

    let mut offset = 1usize;
    write_offset(&mut out, offset, off_size);
    for item in items {
        offset += item.len();
        write_offset(&mut out, offset, off_size);
    }
    for item in items {
        out.extend_from_slice(item);
    }
    out
}

/// The narrowest offset width that can hold `largest`.
pub fn offset_size(largest: usize) -> usize {
    if largest <= 0xff {
        1
    } else if largest <= 0xffff {
        2
    } else if largest <= 0xff_ffff {
        3
    } else {
        4
    }
}

fn write_offset(out: &mut Vec<u8>, value: usize, width: usize) {
    let bytes = (value as u32).to_be_bytes();
    out.extend_from_slice(&bytes[4 - width..]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip_charstring(value: f64) -> f64 {
        let mut bytes = Vec::new();
        write_charstring_number(value, &mut bytes);
        let (read, len) = read_charstring_number(&bytes, 0).unwrap();
        assert_eq!(len, bytes.len(), "{value} left bytes unread");
        read
    }

    #[test]
    fn charstring_integers_round_trip_in_every_form() {
        // One per encoding band, plus the boundaries between them.
        for value in [
            0.0, 1.0, -1.0, 107.0, -107.0, 108.0, -108.0, 1131.0, -1131.0, 1132.0, -1132.0,
            32767.0, -32768.0,
        ] {
            assert_eq!(round_trip_charstring(value), value, "{value}");
        }
    }

    #[test]
    fn charstring_integers_use_the_shortest_encoding() {
        let encode = |v: f64| {
            let mut out = Vec::new();
            write_charstring_number(v, &mut out);
            out
        };
        assert_eq!(encode(0.0), vec![139]);
        assert_eq!(encode(107.0), vec![246]);
        assert_eq!(encode(108.0), vec![247, 0]);
        assert_eq!(encode(-108.0), vec![251, 0]);
        assert_eq!(encode(1131.0), vec![250, 255]);
        assert_eq!(encode(1132.0), vec![28, 0x04, 0x6c]);
    }

    #[test]
    fn fractions_take_the_16_16_form_and_survive_it() {
        let mut bytes = Vec::new();
        write_charstring_number(0.5, &mut bytes);
        assert_eq!(bytes, vec![255, 0x00, 0x00, 0x80, 0x00]);
        assert_eq!(round_trip_charstring(0.5), 0.5);
        assert_eq!(round_trip_charstring(-0.5), -0.5);

        // One ulp of 16.16, the smallest value the format distinguishes from zero.
        let ulp = 1.0 / 65536.0;
        assert_eq!(round_trip_charstring(ulp), ulp);
        assert_eq!(round_trip_charstring(-ulp), -ulp);

        // A fraction that has no exact 16.16 representation comes back within one ulp.
        let third = 1.0 / 3.0;
        assert!((round_trip_charstring(third) - third).abs() <= ulp);
    }

    #[test]
    fn a_fraction_beyond_the_16_16_range_saturates_rather_than_wrapping() {
        // Wrapping would put the coordinate on the far side of the glyph, which is a
        // much worse answer than clamping to the largest value the format has.
        assert!(round_trip_charstring(40000.5) > 32000.0);
        assert!(round_trip_charstring(-40000.5) < -32000.0);
    }

    #[test]
    fn a_truncated_operand_is_an_error_not_a_panic() {
        assert!(read_charstring_number(&[28, 0], 0).is_err());
        assert!(read_charstring_number(&[255, 0, 0], 0).is_err());
        assert!(read_charstring_number(&[247], 0).is_err());
        assert!(read_charstring_number(&[], 0).is_err());
        // 31 is neither an operand nor a value this function should be handed.
        assert!(read_charstring_number(&[31], 0).is_err());
    }

    #[test]
    fn dict_numbers_round_trip() {
        for value in [
            0i32, 1, -1, 107, 108, -108, 1131, -1131, 32767, -32768, 100_000,
        ] {
            let mut bytes = Vec::new();
            write_dict_integer(value, &mut bytes);
            let entries = parse_dict(&[bytes.clone(), vec![17]].concat()).unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].operands, vec![f64::from(value)], "{value}");
        }
    }

    #[test]
    fn the_cff2_only_operators_are_read_as_operators() {
        // The Top DICT of the `cff2-vf` fixture: FDArray 167, CharStrings 49,
        // VariationStore 17. Operator 24 sits in the range CFF 1.0 reserved.
        let data = [0xf7, 0x3b, 0x0c, 0x24, 0xbc, 0x11, 0x9c, 0x18];
        let entries = parse_dict(&data).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!((entries[0].operator, entries[0].operands[0]), (1236, 167.0));
        assert_eq!((entries[1].operator, entries[1].operands[0]), (17, 49.0));
        assert_eq!((entries[2].operator, entries[2].operands[0]), (24, 17.0));
    }

    #[test]
    fn a_dict_real_is_read_as_binary_coded_decimal() {
        // -2.25E-3 written as nibbles: e 2 a 2 5 c 3 f
        let data = [30u8, 0xe2, 0xa2, 0x5c, 0x3f, 12, 7];
        let entries = parse_dict(&data).unwrap();
        assert_eq!(entries[0].operator, 1207);
        assert!((entries[0].operands[0] - -2.25e-3).abs() < 1e-12);
    }

    #[test]
    fn a_dict_entry_keeps_its_own_bytes() {
        // The raw form is what lets an entry be copied through without being re-encoded.
        let data = [30u8, 0xe2, 0xa2, 0x5c, 0x3f, 12, 7, 0x8b, 17];
        let entries = parse_dict(&data).unwrap();
        assert_eq!(entries[0].raw, &data[0..7]);
        assert_eq!(entries[1].raw, &data[7..9]);
    }

    #[test]
    fn a_malformed_dict_is_an_error_not_a_panic() {
        // Operands with no operator to close them.
        assert!(parse_dict(&[0x8b]).is_err());
        // A two-byte operator cut in half.
        assert!(parse_dict(&[0x8b, 12]).is_err());
        // A real number with no terminator.
        assert!(parse_dict(&[30, 0x12]).is_err());
        // The reserved nibble.
        assert!(parse_dict(&[30, 0xd0]).is_err());
        // A reserved operand byte.
        assert!(parse_dict(&[31, 17]).is_err());
    }

    #[test]
    fn a_fractional_dict_value_becomes_a_binary_coded_decimal_real() {
        for value in [0.039625, -2.25, 1.5, -0.5] {
            let mut bytes = Vec::new();
            write_dict_number(value, &mut bytes);
            assert_eq!(bytes[0], 30, "{value} should use the real encoding");
            let entries = parse_dict(&[bytes, vec![12, 9]].concat()).unwrap();
            assert_eq!(entries[0].operands[0], value, "{value}");
        }
        // Whole numbers keep the integer encoding, which is shorter and exact.
        let mut bytes = Vec::new();
        write_dict_number(400.0, &mut bytes);
        assert_ne!(bytes[0], 30);
    }

    #[test]
    fn an_empty_index_is_a_bare_count() {
        assert_eq!(write_index(&[]), vec![0, 0, 0, 0]);
    }

    #[test]
    fn index_offsets_widen_with_the_data() {
        let one_byte = write_index(&[vec![0u8; 10], vec![1u8; 10]]);
        assert_eq!(one_byte[4], 1);
        assert_eq!(&one_byte[5..8], &[1, 11, 21]);

        let two_byte = write_index(&[vec![0u8; 300]]);
        assert_eq!(two_byte[4], 2);
        assert_eq!(&two_byte[5..9], &[0, 1, 1, 45]);

        let three_byte = write_index(&[vec![0u8; 0x10000]]);
        assert_eq!(three_byte[4], 3);

        let four_byte = write_index(&[vec![0u8; 0x100_0000]]);
        assert_eq!(four_byte[4], 4);
    }

    #[test]
    fn an_index_reads_back_through_read_fonts() {
        // The point of this one is that the offsets are one-based and the data begins
        // where the last offset says: an off-by-one here is invisible in isolation and
        // shifts every charstring in the font.
        let items = vec![vec![1u8, 2, 3], vec![], vec![4u8], vec![5u8; 400]];
        let bytes = write_index(&items);
        let index = read_fonts::ps::cff::index::Index::new(&bytes, true).unwrap();
        assert_eq!(index.count(), 4);
        for (i, item) in items.iter().enumerate() {
            assert_eq!(index.get(i).unwrap(), item.as_slice(), "item {i}");
        }
        assert_eq!(index.size_in_bytes().unwrap(), bytes.len());
    }
}
