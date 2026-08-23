//! WOFF2 container handling.
//!
//! WOFF2 is not the thin wrapper WOFF 1.0 is. The whole font is a single brotli stream,
//! and `glyf`/`loca` are normally stored in a re-encoded ("transformed") form that has to
//! be rebuilt point by point before an sfnt exists again. The two directions therefore
//! get very different treatment here.
//!
//! # Reading: delegated to the `wuff` crate
//!
//! Four pure-Rust candidates were compared against `testdata/fonts/Recursive-VF.subset.woff2`
//! before choosing (the comparison lives in `tools/woff2-decoder-eval/`, and
//! `tools/woff2-decoder-eval/README.md` says how to re-run it):
//!
//! | crate | verdict |
//! |---|---|
//! | `woff2` 0.3.0 | **does not compile.** Its `safer-bytes` dependency changed its error type within a semver-compatible range and nothing was ever released to fix it; last published 2022. |
//! | `woff2-patched` 0.4.0 | works, but pads every glyph in the reconstructed `glyf` to a four-byte boundary, so the table comes back 480 bytes where the original was 460 and `loca` differs to match. Legal, but not the original font. Rejects a transformed `hmtx`. |
//! | `woff2-no-std` 0.3.4 | a further fork of the same code; identical output, identical `hmtx` gap. |
//! | `wuff` 0.2.8 | reconstructs the fixture to exactly the original 18288 bytes with `loca` byte-identical, implements the `hmtx` transform as well as `glyf`/`loca`, is `no_std` + pure Rust, and is the only one of the four under active maintenance (0.2.8 in July 2026, ~217k recent downloads; it is the decoder Servo/Blitz use). |
//!
//! `wuff` also caps brotli output at the size the table directory declares, so a
//! decompression bomb cannot be used to exhaust memory — the same guard the reference
//! C++ decoder has, and a good reason not to hand-roll this against the specification.
//!
//! Neither `wuff` nor the `brotli` crate below links any C: `cargo build -p slice-core
//! --target wasm32-unknown-unknown` is what keeps that honest.
//!
//! Reconstructed `glyf` bytes are *not* expected to equal the original font's. The
//! transform re-encodes outlines, and a point delta that the original spelled with a
//! long coordinate may legitimately come back as a short one. The outlines are the same;
//! the bytes need not be.
//!
//! # Writing: the null transform
//!
//! The specification allows `glyf` and `loca` to be stored with transformVersion 3, the
//! null transform, which means "these are the plain sfnt tables, brotli-compressed".
//! That is what [`encode_woff2`] does for every table, so there is no transform encoder
//! here at all. The result is a fully conformant WOFF2 file, and a byte-exact round trip
//! back to the input sfnt — but on a `glyf`-heavy font it is larger than one a
//! transforming encoder such as fontTools or Google's `woff2_compress` would produce,
//! because the outlines are compressed as-is rather than re-encoded into the streams the
//! transform defines. Measured against fontTools 4.x on the same input:
//!
//! | font | sfnt | WOFF | this encoder | fontTools WOFF2 |
//! |---|---|---|---|---|
//! | `testdata/fonts/Recursive-VF.subset.ttf` | 18288 | 7855 | **6280** | 6328 |
//! | DejaVuSans 2.37 | 759720 | 381431 | **307404** | 258588 |
//! | LiberationSerif 2.1.5 | 393692 | 210337 | **173844** | 146604 |
//!
//! So about 19% behind a transforming encoder on a large static font, and — on a small
//! variable subset, where `glyf` is a minor part of the file next to `gvar` and the
//! layout tables — very slightly ahead of it. Always well ahead of WOFF 1.0.

use brotli::enc::backward_references::BrotliEncoderMode;
use brotli::enc::BrotliEncoderParams;

use crate::font::woff::split_sfnt;
use crate::SliceError;

const WOFF2_SIGNATURE: &[u8; 4] = b"wOF2";
const WOFF2_HEADER_LEN: usize = 48;
const SFNT_HEADER_LEN: usize = 12;
const SFNT_RECORD_LEN: usize = 16;

/// Offset of the `flags` field in the `head` table, and the bit the specification tells a
/// web font encoder to set there.
const HEAD_FLAGS_OFFSET: usize = 16;
const HEAD_FLAGS_CONVERTED_BIT: u16 = 1 << 11;

/// The 63 tags a WOFF2 table directory can name by index instead of spelling out.
///
/// Order is normative: this is Table 1 of the WOFF2 specification, and index 63 is
/// reserved to mean "a four-byte tag follows".
const KNOWN_TAGS: [&[u8; 4]; 63] = [
    b"cmap", b"head", b"hhea", b"hmtx", b"maxp", b"name", b"OS/2", b"post", b"cvt ", b"fpgm",
    b"glyf", b"loca", b"prep", b"CFF ", b"VORG", b"EBDT", b"EBLC", b"gasp", b"hdmx", b"kern",
    b"LTSH", b"PCLT", b"VDMX", b"vhea", b"vmtx", b"BASE", b"GDEF", b"GPOS", b"GSUB", b"EBSC",
    b"JSTF", b"MATH", b"CBDT", b"CBLC", b"COLR", b"CPAL", b"SVG ", b"sbix", b"acnt", b"avar",
    b"bdat", b"bloc", b"bsln", b"cvar", b"fdsc", b"feat", b"fmtx", b"fvar", b"gvar", b"hsty",
    b"just", b"lcar", b"mort", b"morx", b"opbd", b"prop", b"trak", b"Zapf", b"Silf", b"Glat",
    b"Gloc", b"Feat", b"Sill",
];

/// The escape index meaning "the tag is written out in full after the flags byte".
const ARBITRARY_TAG: u8 = 63;

/// Turn WOFF2 bytes into the sfnt they encode.
pub fn decode_woff2(data: &[u8]) -> Result<Vec<u8>, SliceError> {
    if data.len() < WOFF2_HEADER_LEN || &data[..4] != WOFF2_SIGNATURE {
        return Err(SliceError::Read("not a WOFF2 file".into()));
    }
    // `wuff`'s error type carries no detail, so say what the possibilities are rather
    // than printing a bare "GenericError" at someone.
    wuff::decompress_woff2(data).map_err(|_| {
        SliceError::Read(
            "the WOFF2 file could not be decoded: it is truncated, corrupt, or uses a \
             transform this build does not implement"
                .into(),
        )
    })
}

/// Wrap an sfnt in a WOFF2 container, storing every table with the null transform.
///
/// See the module comment: this is conformant but larger than a transforming encoder's
/// output.
pub fn encode_woff2(sfnt: &[u8]) -> Result<Vec<u8>, SliceError> {
    if sfnt.get(..4) == Some(&b"ttcf"[..]) {
        return Err(SliceError::Unsupported(
            "writing a font collection as WOFF2 is not supported".into(),
        ));
    }
    let (flavor, tables) = split_sfnt(sfnt)?;
    if tables.is_empty() {
        return Err(SliceError::Write("the font has no tables".into()));
    }
    if tables.len() > u16::MAX as usize {
        return Err(SliceError::Write("the font has too many tables".into()));
    }

    // Owned copies, because `head` is about to be edited.
    let mut tables: Vec<([u8; 4], Vec<u8>)> = tables
        .into_iter()
        .map(|(tag, _, bytes)| (tag, bytes.to_vec()))
        .collect();
    for (tag, bytes) in &mut tables {
        if tag == b"head" {
            set_converted_flag(bytes)?;
        }
    }

    let mut directory: Vec<u8> = Vec::new();
    let mut raw: Vec<u8> = Vec::new();
    // 12 for the sfnt header plus one 16-byte record per table, then every table padded
    // to a four-byte boundary: what the decoder will allocate.
    let mut total_sfnt_size: u64 = (SFNT_HEADER_LEN + tables.len() * SFNT_RECORD_LEN) as u64;

    for (tag, bytes) in &tables {
        let orig_length = u32::try_from(bytes.len())
            .map_err(|_| SliceError::Write(format!("table {} is too large", tag_name(tag))))?;

        let index = KNOWN_TAGS
            .iter()
            .position(|known| *known == tag)
            .map(|i| i as u8)
            .unwrap_or(ARBITRARY_TAG);

        // Bits 6-7 are the transform version. For every table except `glyf` and `loca`,
        // version 0 *is* the null transform; for those two the sense is inverted and
        // version 3 is the null transform, version 0 being the real one.
        let transform_version: u8 = if tag == b"glyf" || tag == b"loca" {
            3
        } else {
            0
        };
        directory.push(index | (transform_version << 6));
        if index == ARBITRARY_TAG {
            directory.extend_from_slice(tag);
        }
        write_base128(&mut directory, orig_length);
        // No transformLength field: it is present only for a table that really was
        // transformed, which here is none of them.

        raw.extend_from_slice(bytes);
        total_sfnt_size += u64::from(orig_length).next_multiple_of(4);
    }

    let total_sfnt_size = u32::try_from(total_sfnt_size)
        .map_err(|_| SliceError::Write("the font is too large for a WOFF2 container".into()))?;
    let compressed = brotli_compress(&raw)?;

    // The compressed block is padded to a four-byte boundary. Nothing follows it here,
    // but the specification puts the metadata block at a four-byte offset and decoders
    // check the arithmetic whether or not that block exists: `wuff` rejects a file whose
    // total length is not a multiple of four, and so does the reference decoder.
    let unpadded = WOFF2_HEADER_LEN + directory.len() + compressed.len();
    let total_len = unpadded.next_multiple_of(4);
    let length = u32::try_from(total_len)
        .map_err(|_| SliceError::Write("the font is too large for a WOFF2 container".into()))?;
    let compressed_len = compressed.len() as u32; // <= total_len, already checked

    let mut out = Vec::with_capacity(total_len);
    out.extend_from_slice(WOFF2_SIGNATURE);
    out.extend_from_slice(&flavor.to_be_bytes());
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(&(tables.len() as u16).to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes()); // reserved
    out.extend_from_slice(&total_sfnt_size.to_be_bytes());
    out.extend_from_slice(&compressed_len.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes()); // majorVersion
    out.extend_from_slice(&0u16.to_be_bytes()); // minorVersion
    out.extend_from_slice(&0u32.to_be_bytes()); // metaOffset
    out.extend_from_slice(&0u32.to_be_bytes()); // metaLength
    out.extend_from_slice(&0u32.to_be_bytes()); // metaOrigLength
    out.extend_from_slice(&0u32.to_be_bytes()); // privOffset
    out.extend_from_slice(&0u32.to_be_bytes()); // privLength
    debug_assert_eq!(out.len(), WOFF2_HEADER_LEN);
    out.extend_from_slice(&directory);
    out.extend_from_slice(&compressed);
    out.resize(total_len, 0);
    Ok(out)
}

/// Set `head.flags` bit 11, "font converted", which the WOFF specifications require of
/// anything that repacks a font losslessly.
fn set_converted_flag(head: &mut [u8]) -> Result<(), SliceError> {
    let field = head
        .get_mut(HEAD_FLAGS_OFFSET..HEAD_FLAGS_OFFSET + 2)
        .ok_or_else(|| SliceError::Write("the head table is too short".into()))?;
    let flags = u16::from_be_bytes([field[0], field[1]]) | HEAD_FLAGS_CONVERTED_BIT;
    field.copy_from_slice(&flags.to_be_bytes());
    Ok(())
}

/// Brotli-compress the concatenated table data.
///
/// Quality 11 and `MODE_FONT` are what fontTools and Google's `woff2_compress` use, and
/// WOFF2 exists to be small, so the slower setting is the right default even in
/// WebAssembly. Window 22 is the largest the reference decoder is guaranteed to accept.
fn brotli_compress(raw: &[u8]) -> Result<Vec<u8>, SliceError> {
    let params = BrotliEncoderParams {
        quality: 11,
        lgwin: 22,
        mode: BrotliEncoderMode::BROTLI_MODE_FONT,
        size_hint: raw.len(),
        ..Default::default()
    };
    let mut out = Vec::new();
    brotli::BrotliCompress(&mut &raw[..], &mut out, &params)
        .map_err(|e| SliceError::Write(format!("brotli compression failed: {e}")))?;
    Ok(out)
}

/// Append `value` as a WOFF2 `UIntBase128`: seven bits per byte, most significant group
/// first, every byte but the last carrying the continuation bit.
fn write_base128(out: &mut Vec<u8>, value: u32) {
    // 32 bits at 7 per byte is five bytes at most, and the specification forbids leading
    // zero groups, so start at the highest group that is actually used.
    let mut size = 1;
    while size < 5 && value >> (7 * size) != 0 {
        size += 1;
    }
    for i in (0..size).rev() {
        let mut byte = ((value >> (7 * i)) & 0x7f) as u8;
        if i != 0 {
            byte |= 0x80;
        }
        out.push(byte);
    }
}

fn tag_name(tag: &[u8; 4]) -> String {
    String::from_utf8_lossy(tag).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::woff::{decode_woff, encode_woff};
    use read_fonts::{FontRef, TableProvider};
    use skrifa::instance::{LocationRef, Size};
    use skrifa::outline::{DrawSettings, OutlinePen};
    use skrifa::{GlyphId, MetadataProvider};

    /// `head` fields that are allowed to differ: `checkSumAdjustment`, which every
    /// container recomputes, and `flags`, whose bit 11 a WOFF2 writer must set.
    const HEAD_VOLATILE: [std::ops::Range<usize>; 2] = [8..12, 16..18];
    /// Additionally `modified`, for the two fixtures, which are independent fontTools
    /// outputs rather than one repack of the other.
    const HEAD_MODIFIED: std::ops::Range<usize> = 28..36;

    fn tables(sfnt: &[u8]) -> Vec<(String, Vec<u8>)> {
        let (_, tables) = crate::font::woff::split_sfnt(sfnt).unwrap();
        tables
            .into_iter()
            .map(|(tag, _, bytes)| (tag_name(&tag), bytes.to_vec()))
            .collect()
    }

    /// Every outline in the font, as a flat list of pen commands.
    fn outlines(sfnt: &[u8]) -> Vec<Vec<String>> {
        #[derive(Default)]
        struct Recorder(Vec<String>);
        impl OutlinePen for Recorder {
            fn move_to(&mut self, x: f32, y: f32) {
                self.0.push(format!("M {x} {y}"));
            }
            fn line_to(&mut self, x: f32, y: f32) {
                self.0.push(format!("L {x} {y}"));
            }
            fn quad_to(&mut self, a: f32, b: f32, x: f32, y: f32) {
                self.0.push(format!("Q {a} {b} {x} {y}"));
            }
            fn curve_to(&mut self, a: f32, b: f32, c: f32, d: f32, x: f32, y: f32) {
                self.0.push(format!("C {a} {b} {c} {d} {x} {y}"));
            }
            fn close(&mut self) {
                self.0.push("Z".into());
            }
        }

        let font = FontRef::new(sfnt).unwrap();
        let outlines = font.outline_glyphs();
        let count = font.maxp().unwrap().num_glyphs();
        (0..count)
            .map(|gid| {
                let mut pen = Recorder::default();
                if let Some(glyph) = outlines.get(GlyphId::from(gid)) {
                    glyph
                        .draw(
                            DrawSettings::unhinted(Size::unscaled(), LocationRef::default()),
                            &mut pen,
                        )
                        .unwrap();
                }
                pen.0
            })
            .collect()
    }

    #[test]
    fn base128_matches_the_worked_examples_in_the_spec() {
        let encode = |v| {
            let mut out = Vec::new();
            write_base128(&mut out, v);
            out
        };
        assert_eq!(encode(0), vec![0x00]);
        assert_eq!(encode(1), vec![0x01]);
        assert_eq!(encode(127), vec![0x7f]);
        assert_eq!(encode(128), vec![0x81, 0x00]);
        assert_eq!(encode(0x3fff), vec![0xff, 0x7f]);
        assert_eq!(encode(0x4000), vec![0x81, 0x80, 0x00]);
        assert_eq!(encode(u32::MAX), vec![0x8f, 0xff, 0xff, 0xff, 0x7f]);
        // No encoding may begin with 0x80: that would be a leading zero group.
        for v in [0u32, 1, 127, 128, 16384, 1 << 28, u32::MAX] {
            assert_ne!(encode(v)[0], 0x80, "leading zero group for {v}");
        }
    }

    #[test]
    fn the_known_tag_table_is_the_one_the_spec_defines() {
        assert_eq!(KNOWN_TAGS.len(), 63);
        assert_eq!(KNOWN_TAGS[0], b"cmap");
        assert_eq!(KNOWN_TAGS[10], b"glyf");
        assert_eq!(KNOWN_TAGS[11], b"loca");
        assert_eq!(KNOWN_TAGS[47], b"fvar");
        assert_eq!(KNOWN_TAGS[62], b"Sill");
        // `STAT` is deliberately absent, so it exercises the escape path.
        assert!(!KNOWN_TAGS.contains(&b"STAT"));
    }

    #[test]
    fn woff2_input_decodes_to_a_parseable_sfnt() {
        let sfnt = decode_woff2(crate::testdata::recursive_vf_woff2()).unwrap();
        assert_eq!(&sfnt[..4], &[0x00, 0x01, 0x00, 0x00]);
        FontRef::new(&sfnt).expect("decoded WOFF2 should parse as a font");
    }

    #[test]
    fn decoded_woff2_has_the_same_tables_as_the_ttf() {
        let from_woff2 = decode_woff2(crate::testdata::recursive_vf_woff2()).unwrap();
        let ttf = crate::testdata::recursive_vf();

        let a = tables(&from_woff2);
        let b = tables(ttf);
        let a_tags: Vec<_> = a.iter().map(|(t, _)| t.clone()).collect();
        let b_tags: Vec<_> = b.iter().map(|(t, _)| t.clone()).collect();
        assert_eq!(a_tags, b_tags);

        for ((tag, ab), (_, bb)) in a.iter().zip(b.iter()) {
            match tag.as_str() {
                // The transform re-encodes outlines, so `glyf` need not come back
                // byte-identical even though the outlines do; that is checked
                // separately, by drawing them.
                "glyf" => {}
                "head" => {
                    assert_eq!(ab.len(), bb.len(), "head length differs");
                    let mut ab = ab.clone();
                    let mut bb = bb.clone();
                    for range in HEAD_VOLATILE.into_iter().chain([HEAD_MODIFIED]) {
                        ab[range.clone()].fill(0);
                        bb[range].fill(0);
                    }
                    assert_eq!(ab, bb, "head differs outside its volatile fields");
                }
                _ => assert_eq!(ab, bb, "table {tag} differs"),
            }
        }
    }

    #[test]
    fn decoded_woff2_draws_the_same_outlines_as_the_ttf() {
        let from_woff2 = decode_woff2(crate::testdata::recursive_vf_woff2()).unwrap();
        let ttf = crate::testdata::recursive_vf();
        let a = outlines(&from_woff2);
        let b = outlines(ttf);
        assert!(!a.is_empty());
        assert_eq!(a.len(), b.len(), "glyph counts differ");
        for (gid, (a, b)) in a.iter().zip(b.iter()).enumerate() {
            assert_eq!(a, b, "glyph {gid} differs");
        }
    }

    #[test]
    fn woff2_round_trip_preserves_every_table_byte_for_byte() {
        let ttf = crate::testdata::recursive_vf();
        let woff2 = encode_woff2(ttf).unwrap();
        assert_eq!(&woff2[..4], b"wOF2");
        let back = decode_woff2(&woff2).unwrap();

        let orig = tables(ttf);
        let round = tables(&back);
        assert_eq!(orig.len(), round.len());
        for ((tag, a), (_, b)) in orig.iter().zip(round.iter()) {
            if tag == "head" {
                // Bit 11 of head.flags is set on the way out, by design, and
                // checkSumAdjustment follows from it.
                let mut a = a.clone();
                let mut b = b.clone();
                assert_eq!(
                    u16::from_be_bytes([b[16], b[17]]) & HEAD_FLAGS_CONVERTED_BIT,
                    HEAD_FLAGS_CONVERTED_BIT,
                    "the writer should have set head.flags bit 11"
                );
                for range in HEAD_VOLATILE {
                    a[range.clone()].fill(0);
                    b[range].fill(0);
                }
                assert_eq!(a, b, "head differs outside its volatile fields");
            } else {
                assert_eq!(a, b, "table {tag} changed");
            }
        }
    }

    #[test]
    fn a_woff2_we_wrote_beats_both_the_sfnt_and_the_woff() {
        let ttf = crate::testdata::recursive_vf();
        let ours = encode_woff2(ttf).unwrap();
        let woff = encode_woff(ttf).unwrap();
        assert!(
            ours.len() < woff.len() && woff.len() < ttf.len(),
            "expected woff2 < woff < ttf, got {} < {} < {}",
            ours.len(),
            woff.len(),
            ttf.len()
        );
        // The file is a multiple of four bytes long, which decoders check: the block
        // after the compressed data has to start on a four-byte boundary whether or not
        // one is present.
        assert_eq!(ours.len() % 4, 0, "the file should be four-byte aligned");
    }

    #[test]
    fn the_round_trip_survives_a_second_pass_through_woff() {
        // WOFF and WOFF2 should agree on what the font is, whichever order they are
        // applied in.
        let ttf = crate::testdata::recursive_vf();
        let via_woff2 = decode_woff2(&encode_woff2(ttf).unwrap()).unwrap();
        let via_both = decode_woff(&encode_woff(&via_woff2).unwrap()).unwrap();
        assert_eq!(tables(&via_woff2), tables(&via_both));
    }

    #[test]
    fn truncated_woff2_is_rejected_rather_than_panicking() {
        let woff2 = crate::testdata::recursive_vf_woff2();
        for cut in [
            0,
            1,
            4,
            20,
            47,
            48,
            60,
            200,
            woff2.len() / 2,
            woff2.len() - 1,
        ] {
            assert!(
                decode_woff2(&woff2[..cut]).is_err(),
                "truncation at {cut} should be an error"
            );
        }
    }

    #[test]
    fn malformed_woff2_is_rejected_rather_than_panicking() {
        let good = crate::testdata::recursive_vf_woff2();

        assert!(decode_woff2(b"").is_err());
        assert!(decode_woff2(&[0u8; 48]).is_err(), "bad signature");
        assert!(
            decode_woff2(crate::testdata::recursive_vf()).is_err(),
            "a plain sfnt is not a WOFF2"
        );
        assert!(
            decode_woff2(crate::testdata::recursive_vf_woff()).is_err(),
            "a WOFF 1.0 file is not a WOFF2"
        );

        // A header that claims an absurd number of tables, or an absurd decompressed
        // size, must not be believed.
        let mut lying = good.to_vec();
        lying[12..14].copy_from_slice(&u16::MAX.to_be_bytes());
        assert!(decode_woff2(&lying).is_err(), "numTables = 65535");

        // `totalSfntSize` is deliberately *not* in this list: a good decoder derives the
        // size it allocates from the table directory and never trusts that field, which
        // is exactly what `wuff` does, so lying in it changes nothing.

        let mut lying = good.to_vec();
        lying[20..24].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(decode_woff2(&lying).is_err(), "totalCompressedSize = 4 GiB");

        // Corrupt bytes scattered through the directory and the brotli stream.
        for at in [48, 49, 60, 100, good.len() / 2, good.len() - 1] {
            let mut broken = good.to_vec();
            broken[at] ^= 0xff;
            // Some of these may still decode to something; the requirement is only that
            // the decoder returns rather than panicking or hanging.
            let _ = decode_woff2(&broken);
        }
    }

    #[test]
    fn encoding_refuses_input_that_is_not_a_font() {
        assert!(encode_woff2(b"").is_err());
        assert!(encode_woff2(b"not a font at all").is_err());
        // A collection has a different directory layout and is not handled.
        let mut ttc = crate::testdata::recursive_vf().to_vec();
        ttc[..4].copy_from_slice(b"ttcf");
        assert!(encode_woff2(&ttc).is_err());
    }

    #[test]
    fn the_header_says_what_the_file_actually_contains() {
        let ttf = crate::testdata::recursive_vf();
        let woff2 = encode_woff2(ttf).unwrap();
        let be32 = |at: usize| u32::from_be_bytes(woff2[at..at + 4].try_into().unwrap());

        assert_eq!(&woff2[4..8], &ttf[..4], "flavor should be the sfnt's");
        assert_eq!(be32(8) as usize, woff2.len(), "length");
        let num_tables = u16::from_be_bytes([woff2[12], woff2[13]]) as usize;
        assert_eq!(num_tables, tables(ttf).len());

        // totalSfntSize is what the decoder allocates: header, directory, padded tables.
        let expected: usize = SFNT_HEADER_LEN
            + num_tables * SFNT_RECORD_LEN
            + tables(ttf)
                .iter()
                .map(|(_, b)| b.len().next_multiple_of(4))
                .sum::<usize>();
        assert_eq!(be32(16) as usize, expected, "totalSfntSize");
        assert_eq!(
            &woff2[24..48],
            &[0u8; 24],
            "version, meta and priv are empty"
        );

        // Walk the table directory the way a decoder does, and check that it ends
        // exactly where the compressed block the header describes begins.
        let mut at = WOFF2_HEADER_LEN;
        for (tag, bytes) in tables(ttf) {
            let flags = woff2[at];
            at += 1;
            let index = flags & 0x3f;
            let expected_version = if tag == "glyf" || tag == "loca" { 3 } else { 0 };
            assert_eq!(flags >> 6, expected_version, "transform version for {tag}");
            if index == ARBITRARY_TAG {
                assert_eq!(&woff2[at..at + 4], tag.as_bytes(), "explicit tag");
                at += 4;
            } else {
                assert_eq!(KNOWN_TAGS[index as usize], tag.as_bytes(), "tag index");
            }
            // UIntBase128 origLength, and no transformLength: nothing was transformed.
            let mut orig_length: u32 = 0;
            loop {
                let byte = woff2[at];
                at += 1;
                orig_length = (orig_length << 7) | u32::from(byte & 0x7f);
                if byte & 0x80 == 0 {
                    break;
                }
            }
            assert_eq!(orig_length as usize, bytes.len(), "origLength for {tag}");
        }
        assert_eq!(
            (at + be32(20) as usize).next_multiple_of(4),
            woff2.len(),
            "header, directory, compressed block and padding should be the whole file"
        );
    }
}
