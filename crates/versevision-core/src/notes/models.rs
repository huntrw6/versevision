//! The shapes of `notes/classifier.py` (`VerseItem`, `PointItem`, `SkippedItem`, `ReviewItem`, `NoteDocument`).
//!
//! They serialise to exactly what `oracle/cases/*/expected/document.json` holds (`NoteDocument.model_dump(mode="json")`).

use serde::{Deserialize, Serialize};

use crate::references::{ParseIssue, PassageReference};

/// What a numbered line, a bullet or a plain line became.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PointRole {
    Point,
    ListItem,
    SlideNote,
}

/// Why a line was left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipCategory {
    StageNote,
    Header,
    MediaMarker,
    QuotedText,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerseItem {
    pub line: usize,
    pub reference: PassageReference,
    pub warnings: Vec<String>,
    /// The verse text the notes quote next to this reference (used to tell which Bible version it is).
    pub quoted: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PointItem {
    pub line: usize,
    pub role: PointRole,
    /// `"1)"`, `"#2:"`, `"a)"`, or `"•"`.
    pub label: String,
    pub text: String,
    pub warnings: Vec<String>,
    pub detail_end: usize,
    /// The label as written, when the importer corrected it to match the other points.
    pub label_fixed_from: String,
}

impl PointItem {
    /// `PointItem.display`.
    pub fn display(&self) -> String {
        crate::pystr::strip(&format!("{} {}", self.label, self.text)).to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkippedItem {
    pub line: usize,
    pub category: SkipCategory,
    pub text: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewItem {
    pub line: usize,
    pub text: String,
    pub reason: String,
}

/// One item of the document, in document order. `kind` is the tag in JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DocumentItem {
    Verse(VerseItem),
    Point(PointItem),
    Skipped(SkippedItem),
    Review(ReviewItem),
}

impl DocumentItem {
    /// The `kind` field of the Python model.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Verse(_) => "verse",
            Self::Point(_) => "point",
            Self::Skipped(_) => "skipped",
            Self::Review(_) => "review",
        }
    }

    pub fn line(&self) -> usize {
        match self {
            Self::Verse(i) => i.line,
            Self::Point(i) => i.line,
            Self::Skipped(i) => i.line,
            Self::Review(i) => i.line,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NoteDocument {
    pub items: Vec<DocumentItem>,
    pub issues: Vec<ParseIssue>,
}

impl NoteDocument {
    pub fn verses(&self) -> Vec<&VerseItem> {
        self.items
            .iter()
            .filter_map(|i| match i {
                DocumentItem::Verse(v) => Some(v),
                _ => None,
            })
            .collect()
    }

    pub fn points(&self) -> Vec<&PointItem> {
        self.items
            .iter()
            .filter_map(|i| match i {
                DocumentItem::Point(p) => Some(p),
                _ => None,
            })
            .collect()
    }

    pub fn skipped(&self) -> Vec<&SkippedItem> {
        self.items
            .iter()
            .filter_map(|i| match i {
                DocumentItem::Skipped(s) => Some(s),
                _ => None,
            })
            .collect()
    }

    pub fn review(&self) -> Vec<&ReviewItem> {
        self.items
            .iter()
            .filter_map(|i| match i {
                DocumentItem::Review(r) => Some(r),
                _ => None,
            })
            .collect()
    }
}
