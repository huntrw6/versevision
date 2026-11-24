//! The notes classifier: the text of a document -> verses, sermon points, skipped lines and lines to review.
//!
//! A port of `python/src/versevision/notes/` (`classifier.py`, `title.py`). The structure follows the Python on purpose: this is where the
//! weekly format fixes land, so every rule keeps its own function (`label_*` in [`classifier`]), its rule name as a string and its comments.
//! The output has to equal `oracle/cases/*/expected/document.json` and the `parse_start`, `line_join`, `paragraph` and `item` records of
//! `expected/trace.jsonl`.
//!
//! Where Rust differs from Python on purpose:
//! * Python raises (`AttributeError`, pydantic `ValidationError`, `re.error`) where this returns a [`ClassifyError`] with the same exception
//!   name and message (`oracle/document_errors.jsonl` holds them).
//! * Patterns the user writes (`notes.skip_patterns`) are Python `re` syntax; see [`pyre`] for what is translated and what is not.
//! * `title::document_title` returns a `Result` because the reference parser can raise (chapter 0) on a title line.

pub mod classifier;
pub mod models;
pub mod pyre;
pub mod title;
pub mod trace;

pub use classifier::{classify_document, classify_document_traced, ClassifyError, Shortcuts};
pub use models::{
    DocumentItem, NoteDocument, PointItem, PointRole, ReviewItem, SkipCategory, SkippedItem,
    VerseItem,
};
pub use title::document_title;
pub use trace::{TraceLog, TraceSink};

#[cfg(test)]
mod tests;
