//! Fonts used by the engine tests.
//!
//! Gated behind the `testdata` feature so the bytes never reach the WebAssembly build.
//! See `testdata/README.md` for where these came from and what each one is for.

/// Recursive, 5 axes (`MONO`, `CASL`, `wght`, `slnt`, `CRSV`), subset.
pub fn recursive_vf() -> &'static [u8] {
    include_bytes!("../../../testdata/fonts/Recursive-VF.subset.ttf")
}

/// The same font, WOFF-wrapped.
pub fn recursive_vf_woff() -> &'static [u8] {
    include_bytes!("../../../testdata/fonts/Recursive-VF.subset.woff")
}

/// The same font, WOFF2-wrapped.
pub fn recursive_vf_woff2() -> &'static [u8] {
    include_bytes!("../../../testdata/fonts/Recursive-VF.subset.woff2")
}

/// A static instance the original Slice produced from `recursive_vf`.
pub fn recursive_sliced() -> &'static [u8] {
    include_bytes!("../../../testdata/fonts/Recursive-Sliced.subset.ttf")
}
