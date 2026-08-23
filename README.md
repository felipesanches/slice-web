# Slice

**Build custom design sub-spaces from variable fonts, in the browser.**

A reimplementation of [Slice](https://github.com/source-foundry/Slice) by Source Foundry:
same interface, Rust engine, runs as a web page, and it removes overlapping contours —
the thing the original never did, and the reason this exists.

Fonts never leave your machine. The file is read by the browser, sliced by WebAssembly,
and handed straight back as a download. There is no server, and nothing to upload.

```
                Axis Editor          Name Editor        Bit Flag Editor
    variable  ┌─────────────┐      ┌────────────┐      ┌──────────────┐
      font ──▶│ wght 200:700│─────▶│ 01 Family… │─────▶│ ☑ bit 6 REG  │──▶ sliced font
              │ wdth  100   │      │ 02 Sub…    │      │ ☐ bit 5 BOLD │
              └─────────────┘      └────────────┘      └──────────────┘
                                                        + remove overlaps
```

## Why

Slice takes a variable font and gives you a smaller one: a single static instance, or a
variable font with fewer axes and narrower ranges. The original is a PyQt5 desktop
application built on fontTools. It works, and this keeps its interface deliberately
close, because the interface is good and people know it.

Two things are different.

**It removes overlaps.** Ten years on, support for overlapping contours in design
applications is still poor. A sliced instance whose stems overlap shows seams when
outlined, misbehaves under boolean operations, and exports badly. fontTools does this
through Skia's path ops, which has no WebAssembly build, so the union here is done with
`flo_curves` — see [Overlap removal](#overlap-removal), which is the most interesting
part of this repository.

**It runs in a browser.** No install, no Python environment, no platform builds to code
sign and notarise.

## Using it

Open the page, drop in a variable font, and fill in the Axis Editor:

| what you want | what you type | example |
|---|---|---|
| keep the whole axis | nothing | |
| pin the axis to one location | a number | `400` |
| restrict the axis to a smaller range | `min:max` | `200:700` |

Pin every axis and you get a static font. Leave any axis with a range and the result is
still variable, with a smaller design space.

A restricted range has to contain the axis's original default value. This is the Level 3
sub-spacing rule: the compiler cannot move a default axis location, so the range has to
keep it. Typing `400:700` on an axis that defaults to 300 is rejected, with the reason.

### Removing overlaps

Tick **Remove overlapping contours**. It needs every axis pinned — merging contours
renumbers a glyph's points, and `gvar` deltas are indexed by point number, so the two
cannot both be true. It also drops hinting, which no longer describes the outlines.

### From the command line

```sh
cargo run -p slice-cli -- info Recursive-VF.ttf
cargo run -p slice-cli -- cut Recursive-VF.ttf Bold.ttf \
    --axis wght=800 --axis CASL=1 --remove-overlaps
```

`info` prints exactly what the three editors would show. `cut` takes the same axis
syntax the interface accepts.

## Building it

Needs a Rust toolchain with the `wasm32-unknown-unknown` target, and `wasm-pack`.

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-pack

./build.sh                  # release build into dist/
./build.sh --dev            # faster, much larger .wasm, panics keep their messages
python3 -m http.server --directory dist 8080
```

`dist/` is a directory of static files. Copy it anywhere that serves files, as long as
`.wasm` is served as `application/wasm`.

```sh
cargo test --workspace      # the engine
tools/browser-smoke.sh      # that it starts in a real browser and reads a font
```

## How it is put together

```
crates/slice-core   the engine: no browser-specific dependencies, so all of it
                    can be tested from a native test binary
crates/slice-cli    a command-line front end, for tests and build scripts
crates/slice-web    the Leptos interface, compiled to WebAssembly
tools/              scripts that produce or re-check committed artefacts
testdata/           fixtures, copied from the original repository
```

The engine is built on [fontations](https://github.com/googlefonts/fontations) —
`read-fonts` for reading, `write-fonts` for writing, and `skrifa` both as a dependency
and, more importantly, as the thing the tests are checked against.

### Verification

There is no Rust equivalent of fontTools' instancer, so the variation maths here is a
port, and a port is worth only as much as the evidence that it matches. Three
independent checks:

**The sub-space solver is checked against fontTools' own test vectors.**
`crates/slice-core/src/solver.rs` is a hand port of
`fontTools.varLib.instancer.solver`. `tools/gen-solver-vectors.py` lifts upstream's
parametrised test table verbatim from a pinned release and compiles it into a Rust
`const`. All 32 cases pass.

**Instancing is checked against skrifa.** skrifa is a separate implementation of the
same specification, and it is what browsers and Android use to render these fonts. So:

> drawing the *variable* font at location L must produce the same outlines and advances
> as drawing our *instance at L* at its own default.

For static instances, over 15 locations and every glyph, the observed deviation is **0
font units** — outlines and advances both, and the test asserts exactly that rather than
a tolerance. For partial instances the same property is checked at locations sampled
across each restricted range; there the largest deviation is **1 font unit**, which is
the integer rounding of `gvar` deltas.

This oracle earned its keep. It found IUP interpolating a tuple's missing deltas against
coordinates that earlier tuples had already moved, and normalized coordinates being
computed in `f64` when a font stores them as F2Dot14 — neither of which a single-axis
test would have caught.

**Overlap removal is checked by filled region, not by outline.** The outline is supposed
to change; what must not change is which points are inside the glyph. Both the before and
after are filled with the non-zero winding rule and compared over a dense grid of sample
points.

## Overlap removal

The interesting part, and the one worth reading the code for.

`flo_curves` ships `path_remove_interior_points`, which looks like exactly the right
function and is not. `GraphPath::from_path` reverses every anti-clockwise contour to
clockwise before building its graph, so by the time winding is counted nothing can
cancel out. Run an `o` through it and the counter fills in solid.

The ray-casting pass underneath it, though, keeps a *signed* crossing count per path
label, and lets the caller decide what "inside" means. So each contour gets its own
label, the reversals `from_path` performed are recorded, and the crossings are summed
with those reversals undone. That total is the true winding number, and testing it
against zero is the non-zero rule that `glyf` is defined in terms of.

`flo_curves` then hands back its result as an *even-odd* set, with every contour turning
the same way — which `glyf` would fill solid all over again. So the contours are re-wound
by nesting depth, measured from a point on each contour's own boundary. Not from inside
it: the bounding-box centre of an `o` sits inside its own counter, which makes the outer
contour look one level deeper than it is.

Tested against a counter, a counter inside a counter (a circled letter — the case that
rules out the tempting shortcut of unioning the outer-wound contours and subtracting the
inner-wound ones), overlapping rings, same-direction nesting, and a self-intersecting bow
tie.

## What it does not do yet

Stated plainly, because a font tool that is quiet about its gaps is worse than one that
has them.

- **CFF outlines.** TrueType (`glyf`) only. A `CFF`/`CFF2` font is refused rather than
  mangled.
- **WOFF2.** Reading and writing WOFF works; WOFF2 needs the glyf transform on top of
  brotli and is not implemented. WOFF2 input is refused with a message saying so.
- **Variable positioning.** A font with a `GDEF` item variation store — variable kerning
  and anchors — is refused for *partial* instancing, because leaving that data keeps
  regions describing an axis space that no longer exists, and removing it dangles the
  indices pointing into it. For *static* instances the table is carried through and
  evaluates to its default, so kerning comes out at the default master's values rather
  than the pinned location's.
- **`MVAR` across a restricted range.** Applied at the new default and then dropped, so
  vertical metrics are right there but stop varying across whatever range is left.
- **`avar` version 2.** Refused for partial instancing.
- **Threading.** The engine runs on the main thread, so the page is unresponsive while a
  large font is processed. The progress dialog says so rather than showing a bar that
  pretends to move. A Web Worker is the answer and is not here yet.

## Licences

GPL-3.0-or-later, as the original is. See [LICENSE](LICENSE).

The wordmark uses the Recursive subset the original embedded, and the test fixtures are
Recursive subsets too; both under the SIL Open Font License. See
[thirdparty/](thirdparty/).

The Rust dependencies are Apache-2.0 / MIT.

## Acknowledgements

[Slice](https://github.com/source-foundry/Slice) by Christopher Simpkins and the Source
Foundry authors, whose interface this keeps.
[fontTools](https://github.com/fonttools/fonttools), whose instancer the sub-space solver
is a port of. [fontations](https://github.com/googlefonts/fontations), which does the
reading and writing and is also the yardstick the results are measured against.
[Recursive](https://github.com/arrowtype/recursive) by Stephen Nixon.
