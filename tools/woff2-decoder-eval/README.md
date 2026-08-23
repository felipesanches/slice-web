# `woff2-decoder-eval`

**Question it answers:** which pure-Rust WOFF2 decoder reconstructs an sfnt most
faithfully, and which ones can still be built at all?

`crates/slice-core/src/font/woff2.rs` delegates WOFF2 *reading* to the `wuff` crate. This
is the harness that picked it. Re-run it before swapping that dependency, or when a new
candidate appears.

## Running it

```sh
cd tools/woff2-decoder-eval
cargo run -- ../../testdata/fonts/Recursive-VF.subset.woff2 \
             ../../testdata/fonts/Recursive-VF.subset.ttf
```

It decodes the WOFF2 with each candidate and diffs the result against the TTF the fixture
was made from, table by table. It is not part of the workspace, and has its own lockfile,
because it deliberately depends on crates slice-core does not.

## What the output means

- **A table listed as differing** is a reconstruction that is not the original font.
  `glyf` is the exception: the WOFF2 transform re-encodes outlines, so a decoder may
  legitimately return different bytes for the same curves. Every other table should come
  back untouched.
- **`head` is compared with `checkSumAdjustment` (8..12), `flags` (16..18) and `modified`
  (28..36) masked out.** The first is recomputed by every container, the second carries
  bit 11 which a WOFF2 writer must set, and the third differs because the `.woff2` and
  `.ttf` fixtures are independent fontTools runs rather than one repack of the other.
- **Total size equal to the reference** means the layout was reproduced too, not just the
  contents.

## Result, 2026-08 (rustc 1.93.1)

| crate | result |
|---|---|
| `woff2` 0.3.0 | does not compile. Its `safer-bytes` dependency changed the error type its `?` operators convert from within a semver-compatible range; nothing published since 2022. Left commented out in `Cargo.toml`. |
| `woff2-patched` 0.4.0 | decodes, but pads every glyph in `glyf` to four bytes: 480 bytes where the original had 460, and `loca` shifted to match. Rejects a transformed `hmtx`. |
| `woff2-no-std` 0.3.4 | a further fork of the same code; byte-for-byte the same output and the same `hmtx` gap. |
| **`wuff` 0.2.8** | 18288 bytes, exactly the original; `loca` identical; only `glyf` differs, as the transform allows. Implements the `hmtx` transform. Actively maintained. **Chosen.** |
