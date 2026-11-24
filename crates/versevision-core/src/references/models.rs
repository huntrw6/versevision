//! `models.py` and `bibles/models.py` (the parts that are plain data): the shapes the oracle dumps.

use serde::{Deserialize, Serialize};

use super::ReferenceError;

/// A chapter and a verse, both at least 1 (pydantic `Field(gt=0)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct VersePoint {
    pub chapter: u64,
    pub verse: u64,
}

impl VersePoint {
    /// Python builds a `VersePoint` with validation: a zero chapter or verse raises `ValidationError`.
    pub fn new(chapter: u64, verse: u64) -> Result<Self, ReferenceError> {
        if chapter == 0 {
            return Err(ReferenceError::NotPositive { field: "chapter" });
        }
        if verse == 0 {
            return Err(ReferenceError::NotPositive { field: "verse" });
        }
        Ok(Self { chapter, verse })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VerseSpan {
    pub start: VersePoint,
    pub end: Option<VersePoint>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PassageReference {
    pub book_id: String,
    pub book_display_name: String,
    pub spans: Vec<VerseSpan>,
    pub version: Option<String>,
    pub source_text: String,
    pub source_start: usize,
    pub source_end: usize,
    pub normalized_text: String,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParseIssue {
    pub source_text: String,
    pub source_start: usize,
    pub source_end: usize,
    pub message: String,
    pub suggestion: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParseResult {
    pub references: Vec<PassageReference>,
    pub issues: Vec<ParseIssue>,
}

/// One verse as a Bible provider returns it (`bibles/models.py`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BibleVerse {
    pub book_id: String,
    pub chapter: u64,
    pub verse: u64,
    pub text: String,
    pub version: String,
    #[serde(default)]
    pub paragraph_start: bool,
    #[serde(default)]
    pub poetry_indent: i64,
    #[serde(default)]
    pub copyright: Option<String>,
    #[serde(default)]
    pub attribution: Option<String>,
}

impl BibleVerse {
    /// A verse with only the required fields (the others at their Python defaults).
    pub fn new(book_id: &str, chapter: u64, verse: u64, text: &str, version: &str) -> Self {
        Self {
            book_id: book_id.to_string(),
            chapter,
            verse,
            text: text.to_string(),
            version: version.to_string(),
            paragraph_start: false,
            poetry_indent: 0,
            copyright: None,
            attribution: None,
        }
    }

    /// `BibleVerse.key`: `EPH.2.9`.
    pub fn key(&self) -> String {
        format!("{}.{}.{}", self.book_id, self.chapter, self.verse)
    }
}

/// What a provider returns for one reference (`bibles/models.py`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedPassage {
    pub reference: PassageReference,
    pub resolved_version: String,
    pub verses: Vec<BibleVerse>,
}
