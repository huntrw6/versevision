//! VerseVision core. No network, no files except through traits defined here later.
//!
//! S1: the config model, the line fitting and the plan types. S2a: Scripture references, versification and Bible versions.
//! S2b: the notes classifier. The planner comes next.

pub mod config;
pub mod fitting;
pub mod notes;
pub mod plan;
pub mod pystr;
pub mod references;
pub mod versions;
