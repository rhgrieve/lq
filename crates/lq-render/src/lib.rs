//! Offline Liquid render profiling.
//!
//! Renders a theme page against a mock shop with a Shopify-flavoured Liquid interpreter, and
//! records a profile in the shape Shopify's renderer produces (one node per rendered tag or
//! output, as the Liquid gem's profiler does). Time comes from a cost model: each node's
//! operation counts times calibrated per-operation costs. Absolute times are estimates;
//! rankings and before/after differences are the useful part.

pub mod calibrate;
pub mod cost;
mod filters;
pub mod interp;
pub mod page;
pub mod profile;
pub mod report;
pub mod shop;
pub mod speedscope;
pub mod theme;
pub mod value;

pub use cost::CostModel;
pub use interp::Limits;
pub use page::{render_page, PageRequest, PageResult, PAGE_TYPES};
pub use profile::Profile;
pub use shop::{Knobs, Shop};
pub use theme::Theme;
