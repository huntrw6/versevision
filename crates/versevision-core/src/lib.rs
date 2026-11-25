//! VerseVision core. No network, no files except through traits defined here later.
//!
//! S1: the config model, the line fitting and the plan types. S2a: Scripture references, versification and Bible versions.
//! S2b: the notes classifier. S3: the planner (`workflow`), the slide planner (`slides`), the Bible text trait (`bibles`).

pub mod bibles;
pub mod config;
pub mod fitting;
pub mod looks;
pub mod notes;
pub mod plan;
pub mod pyfmt;
pub mod pystr;
pub mod references;
pub mod slides;
pub mod versions;
pub mod workflow;

/// The version the program reports: `versevision --version`, the web page's `/api/status`, the run log, the User-Agent. The one place it is
/// written (the Cargo workspace version is a separate, internal label).
pub const VERSEVISION_VERSION: &str = "1.0.0-dev";
