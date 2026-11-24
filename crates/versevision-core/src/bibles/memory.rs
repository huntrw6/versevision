//! The offline provider of the oracle (`OracleBible` of `python/scripts/export_oracle.py`, `docs/PLAN_CONTRACT.md` section 1):
//! it serves a case's `bible.json`.

use indexmap::IndexMap;
use serde_json::Value;

use super::{BibleError, BiblePassages, BibleVerse, ResolvedPassage};
use crate::notes::trace::emit;
use crate::notes::TraceSink;
use crate::references::versification::expand_span;
use crate::references::PassageReference;
use crate::trace_fields;

/// Verse text kept in memory: `"<VERSION> <Book> <chapter>:<verse>"` -> text, for example `"NKJV Psalms 23:1"` (the book is the
/// display name the parser produced).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InMemoryBible {
    pub verses: IndexMap<String, String>,
}

impl InMemoryBible {
    pub fn new(verses: IndexMap<String, String>) -> Self {
        Self { verses }
    }

    /// Read a `bible.json` (an object of text by key).
    pub fn from_json(json: &str) -> Result<Self, String> {
        match serde_json::from_str::<Value>(json).map_err(|e| e.to_string())? {
            Value::Object(map) => map
                .into_iter()
                .map(|(key, value)| match value {
                    Value::String(text) => Ok((key, text)),
                    other => Err(format!("{key}: not a string: {other}")),
                })
                .collect::<Result<IndexMap<_, _>, _>>()
                .map(Self::new),
            other => Err(format!("not an object: {other}")),
        }
    }

    /// The verse key of the table, the way Python writes it: an f-string, so a missing version reads `None`.
    fn key(reference: &PassageReference, chapter: u64, verse: u64) -> String {
        format!(
            "{} {} {chapter}:{verse}",
            reference.version.as_deref().unwrap_or("None"),
            reference.book_display_name
        )
    }

    fn passage(&self, reference: &PassageReference) -> Result<ResolvedPassage, BibleError> {
        let version = reference.version.clone().unwrap_or_default();
        let mut verses = Vec::new();
        for span in &reference.spans {
            for (chapter, number) in expand_span(&reference.book_id, span) {
                let key = Self::key(reference, chapter, number);
                let Some(text) = self.verses.get(&key) else {
                    return Err(BibleError::Provider(format!("no verse text for {key}")));
                };
                verses.push(BibleVerse::new(
                    &reference.book_id,
                    chapter,
                    number,
                    text,
                    &version,
                ));
            }
        }
        Ok(ResolvedPassage {
            reference: reference.clone(),
            resolved_version: version,
            verses,
        })
    }
}

impl BiblePassages for InMemoryBible {
    async fn fetch(
        &self,
        reference: &PassageReference,
        trace: &mut dyn TraceSink,
    ) -> Result<ResolvedPassage, BibleError> {
        let mut info = trace_fields! {
            "reference" => reference.normalized_text,
            "version" => reference.version,
        };
        match self.passage(reference) {
            Ok(passage) => {
                let mut fields =
                    trace_fields! { "provider" => "oracle", "source" => "bible.json", "ms" => 0 };
                fields.append(&mut info);
                emit(trace, "provider", fields);
                Ok(passage)
            }
            Err(error) => {
                let mut fields = trace_fields! { "error" => error.message(), "ms" => 0 };
                fields.append(&mut info);
                emit(trace, "provider", fields);
                Err(error)
            }
        }
    }

    fn description(&self) -> String {
        format!("bible.json (offline, {} verses)", self.verses.len())
    }
}
