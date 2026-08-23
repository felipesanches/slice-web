//! Restricting a variable font's design space.

pub mod feature_vars;
pub mod glyphs;
pub mod iup;
pub mod mvar;
pub mod normalize;
pub mod partial;
pub mod statics;

pub use feature_vars::instantiate_feature_variations;
pub use normalize::{normalize_axis, normalize_location, NormalizedLocation};
pub use partial::{instantiate_partial, plan_axes, AxisPlan};
pub use statics::instantiate_static;
