# Third-party licences

## Recursive

`web/fonts/RecursiveSans-Slice_mod.subset.ttf` and the fonts in `testdata/fonts/` are
derivatives of the [Recursive typeface](https://github.com/arrowtype/recursive) by
Stephen Nixon, used under the SIL Open Font License 1.1. See
[Recursive-OFL.txt](Recursive-OFL.txt).

The file in `web/fonts/` is the subset the original Slice used for its wordmark: the
variable font sliced at `MONO=0, CASL=0.5, wght=800, slnt=0, CRSV=1` and then subset with
`pyftsubset` to the characters in the word "Slice". It is carried over unchanged so this
version's wordmark matches the application it replaces. At 2.8 kB it costs nothing.

`testdata/fonts/` holds the test fixtures copied from the original repository; see
[testdata/README.md](../testdata/README.md).

## The original Slice

This project reimplements the interface and behaviour of
[Slice](https://github.com/source-foundry/Slice) by Source Foundry (Christopher
Simpkins), which is licensed under the GNU General Public License v3. This project
carries the same licence.

## Rust dependencies

The crates this depends on are Apache-2.0 / MIT, which the GPL-3.0 permits combining
with. `cargo tree` lists them; `cargo about` or `cargo deny` can produce a full report.
