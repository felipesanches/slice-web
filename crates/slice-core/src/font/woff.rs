//! WOFF 1.0 container handling.
//!
//! WOFF is a thin wrapper: an sfnt whose tables have each been zlib-compressed
//! individually. Unwrapping and rewrapping it is small enough to do here, which keeps a
//! C dependency out of the WebAssembly build.

use miniz_oxide::deflate::compress_to_vec_zlib;
use miniz_oxide::inflate::decompress_to_vec_zlib_with_limit;

use crate::SliceError;

const WOFF_HEADER_LEN: usize = 44;
const WOFF_ENTRY_LEN: usize = 20;
const SFNT_HEADER_LEN: usize = 12;
const SFNT_RECORD_LEN: usize = 16;

/// Largest table we are willing to inflate, as a guard against a malicious `origLength`.
const MAX_TABLE_LEN: usize = 256 * 1024 * 1024;

struct Entry {
    tag: [u8; 4],
    offset: usize,
    comp_length: usize,
    orig_length: usize,
    orig_checksum: u32,
}

/// Turn WOFF bytes into the sfnt they wrap.
pub fn decode_woff(data: &[u8]) -> Result<Vec<u8>, SliceError> {
    if data.len() < WOFF_HEADER_LEN || &data[..4] != b"wOFF" {
        return Err(SliceError::Read("not a WOFF file".into()));
    }
    let flavor = be_u32(data, 4)?;
    let num_tables = be_u16(data, 12)? as usize;
    if num_tables == 0 {
        return Err(SliceError::Read("WOFF file declares no tables".into()));
    }

    let mut entries = Vec::with_capacity(num_tables);
    for i in 0..num_tables {
        let base = WOFF_HEADER_LEN + i * WOFF_ENTRY_LEN;
        if base + WOFF_ENTRY_LEN > data.len() {
            return Err(SliceError::Read("truncated WOFF table directory".into()));
        }
        let mut tag = [0u8; 4];
        tag.copy_from_slice(&data[base..base + 4]);
        entries.push(Entry {
            tag,
            offset: be_u32(data, base + 4)? as usize,
            comp_length: be_u32(data, base + 8)? as usize,
            orig_length: be_u32(data, base + 12)? as usize,
            orig_checksum: be_u32(data, base + 16)?,
        });
    }

    // The sfnt table directory has to be sorted by tag; WOFF's need not be.
    entries.sort_by_key(|e| e.tag);

    let mut tables: Vec<(Entry, Vec<u8>)> = Vec::with_capacity(num_tables);
    for entry in entries {
        let end = entry
            .offset
            .checked_add(entry.comp_length)
            .ok_or_else(|| SliceError::Read("WOFF table extends past end of file".into()))?;
        if end > data.len() {
            return Err(SliceError::Read(format!(
                "WOFF table {} extends past end of file",
                tag_name(&entry.tag)
            )));
        }
        if entry.orig_length > MAX_TABLE_LEN {
            return Err(SliceError::Read(format!(
                "WOFF table {} declares an implausible length",
                tag_name(&entry.tag)
            )));
        }
        let raw = &data[entry.offset..end];

        // Per the specification, a table is stored uncompressed when compressing it
        // would not have made it smaller, and that is signalled by the two lengths
        // being equal.
        let bytes = if entry.comp_length == entry.orig_length {
            raw.to_vec()
        } else {
            decompress_to_vec_zlib_with_limit(raw, entry.orig_length).map_err(|e| {
                SliceError::Read(format!(
                    "WOFF table {} failed to decompress: {e:?}",
                    tag_name(&entry.tag)
                ))
            })?
        };
        if bytes.len() != entry.orig_length {
            return Err(SliceError::Read(format!(
                "WOFF table {} decompressed to {} bytes, expected {}",
                tag_name(&entry.tag),
                bytes.len(),
                entry.orig_length
            )));
        }
        tables.push((entry, bytes));
    }

    Ok(assemble_sfnt(
        flavor,
        tables
            .iter()
            .map(|(e, b)| (e.tag, e.orig_checksum, b.as_slice())),
    ))
}

/// Wrap an sfnt in a WOFF 1.0 container.
pub fn encode_woff(sfnt: &[u8]) -> Result<Vec<u8>, SliceError> {
    let (flavor, tables) = split_sfnt(sfnt)?;
    let num_tables = tables.len();

    let mut directory = Vec::with_capacity(num_tables * WOFF_ENTRY_LEN);
    let mut body: Vec<u8> = Vec::new();
    let mut total_sfnt_size = SFNT_HEADER_LEN + num_tables * SFNT_RECORD_LEN;

    let body_start = WOFF_HEADER_LEN + num_tables * WOFF_ENTRY_LEN;

    for (tag, checksum, bytes) in &tables {
        // Level 8 rather than the maximum: WOFF is generally an intermediate artefact and
        // the last two levels cost noticeably more time for very little size.
        let compressed = compress_to_vec_zlib(bytes, 8);
        let stored: &[u8] = if compressed.len() < bytes.len() {
            &compressed
        } else {
            bytes
        };

        while body.len() % 4 != 0 {
            body.push(0);
        }
        let offset = body_start + body.len();

        directory.extend_from_slice(tag);
        directory.extend_from_slice(&(offset as u32).to_be_bytes());
        directory.extend_from_slice(&(stored.len() as u32).to_be_bytes());
        directory.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        directory.extend_from_slice(&checksum.to_be_bytes());

        body.extend_from_slice(stored);
        total_sfnt_size += (bytes.len() + 3) & !3;
    }

    let total_len = body_start + body.len();
    let mut out = Vec::with_capacity(total_len);
    out.extend_from_slice(b"wOFF");
    out.extend_from_slice(&flavor.to_be_bytes());
    out.extend_from_slice(&(total_len as u32).to_be_bytes());
    out.extend_from_slice(&(num_tables as u16).to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes()); // reserved
    out.extend_from_slice(&(total_sfnt_size as u32).to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes()); // majorVersion
    out.extend_from_slice(&0u16.to_be_bytes()); // minorVersion
    out.extend_from_slice(&0u32.to_be_bytes()); // metaOffset
    out.extend_from_slice(&0u32.to_be_bytes()); // metaLength
    out.extend_from_slice(&0u32.to_be_bytes()); // metaOrigLength
    out.extend_from_slice(&0u32.to_be_bytes()); // privOffset
    out.extend_from_slice(&0u32.to_be_bytes()); // privLength
    debug_assert_eq!(out.len(), WOFF_HEADER_LEN);
    out.extend_from_slice(&directory);
    out.extend_from_slice(&body);
    Ok(out)
}

/// Build an sfnt from a set of tables.
pub fn assemble_sfnt<'a>(
    flavor: u32,
    tables: impl Iterator<Item = ([u8; 4], u32, &'a [u8])>,
) -> Vec<u8> {
    let tables: Vec<_> = tables.collect();
    let num_tables = tables.len();
    let (search_range, entry_selector, range_shift) = search_params(num_tables as u16);

    let mut out = Vec::new();
    out.extend_from_slice(&flavor.to_be_bytes());
    out.extend_from_slice(&(num_tables as u16).to_be_bytes());
    out.extend_from_slice(&search_range.to_be_bytes());
    out.extend_from_slice(&entry_selector.to_be_bytes());
    out.extend_from_slice(&range_shift.to_be_bytes());

    let mut offset = SFNT_HEADER_LEN + num_tables * SFNT_RECORD_LEN;
    for (tag, checksum, bytes) in &tables {
        out.extend_from_slice(tag);
        out.extend_from_slice(&checksum.to_be_bytes());
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        offset += (bytes.len() + 3) & !3;
    }
    for (_, _, bytes) in &tables {
        out.extend_from_slice(bytes);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    out
}

/// Take an sfnt apart into `(tag, checksum, bytes)` triples, sorted by tag.
pub fn split_sfnt(data: &[u8]) -> Result<(u32, Vec<([u8; 4], u32, &[u8])>), SliceError> {
    if data.len() < SFNT_HEADER_LEN {
        return Err(SliceError::Read("file is too short to be a font".into()));
    }
    let flavor = be_u32(data, 0)?;
    let num_tables = be_u16(data, 4)? as usize;
    let mut tables = Vec::with_capacity(num_tables);
    for i in 0..num_tables {
        let base = SFNT_HEADER_LEN + i * SFNT_RECORD_LEN;
        if base + SFNT_RECORD_LEN > data.len() {
            return Err(SliceError::Read("truncated sfnt table directory".into()));
        }
        let mut tag = [0u8; 4];
        tag.copy_from_slice(&data[base..base + 4]);
        let checksum = be_u32(data, base + 4)?;
        let offset = be_u32(data, base + 8)? as usize;
        let length = be_u32(data, base + 12)? as usize;
        let end = offset
            .checked_add(length)
            .ok_or_else(|| SliceError::Read("sfnt table extends past end of file".into()))?;
        if end > data.len() {
            return Err(SliceError::Read(format!(
                "sfnt table {} extends past end of file",
                tag_name(&tag)
            )));
        }
        tables.push((tag, checksum, &data[offset..end]));
    }
    tables.sort_by_key(|(tag, _, _)| *tag);
    Ok((flavor, tables))
}

/// `searchRange`, `entrySelector` and `rangeShift` for an sfnt header.
fn search_params(num_tables: u16) -> (u16, u16, u16) {
    if num_tables == 0 {
        return (0, 0, 0);
    }
    let entry_selector = (15 - num_tables.leading_zeros()) as u16;
    let search_range = (1u16 << entry_selector) * 16;
    let range_shift = num_tables * 16 - search_range;
    (search_range, entry_selector, range_shift)
}

fn be_u16(data: &[u8], at: usize) -> Result<u16, SliceError> {
    data.get(at..at + 2)
        .map(|b| u16::from_be_bytes([b[0], b[1]]))
        .ok_or_else(|| SliceError::Read("unexpected end of file".into()))
}

fn be_u32(data: &[u8], at: usize) -> Result<u32, SliceError> {
    data.get(at..at + 4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| SliceError::Read("unexpected end of file".into()))
}

fn tag_name(tag: &[u8; 4]) -> String {
    String::from_utf8_lossy(tag).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_params_follow_the_spec() {
        // The worked example in the OpenType spec: 9 tables.
        assert_eq!(search_params(9), (128, 3, 16));
        assert_eq!(search_params(1), (16, 0, 0));
        assert_eq!(search_params(16), (256, 4, 0));
    }

    #[test]
    fn woff_input_decodes_to_a_parseable_sfnt() {
        let woff = crate::testdata::recursive_vf_woff();
        let sfnt = decode_woff(woff).unwrap();
        assert_eq!(&sfnt[..4], &[0x00, 0x01, 0x00, 0x00]);
        read_fonts::FontRef::new(&sfnt).expect("decoded WOFF should parse as a font");
    }

    #[test]
    fn decoded_woff_has_the_same_tables_as_the_ttf() {
        let from_woff = decode_woff(crate::testdata::recursive_vf_woff()).unwrap();
        let ttf = crate::testdata::recursive_vf();

        let (_, a) = split_sfnt(&from_woff).unwrap();
        let (_, b) = split_sfnt(ttf).unwrap();
        let a_tags: Vec<_> = a.iter().map(|(t, _, _)| tag_name(t)).collect();
        let b_tags: Vec<_> = b.iter().map(|(t, _, _)| tag_name(t)).collect();
        assert_eq!(a_tags, b_tags);

        // The two fixtures are independent fontTools outputs rather than one repack of
        // the other, so `head` carries a different `modified` timestamp (offset 28..36)
        // and, following from that, a different `checkSumAdjustment` (offset 8..12).
        // Every other byte of every table has to agree.
        const HEAD_VOLATILE: [std::ops::Range<usize>; 2] = [8..12, 28..36];

        for ((_, _, ab), (tag, _, bb)) in a.iter().zip(b.iter()) {
            if tag == b"head" {
                assert_eq!(ab.len(), bb.len(), "head length differs");
                let mut ab = ab.to_vec();
                let mut bb = bb.to_vec();
                for range in HEAD_VOLATILE {
                    ab[range.clone()].fill(0);
                    bb[range].fill(0);
                }
                assert_eq!(ab, bb, "head differs outside its volatile fields");
            } else {
                assert_eq!(ab, bb, "table {} differs", tag_name(tag));
            }
        }
    }

    #[test]
    fn woff_round_trip_preserves_table_contents() {
        let ttf = crate::testdata::recursive_vf();
        let woff = encode_woff(ttf).unwrap();
        assert_eq!(&woff[..4], b"wOFF");
        let back = decode_woff(&woff).unwrap();

        let (_, orig) = split_sfnt(ttf).unwrap();
        let (_, round) = split_sfnt(&back).unwrap();
        assert_eq!(orig.len(), round.len());
        for ((tag, _, a), (_, _, b)) in orig.iter().zip(round.iter()) {
            assert_eq!(a, b, "table {} changed", tag_name(tag));
        }
    }

    #[test]
    fn truncated_woff_is_rejected_rather_than_panicking() {
        let woff = crate::testdata::recursive_vf_woff();
        for cut in [4, 20, 44, 60, woff.len() / 2] {
            assert!(
                decode_woff(&woff[..cut]).is_err(),
                "truncation at {cut} should be an error"
            );
        }
    }
}
