//! Restricting a variable font's design space.

pub mod glyphs;
pub mod iup;
pub mod mvar;
pub mod normalize;
pub mod statics;

pub use normalize::{normalize_axis, normalize_location, NormalizedLocation};
pub use statics::instantiate_static;
