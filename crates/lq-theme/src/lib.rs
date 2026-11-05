//! Theme-level plumbing for `lq`: ports of `minimatch`, theme-check's ignore
//! rules, `glob`-based file discovery, the `yaml` package's core-schema subset
//! and theme-check-node's config resolution.

pub mod config;
pub mod discovery;
pub mod divergence;
pub mod docs;
pub mod ignore;
pub mod minimatch;
pub mod yaml;
