//! Scripture references: the book table, the tokenizer and parser, versification and validation, and the reference formatter.
//!
//! A port of `python/src/versevision/references/` (and the data shapes of `bibles/models.py`). The output has to be
//! byte-identical to `oracle/references.jsonl`; see `parser.rs` for the one regex Python uses that the `regex` crate cannot
//! express, and `pytext.rs` for the Python text semantics (`isalpha`, `\d`, `IGNORECASE`, `casefold`) the scanner relies on.
//!
//! Where Rust differs from Python on purpose:
//! * A chapter or verse number is a `u64`. Python's `int` is unbounded; a number of more than 19 digits next to a book name is
//!   clamped to `u64::MAX` here, so it is still reported as a chapter/verse that does not exist, but the message shows
//!   18446744073709551615 instead of the digits that were written.
//! * Python raises pydantic `ValidationError` for chapter or verse 0 (`John 0:1`): [`ReferenceError::NotPositive`], and the
//!   whole call fails, exactly as the oracle records.

mod book_data;
pub mod books;
mod formatter;
pub mod models;
mod normalizer;
mod parser;
pub mod pytext;
mod tokenizer;
mod unitables;
mod validator;
mod verse_counts;
pub mod versification;

use serde_json::{Map, Value};
use thiserror::Error;

pub use books::{book_by_id, books, recognize_book, Book};
pub use formatter::format_reference;
pub use models::{
    BibleVerse, ParseIssue, ParseResult, PassageReference, ResolvedPassage, VersePoint, VerseSpan,
};
pub use normalizer::normalize_fragment;
pub use parser::{default_user_aliases, parse_references_with};
pub use pytext::{casefold, decimal_value};
pub use validator::validate_spans;
pub use versification::{expand_span, verse_counts};

/// What makes the Python reference parser raise instead of returning issues.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ReferenceError {
    /// pydantic `ValidationError`: `VersePoint(chapter=0, ...)` / `verse=0` (`field` is the one that failed).
    #[error("1 validation error for VersePoint\n{field}\n  Input should be greater than 0 [type=greater_than]")]
    NotPositive { field: &'static str },
    /// Python `ValueError` from `int()`: a "digit" that is not a decimal digit (`①`).
    #[error("invalid literal for int() with base 10: {0:?}")]
    NotADecimal(String),
}

impl ReferenceError {
    /// The name of the exception Python raises for the same input (the `raises` field of `oracle/references.jsonl`).
    pub fn python_exception(&self) -> &'static str {
        match self {
            Self::NotPositive { .. } => "ValidationError",
            Self::NotADecimal(_) => "ValueError",
        }
    }
}

/// `int(text)` for a token made of decimal digits of any script (saturating at `u64::MAX`, see the module notes).
pub(crate) fn int_value(text: &str) -> Result<u64, ReferenceError> {
    let mut value: u64 = 0;
    for c in text.chars() {
        let digit = pytext::decimal_value(c)
            .ok_or_else(|| ReferenceError::NotADecimal(text.to_string()))?;
        value = value.saturating_mul(10).saturating_add(u64::from(digit));
    }
    Ok(value)
}

/// `parse_references(line, aliases)` as `oracle/references.jsonl` records it (`{"references": [...], "issues": [...]}`).
/// `aliases` is `bible.version_aliases`; `None` means Python's default (`{"YLT": "NLT"}`).
pub fn parse_references(
    line: &str,
    aliases: Option<&Map<String, Value>>,
) -> Result<Value, ReferenceError> {
    let aliases: Option<indexmap::IndexMap<String, String>> = aliases.map(|map| {
        map.iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
            .collect()
    });
    let result = parse_references_with(line, aliases.as_ref())?;
    Ok(serde_json::to_value(result).expect("a parse result is plain data"))
}

#[cfg(test)]
mod tests;
