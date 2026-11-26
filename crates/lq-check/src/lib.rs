//! The lint engine of `lq`, a port of `@shopify/theme-check-common` and `theme-graph` at
//! Shopify/theme-tools `main` (af2e1a7), minus the individual checks' upstream code. See `PORTING.md`.
//!
//! - [`api`]: what a check is written against.
//! - [`registry`]: all checks; [`checks`]: one file per check.
//! - [`run`]: `check()`: one AST walk per Liquid file, files in parallel.
//! - [`deps`]: the dependencies a run uses, [`deps::NodeDependencies`] for real themes.
//! - [`golden`]: replay of recorded check runs (`lq debug-check-golden`), and the mock
//!   dependencies of upstream's test helper, which [`upstream_specs`] runs upstream's specs on.

pub mod api;
pub mod block_parameters;
pub mod check_utils;
pub mod checks;
pub mod context_utils;
pub mod deps;
pub mod disabled_checks;
pub mod fixes;
pub mod fs;
pub mod golden;
pub mod upstream_specs;
pub mod graph;
pub mod liquid_doc;
pub mod registry;
pub mod run;
pub mod schema_settings;
pub mod source;
pub mod to_schema;
pub mod utils;
pub mod visible_if;
pub mod visitor;

pub use api::*;
pub use run::{check, Offense, Position, RunOutput};
pub use source::{parse_theme, to_source_code, Ast, SourceCode, Theme};
