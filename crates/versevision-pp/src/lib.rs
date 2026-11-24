//! VerseVision's ProPresenter 19 side: the presentation file writer (protobuf, template, slides, RTF) and the theme reader.
//!
//! The schema is `proto/source/propresenter19/*.proto`, compiled to `proto/descriptor.bin` (pure Rust, by `protox`, through
//! `cargo run -p versevision-pp --features compile --example gen_descriptors`) and committed: building this crate needs no `protoc` and no
//! build script. A test proves the committed set is what the sources compile to.

pub mod backup;
pub mod compat;
pub mod error;
pub mod files;
pub mod model;
pub mod pb;
pub mod rtf;
pub mod schema;
pub mod template;
pub mod themes;
pub mod writer;

pub use compat::CompatibilityPolicy;
pub use error::{PpError, Result};
pub use pb::{Message, PbError, Value};
pub use schema::*;
pub use writer::{
    PlanWriteResult, PresentationWriter, RandomUuids, SeededUuids, UuidSource, Written,
};
