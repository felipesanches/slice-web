//! Which pure-Rust WOFF2 decoder should slice-core use?
//!
//! Decodes one WOFF2 file with each candidate and compares the reconstruction, table by
//! table, against the sfnt it was made from. The answer this produced is recorded in the
//! module comment of `crates/slice-core/src/font/woff2.rs`; re-run it before changing
//! that choice. See README.md for the exact command.

use std::collections::BTreeMap;
use std::fs;

/// A reconstructed font, split into `tag -> bytes`.
fn tables(sfnt: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let num_tables = u16::from_be_bytes([sfnt[4], sfnt[5]]) as usize;
    (0..num_tables)
        .map(|i| {
            let base = 12 + i * 16;
            let tag = String::from_utf8_lossy(&sfnt[base..base + 4]).to_string();
            let at = |o: usize| {
                u32::from_be_bytes(sfnt[base + o..base + o + 4].try_into().unwrap()) as usize
            };
            let (offset, length) = (at(8), at(12));
            (tag, sfnt[offset..offset + length].to_vec())
        })
        .collect()
}

/// `head` fields nobody should expect to survive: checkSumAdjustment, flags (bit 11 is
/// set by a WOFF2 writer), and `modified` when the two fixtures are independent outputs.
const HEAD_VOLATILE: [std::ops::Range<usize>; 3] = [8..12, 16..18, 28..36];

fn report(name: &str, decoded: &[u8], reference: &[u8]) {
    println!("  {name}: {} bytes (reference {})", decoded.len(), reference.len());
    let got = tables(decoded);
    let want = tables(reference);
    if got.keys().ne(want.keys()) {
        println!("    TAG SETS DIFFER");
    }
    for (tag, want_bytes) in &want {
        let Some(got_bytes) = got.get(tag) else {
            println!("    {tag}: missing");
            continue;
        };
        if tag == "head" {
            let (mut a, mut b) = (got_bytes.clone(), want_bytes.clone());
            if a.len() == b.len() {
                for range in HEAD_VOLATILE {
                    a[range.clone()].fill(0);
                    b[range].fill(0);
                }
            }
            if a != b {
                println!("    head: differs outside checkSumAdjustment/flags/modified");
            }
            continue;
        }
        if got_bytes != want_bytes {
            let same_length = got_bytes.len() == want_bytes.len();
            let differing = if same_length {
                got_bytes
                    .iter()
                    .zip(want_bytes)
                    .filter(|(a, b)| a != b)
                    .count()
            } else {
                0
            };
            if same_length {
                println!("    {tag}: {differing} of {} bytes differ", want_bytes.len());
            } else {
                println!(
                    "    {tag}: length {}, reference {}",
                    got_bytes.len(),
                    want_bytes.len()
                );
            }
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let woff2_path = args.next().expect("usage: woff2-decoder-eval FONT.woff2 FONT.ttf");
    let ttf_path = args.next().expect("usage: woff2-decoder-eval FONT.woff2 FONT.ttf");
    let woff2 = fs::read(&woff2_path).unwrap();
    let ttf = fs::read(&ttf_path).unwrap();

    println!("{woff2_path} -> {ttf_path}");

    // `woff2` 0.3.0 is not here because it does not build: its `safer-bytes` dependency
    // replaced the error type its `?` operators convert from, within a semver-compatible
    // range, and nothing has been published since 2022.

    match woff2_patched::convert_woff2_to_ttf(&mut std::io::Cursor::new(&woff2)) {
        Ok(v) => report("woff2-patched 0.4.0", &v, &ttf),
        Err(e) => println!("  woff2-patched 0.4.0: ERROR {e}"),
    }
    match woff2::convert_woff2_to_ttf(&mut woff2.as_slice()) {
        Ok(v) => report("woff2-no-std 0.3.4", &v, &ttf),
        Err(e) => println!("  woff2-no-std 0.3.4: ERROR {e}"),
    }
    match wuff::decompress_woff2(&woff2) {
        Ok(v) => report("wuff 0.2.8", &v, &ttf),
        Err(e) => println!("  wuff 0.2.8: ERROR {e}"),
    }
}
