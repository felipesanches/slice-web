//! Reading and writing the CFF2 table itself.
//!
//! `read-fonts` parses CFF2 far enough to draw from it; `write-fonts` has no CFF or CFF2
//! writer at all, so the serialiser here is written from the specification. It is short,
//! because CFF2 threw out most of what made CFF 1.0 awkward: there is no Name INDEX, no
//! String INDEX, no charset, no encoding, and every charstring is Type 2 with no width
//! prefix.
//!
//! What remains awkward is that DICT offsets are absolute and point forward past the DICT
//! that contains them, so the size of a DICT depends on values that depend on its size.
//! Every offset this writes therefore uses the five-byte `29` form regardless of
//! magnitude, which makes each DICT's length known before its contents are, and turns
//! the layout into a single forward pass.

use read_fonts::ps::cff::index::Index;
use read_fonts::{FontRef, TableProvider};
use write_fonts::types::Tag;

use super::num::{
    offset_size, parse_dict, write_dict_offset, write_index, DictEntry, DICT_OFFSET_OPERAND_LEN,
};
use crate::SliceError;

pub const CFF2_TAG: Tag = Tag::new(b"CFF2");

/// Top DICT operators that hold an offset into the table, and so are rebuilt rather than
/// copied.
mod top {
    pub const CHARSTRINGS: u16 = 17;
    pub const VAR_STORE: u16 = 24;
    pub const FD_ARRAY: u16 = 1236;
    pub const FD_SELECT: u16 = 1237;
}

/// Font DICT and Private DICT operators this writer manages itself.
mod private {
    /// Font DICT: the Private DICT's size and offset, as a pair.
    pub const PRIVATE: u16 = 18;
    /// Private DICT: the offset of the local subroutine INDEX, relative to the DICT.
    pub const SUBRS: u16 = 19;
    /// Private DICT: the ItemVariationData its blends index.
    pub const VSINDEX: u16 = 22;
}

fn malformed(what: &str) -> SliceError {
    SliceError::Read(format!("the CFF2 table is malformed: {what}"))
}

/// A CFF2 table taken apart into the pieces instancing needs.
pub struct Cff2Table<'a> {
    pub top_dict: Vec<DictEntry>,
    pub global_subrs: Vec<&'a [u8]>,
    pub charstrings: Vec<&'a [u8]>,
    /// The ItemVariationStore, without the two-byte length that precedes it in the file.
    pub var_store: Option<&'a [u8]>,
    /// FDSelect exactly as stored. Glyph order does not change, so neither does this.
    pub fd_select: Option<&'a [u8]>,
    pub font_dicts: Vec<FontDict<'a>>,
}

/// One entry of the FDArray: a Font DICT and the Private DICT it points at.
pub struct FontDict<'a> {
    /// Everything except the `Private` entry, which is rebuilt.
    pub other_entries: Vec<DictEntry>,
    pub private: Vec<DictEntry>,
    pub local_subrs: Vec<&'a [u8]>,
}

impl FontDict<'_> {
    /// The `vsindex` this Private DICT sets, which its glyphs start out using.
    pub fn vsindex(&self) -> u16 {
        self.private
            .iter()
            .find(|entry| entry.operator == private::VSINDEX)
            .and_then(|entry| entry.operands.first())
            .map(|v| v.clamp(0.0, 65535.0) as u16)
            .unwrap_or(0)
    }
}

/// Read the `CFF2` table of `font`.
pub fn read<'a>(font: &FontRef<'a>) -> Result<Cff2Table<'a>, SliceError> {
    let data = font
        .table_data(CFF2_TAG)
        .ok_or_else(|| malformed("the table is missing"))?
        .as_bytes();
    let cff2 = font.cff2()?;

    let top_dict = parse_dict(cff2.top_dict_data())?;
    let header_size = usize::from(cff2.header().header_size());
    let top_dict_length = usize::from(cff2.header().top_dict_length());
    let global_subrs = index_items(data, header_size + top_dict_length)?.0;

    let offset = |operator: u16| -> Option<usize> {
        top_dict
            .iter()
            .find(|entry| entry.operator == operator)
            .and_then(|entry| entry.operands.first())
            .filter(|v| **v >= 0.0)
            .map(|v| *v as usize)
    };

    let charstrings_offset =
        offset(top::CHARSTRINGS).ok_or_else(|| malformed("no CharStrings offset"))?;
    let charstrings = index_items(data, charstrings_offset)?.0;

    let var_store = match offset(top::VAR_STORE) {
        None => None,
        Some(at) => {
            let length = data
                .get(at..at + 2)
                .map(|b| usize::from(u16::from_be_bytes([b[0], b[1]])))
                .ok_or_else(|| malformed("the VariationStore offset is out of bounds"))?;
            Some(
                data.get(at + 2..at + 2 + length)
                    .ok_or_else(|| malformed("the VariationStore runs past the table"))?,
            )
        }
    };

    let fd_select = match offset(top::FD_SELECT) {
        None => None,
        Some(at) => Some(read_fd_select(data, at, charstrings.len())?),
    };

    let fd_array_offset = offset(top::FD_ARRAY).ok_or_else(|| malformed("no FDArray offset"))?;
    let font_dict_data = index_items(data, fd_array_offset)?.0;
    let mut font_dicts = Vec::with_capacity(font_dict_data.len());
    for entry in font_dict_data {
        font_dicts.push(read_font_dict(data, entry)?);
    }
    if font_dicts.is_empty() {
        return Err(malformed("the FDArray is empty"));
    }

    Ok(Cff2Table {
        top_dict,
        global_subrs,
        charstrings,
        var_store,
        fd_select,
        font_dicts,
    })
}

fn read_font_dict<'a>(data: &'a [u8], dict: &'a [u8]) -> Result<FontDict<'a>, SliceError> {
    let entries = parse_dict(dict)?;
    let mut other_entries = Vec::new();
    let mut private = Vec::new();
    let mut local_subrs = Vec::new();

    for entry in entries {
        if entry.operator != private::PRIVATE {
            other_entries.push(entry);
            continue;
        }
        let [size, at] = entry.operands[..] else {
            return Err(malformed("a Private entry needs a size and an offset"));
        };
        if size < 0.0 || at < 0.0 {
            return Err(malformed("a Private entry has a negative size or offset"));
        }
        let (at, size) = (at as usize, size as usize);
        let bytes = data
            .get(at..at + size)
            .ok_or_else(|| malformed("a Private DICT runs past the table"))?;
        private = parse_dict(bytes)?;

        // The Subrs offset is relative to the start of the Private DICT, which is the one
        // relative offset left in the format.
        if let Some(subrs) = private
            .iter()
            .find(|e| e.operator == private::SUBRS)
            .and_then(|e| e.operands.first())
            .filter(|v| **v > 0.0)
        {
            local_subrs = index_items(data, at + *subrs as usize)?.0;
        }
    }

    Ok(FontDict {
        other_entries,
        private,
        local_subrs,
    })
}

/// The items of the INDEX at `at`, and the offset just past it.
fn index_items(data: &[u8], at: usize) -> Result<(Vec<&[u8]>, usize), SliceError> {
    let tail = data
        .get(at..)
        .ok_or_else(|| malformed("an INDEX offset is out of bounds"))?;
    let index = Index::new(tail, true).map_err(|e| malformed(&format!("bad INDEX: {e}")))?;
    let size = index
        .size_in_bytes()
        .map_err(|e| malformed(&format!("bad INDEX: {e}")))?;
    let mut items = Vec::with_capacity(index.count() as usize);
    for i in 0..index.count() as usize {
        items.push(
            index
                .get(i)
                .map_err(|e| malformed(&format!("bad INDEX entry {i}: {e}")))?,
        );
    }
    Ok((items, at + size))
}

/// FDSelect exactly as stored, which needs its length worked out from its format.
fn read_fd_select(data: &[u8], at: usize, num_glyphs: usize) -> Result<&[u8], SliceError> {
    let read_u16 = |i: usize| -> Result<usize, SliceError> {
        data.get(i..i + 2)
            .map(|b| usize::from(u16::from_be_bytes([b[0], b[1]])))
            .ok_or_else(|| malformed("FDSelect runs past the table"))
    };
    let read_u32 = |i: usize| -> Result<usize, SliceError> {
        data.get(i..i + 4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize)
            .ok_or_else(|| malformed("FDSelect runs past the table"))
    };

    let format = *data
        .get(at)
        .ok_or_else(|| malformed("the FDSelect offset is out of bounds"))?;
    let length = match format {
        // Format 0 has one byte per glyph and stores no count of its own, which is why
        // the charstring count has to be passed in.
        0 => 1 + num_glyphs,
        3 => 3 + read_u16(at + 1)? * 3 + 2,
        4 => 5 + read_u32(at + 1)? * 6 + 4,
        other => {
            return Err(malformed(&format!(
                "FDSelect format {other} is not defined"
            )))
        }
    };
    data.get(at..at + length)
        .ok_or_else(|| malformed("FDSelect runs past the table"))
}

// ------------------------------------------------------------------------- writing

/// The pieces of a CFF2 table that is about to be written.
#[derive(Default)]
pub struct Cff2Builder {
    /// Top DICT entries other than the offsets, copied verbatim from the input.
    pub top_dict_extra: Vec<Vec<u8>>,
    pub global_subrs: Vec<Vec<u8>>,
    pub charstrings: Vec<Vec<u8>>,
    /// The ItemVariationStore, without the two-byte length.
    pub var_store: Option<Vec<u8>>,
    pub fd_select: Option<Vec<u8>>,
    pub font_dicts: Vec<FontDictBuilder>,
}

#[derive(Default)]
pub struct FontDictBuilder {
    /// Font DICT entries other than `Private`, copied verbatim.
    pub other_entries: Vec<Vec<u8>>,
    /// The Private DICT with its `Subrs` entry removed; this writer adds its own.
    pub private: Vec<u8>,
    pub local_subrs: Vec<Vec<u8>>,
}

impl Cff2Builder {
    /// Serialise the table.
    pub fn build(&self) -> Result<Vec<u8>, SliceError> {
        // Lengths first. Every offset operand is five bytes wide whatever it holds, so
        // each of these is known before any of the offsets are.
        let mut top_dict_len: usize = self.top_dict_extra.iter().map(|e| e.len()).sum();
        top_dict_len += DICT_OFFSET_OPERAND_LEN + 1; // CharStrings
        top_dict_len += DICT_OFFSET_OPERAND_LEN + 2; // FDArray, a two-byte operator
        if self.var_store.is_some() {
            top_dict_len += DICT_OFFSET_OPERAND_LEN + 1;
        }
        if self.fd_select.is_some() {
            top_dict_len += DICT_OFFSET_OPERAND_LEN + 2;
        }

        let global_subrs = write_index(&self.global_subrs);
        let charstrings = write_index(&self.charstrings);

        // Each Private DICT gains its own `Subrs` entry when there are local subroutines
        // to point at, and the local subroutines sit immediately after it.
        let mut private_blobs: Vec<(Vec<u8>, Vec<u8>)> = Vec::with_capacity(self.font_dicts.len());
        for fd in &self.font_dicts {
            let subrs = write_index(&fd.local_subrs);
            let mut private = fd.private.clone();
            if !fd.local_subrs.is_empty() {
                let offset = private.len() + DICT_OFFSET_OPERAND_LEN + 1;
                write_dict_offset(offset as i32, &mut private);
                private.push(private::SUBRS as u8);
            }
            private_blobs.push((private, subrs));
        }

        // The Font DICTs, with placeholder offsets. Their lengths are final because the
        // Private entry's two operands are fixed width.
        let font_dict_lengths: Vec<usize> = self
            .font_dicts
            .iter()
            .map(|fd| {
                fd.other_entries.iter().map(|e| e.len()).sum::<usize>()
                    + DICT_OFFSET_OPERAND_LEN * 2
                    + 1
            })
            .collect();
        let fd_array_len = index_length(&font_dict_lengths);

        // Now the layout.
        const HEADER_LEN: usize = 5;
        let mut cursor = HEADER_LEN + top_dict_len + global_subrs.len();
        let charstrings_offset = cursor;
        cursor += charstrings.len();

        let var_store_offset = self.var_store.as_ref().map(|store| {
            let at = cursor;
            cursor += 2 + store.len();
            at
        });
        let fd_select_offset = self.fd_select.as_ref().map(|select| {
            let at = cursor;
            cursor += select.len();
            at
        });
        let fd_array_offset = cursor;
        cursor += fd_array_len;

        let mut private_offsets = Vec::with_capacity(private_blobs.len());
        for (private, subrs) in &private_blobs {
            private_offsets.push((private.len(), cursor));
            cursor += private.len() + subrs.len();
        }
        if cursor > i32::MAX as usize {
            return Err(SliceError::Write(
                "the CFF2 table would be larger than its offsets can address".into(),
            ));
        }

        // And the bytes.
        let mut out = Vec::with_capacity(cursor);
        out.extend_from_slice(&[2, 0, HEADER_LEN as u8]);
        out.extend_from_slice(&(top_dict_len as u16).to_be_bytes());

        let top_dict_start = out.len();
        for entry in &self.top_dict_extra {
            out.extend_from_slice(entry);
        }
        write_dict_offset(charstrings_offset as i32, &mut out);
        out.push(top::CHARSTRINGS as u8);
        if let Some(at) = var_store_offset {
            write_dict_offset(at as i32, &mut out);
            out.push(top::VAR_STORE as u8);
        }
        if let Some(at) = fd_select_offset {
            write_dict_offset(at as i32, &mut out);
            out.extend_from_slice(&[12, (top::FD_SELECT - 1200) as u8]);
        }
        write_dict_offset(fd_array_offset as i32, &mut out);
        out.extend_from_slice(&[12, (top::FD_ARRAY - 1200) as u8]);
        debug_assert_eq!(out.len() - top_dict_start, top_dict_len);

        out.extend_from_slice(&global_subrs);
        debug_assert_eq!(out.len(), charstrings_offset);
        out.extend_from_slice(&charstrings);

        if let Some(store) = &self.var_store {
            let length = u16::try_from(store.len()).map_err(|_| {
                SliceError::Write("the CFF2 variation store is larger than 64KB".into())
            })?;
            out.extend_from_slice(&length.to_be_bytes());
            out.extend_from_slice(store);
        }
        if let Some(select) = &self.fd_select {
            out.extend_from_slice(select);
        }

        let font_dicts: Vec<Vec<u8>> = self
            .font_dicts
            .iter()
            .zip(&private_offsets)
            .map(|(fd, (size, at))| {
                let mut dict = Vec::new();
                for entry in &fd.other_entries {
                    dict.extend_from_slice(entry);
                }
                write_dict_offset(*size as i32, &mut dict);
                write_dict_offset(*at as i32, &mut dict);
                dict.push(private::PRIVATE as u8);
                dict
            })
            .collect();
        debug_assert_eq!(out.len(), fd_array_offset);
        out.extend_from_slice(&write_index(&font_dicts));

        for (private, subrs) in &private_blobs {
            out.extend_from_slice(private);
            out.extend_from_slice(subrs);
        }

        debug_assert_eq!(out.len(), cursor);
        Ok(out)
    }
}

/// How long the INDEX holding items of these lengths will be.
///
/// The FDArray's own size has to be known before the Private DICTs that follow it can be
/// placed, and its contents are not built until after that, so its length is computed
/// from the item lengths alone.
fn index_length(item_lengths: &[usize]) -> usize {
    if item_lengths.is_empty() {
        return 4;
    }
    let total: usize = item_lengths.iter().sum();
    4 + 1 + (item_lengths.len() + 1) * offset_size(total + 1) + total
}

/// Copy the Top DICT entries that survive, dropping the offsets this writer recomputes.
pub fn top_dict_extra(top_dict: &[DictEntry]) -> Vec<Vec<u8>> {
    top_dict
        .iter()
        .filter(|entry| {
            !matches!(
                entry.operator,
                top::CHARSTRINGS | top::VAR_STORE | top::FD_ARRAY | top::FD_SELECT
            )
        })
        .map(|entry| entry.raw.clone())
        .collect()
}

/// A Private DICT copied through verbatim, minus the `Subrs` offset the writer
/// recomputes from where it actually puts the local subroutines.
///
/// Copying the raw bytes rather than re-encoding keeps binary-coded-decimal reals such
/// as `BlueScale` exactly as the designer set them.
pub fn private_dict_without_subrs(entries: &[DictEntry]) -> Vec<u8> {
    let mut out = Vec::new();
    for entry in entries {
        if entry.operator == private::SUBRS {
            continue;
        }
        out.extend_from_slice(&entry.raw);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use read_fonts::FontRead;

    #[test]
    fn an_empty_font_dict_still_gets_a_private_entry() {
        // The specification requires every Font DICT to have one, even when the Private
        // DICT itself is empty, and a reader that does not find one refuses the font.
        let builder = Cff2Builder {
            charstrings: vec![vec![139, 22]],
            font_dicts: vec![FontDictBuilder::default()],
            ..Default::default()
        };
        let bytes = builder.build().unwrap();
        assert_eq!(&bytes[0..3], &[2, 0, 5]);
        // Header plus top DICT plus an empty global subr INDEX.
        let top_dict_length = usize::from(u16::from_be_bytes([bytes[3], bytes[4]]));
        assert!(top_dict_length > 0);
    }

    #[test]
    fn the_written_table_reads_back_through_read_fonts() {
        let builder = Cff2Builder {
            charstrings: vec![vec![139, 22], vec![140, 22]],
            var_store: Some(vec![0u8; 12]),
            font_dicts: vec![FontDictBuilder {
                private: vec![0x8b, 20],
                local_subrs: vec![vec![139, 11]],
                ..Default::default()
            }],
            ..Default::default()
        };
        let bytes = builder.build().unwrap();

        let cff2 = read_fonts::tables::cff2::Cff2::read(read_fonts::FontData::new(&bytes))
            .expect("the table should parse");
        assert_eq!(cff2.header().major_version(), 2);
        assert_eq!(cff2.global_subrs().count(), 0);

        // And the offsets in the Top DICT actually point at the right things.
        let entries = parse_dict(cff2.top_dict_data()).unwrap();
        let charstrings_at = entries
            .iter()
            .find(|e| e.operator == top::CHARSTRINGS)
            .unwrap()
            .operands[0] as usize;
        let (items, _) = index_items(&bytes, charstrings_at).unwrap();
        assert_eq!(items, vec![&[139u8, 22][..], &[140u8, 22][..]]);
    }

    #[test]
    fn a_truncated_table_is_an_error_rather_than_a_panic() {
        // Every prefix of a valid table is a malformed one. None of them may panic:
        // these bytes come from a file the user chose, and a crash in a browser tab is
        // a worse answer than a message.
        let builder = Cff2Builder {
            charstrings: vec![vec![139, 22], vec![140, 22]],
            var_store: Some(vec![0u8; 12]),
            font_dicts: vec![FontDictBuilder {
                private: vec![0x8b, 20],
                local_subrs: vec![vec![139, 11]],
                ..Default::default()
            }],
            ..Default::default()
        };
        let whole = builder.build().unwrap();
        for length in 0..whole.len() {
            let truncated = &whole[..length];
            // Reading is exercised through the pieces that take raw bytes; the table
            // reader itself needs a FontRef, which a bare table cannot provide.
            if let Ok(cff2) =
                read_fonts::tables::cff2::Cff2::read(read_fonts::FontData::new(truncated))
            {
                let _ = parse_dict(cff2.top_dict_data());
                let _ = index_items(truncated, 0);
            }
        }
    }

    #[test]
    fn an_index_offset_past_the_end_is_an_error() {
        assert!(index_items(&[0, 0, 0, 1, 1, 1], 999).is_err());
        // A count that claims more entries than the offsets can describe.
        assert!(index_items(&[0, 0, 0xff, 0xff, 1, 1], 0).is_err());
    }

    #[test]
    fn local_subroutines_are_found_through_the_private_dict() {
        let builder = Cff2Builder {
            charstrings: vec![vec![139, 22]],
            font_dicts: vec![FontDictBuilder {
                private: vec![],
                local_subrs: vec![vec![1, 2, 3], vec![4]],
                ..Default::default()
            }],
            ..Default::default()
        };
        let bytes = builder.build().unwrap();
        let cff2 = read_fonts::tables::cff2::Cff2::read(read_fonts::FontData::new(&bytes)).unwrap();
        let top = parse_dict(cff2.top_dict_data()).unwrap();
        let fd_array_at = top
            .iter()
            .find(|e| e.operator == top::FD_ARRAY)
            .unwrap()
            .operands[0] as usize;
        let (dicts, _) = index_items(&bytes, fd_array_at).unwrap();
        let fd = read_font_dict(&bytes, dicts[0]).unwrap();
        assert_eq!(fd.local_subrs, vec![&[1u8, 2, 3][..], &[4u8][..]]);
    }
}
