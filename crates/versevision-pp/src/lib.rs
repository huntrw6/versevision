//! VerseVision's ProPresenter 19 side: the presentation file writer (protobuf, template, slides, RTF) and the theme reader (S6a), and
//! what talks to ProPresenter (S6b): its network API, the local transport, the swap pipeline that replaces or appends to a presentation
//! without restarting ProPresenter, history and undo, the run log, `explain` and `replay`. Everything that waits or stamps goes through a
//! [`clock::Clock`], so a run replays in virtual time. The command line that prints it all is S7's; the remote-node (SSH) transport is not ported yet.
//!
//! The schema is `proto/source/propresenter19/*.proto`, compiled to `proto/descriptor.bin` (pure Rust, by `protox`, through
//! `cargo run -p versevision-pp --features compile --example gen_descriptors`) and committed: building this crate needs no `protoc` and no
//! build script. A test proves the committed set is what the sources compile to.

pub mod api_client;
pub mod backup;
pub mod clock;
pub mod compat;
pub mod dirs;
pub mod error;
pub mod errors;
pub mod explain;
pub mod files;
pub mod history;
pub mod http;
pub mod import_workflow;
pub mod model;
pub mod pb;
pub mod pipeline;
pub mod process;
pub mod pyjson;
pub mod pyrepr;
pub mod rtf;
pub mod run;
pub mod runlog;
pub mod schema;
pub mod selection;
pub mod template;
pub mod themes;
pub mod transport;
pub mod writer;

pub use compat::CompatibilityPolicy;
pub use error::{PpError, Result};
pub use pb::{Message, PbError, Value};
pub use schema::*;
pub use writer::{
    PlanWriteResult, PresentationWriter, RandomUuids, SeededUuids, UuidSource, Written,
};
