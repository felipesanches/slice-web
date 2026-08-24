# overlap-engine-eval

**Question it answers:** would `linesweeper` remove overlaps correctly on the shapes that
`flo_curves` gets wrong, and can it be used from WebAssembly?

```sh
cd tools/overlap-engine-eval
cargo run                                  # the probe report
cargo build --target wasm32-unknown-unknown   # that it can be used at all
```

Overlap removal is the least externally validated part of Slice, because no reference
implementation exists to diff it against — fontTools' instancer does not remove overlaps.
The engine in use is `flo_curves`, and it needed working around: its
`path_remove_interior_points` documents itself as the non-zero winding rule and is not
one. `GraphPath::from_path` normalizes every contour to clockwise before doing anything,
so nothing can cancel and every counter fills in solid. `crates/slice-core/src/overlaps.rs`
drives the ray caster directly instead, with per-contour labels and restored signs, and
re-winds the result by nesting depth.

`linesweeper` is a robust Bentley–Ottmann sweep line by Joe Neeman, operating on Bézier
paths through `kurbo`. This measures whether it needs any of that work.

## What it tests

The five glyphs of the `overlapping` fixture, reconstructed from the definitions in
`tests/suite/fixtures/build.py`, with the probe points and expected winding magnitudes
copied from `OVERLAP_PROBES` in the same file:

| glyph | what makes it hard |
|---|---|
| `bars` | two clockwise rectangles crossing: winding −2 in the middle |
| `o` | a counter — the case `flo_curves` fills in solid |
| `circled` | nesting depth 3, directions alternating |
| `bowtie` | one self-intersecting contour, two lobes of opposite winding |
| `clean` | a triangle with nothing to merge, which must come back untouched |

The probes deliberately include points that must come out **empty**. A boolean engine
that mishandles contour direction gets exactly those wrong, and a check that only compares
the outer boundary would not notice.

## Result, linesweeper 0.4.0

```
bars      2 contour(s) in, 1 out (0 marked as holes)
o         2 contour(s) in, 2 out (1 marked as holes)
circled   4 contour(s) in, 4 out (3 marked as holes)
bowtie    1 contour(s) in, 2 out (0 marked as holes)
clean     1 contour(s) in, 1 out (0 marked as holes)

20/20 probe points agree with the original filled region
```

Correct on every one, with no workaround: `binary_op(path, empty, FillRule::NonZero,
BinaryOp::Union)` is the whole call. It also builds for `wasm32-unknown-unknown`, and
`cargo tree` shows no `-sys`, `cc`, `cmake` or `bindgen` crate.

Two findings beyond the pass/fail:

- **It reports hole nesting.** Each returned `Contour` carries a `parent`, so holes are
  described explicitly rather than left to be recovered from winding direction.
  `overlaps.rs` currently reconstructs that by ray-casting a boundary point per contour.
- **It needs kurbo 0.13**, where Slice was on 0.11.3 when this was written. The two are
  semver-incompatible, so adopting `linesweeper` meant upgrading kurbo across the
  workspace. That turned out to cost nothing — kurbo was used in one file — and the
  upgrade landed with the switch.

## What this does not tell you

The fixtures are rectangles and triangles. Real glyphs are curves meeting at shallow
angles, which is where boolean geometry actually goes wrong, and nothing here exercises
that. A decision to switch should be made against the 775-font corpus sweep
(`tools/corpus-sweep.py`) with overlap removal enabled, not against this.

`linesweeper` also describes itself as "in an early beta state", and its repository has
moved from GitHub to Radicle — the GitHub mirror is archived, though 0.4.0 was published
to crates.io in June 2026, after that move.
