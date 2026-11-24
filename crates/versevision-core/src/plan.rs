//! The plan: what the notes planner returns, the review report built from it and the trace of how it got there.
//!
//! The types serialise to exactly the JSON of `oracle/cases/*/expected/*` (field names are the Python model's;
//! `docs/PLAN_CONTRACT.md` is the contract). The planner itself is [`crate::workflow::plan_notes`].

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

use crate::looks::Looks;
use crate::notes::ClassifyError;

/// What makes `plan_notes` raise where Python raises (everything else, a verse that cannot be had, is a decision in the plan).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PlanError {
    /// `classify_document` raised (the reference parser, a bad `notes.skip_patterns` entry, the numbered-duplicate bug).
    #[error("{0}")]
    Classify(#[from] ClassifyError),
    /// `VerseVisionError("Unknown Bible version: ...")`: the default version is not a Bible version.
    #[error("Unknown Bible version: {0}")]
    UnknownVersion(String),
    /// Python builds the real provider registry from `bible.providers` when none is given; here the caller has to supply the
    /// Bible text (S5 brings the real providers), unless the plan is made without it.
    #[error("no Bible text source was given (pass one, or plan with no_fetch)")]
    NoBibleSource,
    /// `points.number_style` is not a `str.format` pattern with `{n}` (Python raises `KeyError`, `IndexError` or `ValueError`).
    #[error("points.number_style {style:?} cannot be formatted: {why}")]
    NumberStyle { style: String, why: String },
}

/// What kind of slide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlideKind {
    Verse,
    Point,
}

/// What a line of the notes turned into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    Verse,
    Point,
    LeftOut,
    Error,
}

/// How sure the planner is that the decision is right.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Good,
    Warning,
    Error,
}

/// One slide.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannedSlide {
    pub kind: SlideKind,
    /// `\n` between the lines of one slide; the line breaks are part of the contract.
    pub body_text: String,
    pub reference_text: String,
    pub version_text: String,
    pub copyright_text: Option<String>,
    /// 0-based: one number per verse or point entry of the document, in order.
    pub source_passage_index: usize,
    /// `BOOKID.chapter.verse`; empty for a point.
    pub verse_keys: Vec<String>,
}

/// All slides, in show order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SlidePlan {
    pub slides: Vec<PlannedSlide>,
    pub attribution_slides: Vec<PlannedSlide>,
    /// The theme slides chosen for verses and points (not part of the saved plan; the ProPresenter theme reader is S6).
    #[serde(skip)]
    pub looks: Option<Looks>,
}

/// An automatic fix the user has to approve (a point header written unlike the others).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Correction {
    pub from: String,
    pub to: String,
    pub original_slides: Vec<String>,
}

/// `{}` in the plan means "no correction"; this reads and writes that.
mod empty_object_is_none {
    use super::Correction;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(
        value: &Option<Correction>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(correction) => correction.serialize(serializer),
            None => serde_json::Map::new().serialize(serializer),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Correction>, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        match &value {
            serde_json::Value::Object(map) if map.is_empty() => Ok(None),
            serde_json::Value::Null => Ok(None),
            _ => serde_json::from_value(value)
                .map(Some)
                .map_err(serde::de::Error::custom),
        }
    }
}

/// What the planner decided about one verse, point or left-out line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    /// 0.. in document order.
    pub id: usize,
    /// 1-based line of the document after reading; 0 for an invalid reference found outside any line.
    pub line: usize,
    pub kind: DecisionKind,
    pub title: String,
    pub text: String,
    pub tier: Tier,
    /// Exact prose, in order, without duplicates.
    pub reasons: Vec<String>,
    /// 0-based index in `slide_plan.slides`.
    pub first_slide: usize,
    /// 0 = no slide.
    pub slide_count: usize,
    #[serde(with = "empty_object_is_none")]
    pub correction: Option<Correction>,
    /// For a left-out line: the slide text "add this as a slide" would make (`\f` separates slides).
    pub add_text: String,
}

/// A line of the notes with something to say about it (`notices`, `skipped`, `review`, `failed`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProblemEntry {
    pub line: usize,
    pub text: String,
    pub problem: String,
}

/// The planner's result (`NotesImportResult` without `target` and `files_changed`, which are always empty).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotesImportResult {
    pub fetched: bool,
    pub verse_count: usize,
    pub point_count: usize,
    pub slide_plan: SlidePlan,
    pub decisions: Vec<Decision>,
    /// Adjustments the tool made.
    pub notices: Vec<ProblemEntry>,
    /// Left out on purpose.
    pub skipped: Vec<ProblemEntry>,
    /// Need a decision.
    pub review: Vec<ProblemEntry>,
    /// Could not be added.
    pub failed: Vec<ProblemEntry>,
    /// The contract's `looks` key: always `null` (the oracle does not cover themes). The real look is `slide_plan.looks`.
    pub looks: Option<Value>,
}

/// The numbers at the top of the review page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewSummary {
    pub slides: usize,
    pub verse_slides: usize,
    pub point_slides: usize,
    pub flagged: usize,
    pub not_added: usize,
    pub needs_review: usize,
    pub left_out: usize,
    pub adjustments: usize,
    pub errors: usize,
    pub fixes: usize,
    pub warnings: usize,
    pub good: usize,
}

/// One slide as the review page lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewSlide {
    /// 1-based.
    pub n: usize,
    pub kind: SlideKind,
    /// A verse: reference + ` (version)`; a point: the first line of its text.
    pub title: String,
    pub text: String,
}

/// A correction as the review shows it (the original slides are not in the report).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewCorrection {
    pub from: String,
    pub to: String,
}

/// A decision as the review page reads it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewDecision {
    pub id: usize,
    pub line: usize,
    pub kind: DecisionKind,
    pub title: String,
    pub text: String,
    pub tier: Tier,
    pub reasons: Vec<String>,
    /// 1-based slide numbers; empty when the decision has no slide.
    pub slides: Vec<usize>,
    /// `add_text` is set and the decision has no slide.
    pub addable: bool,
    pub add_text: String,
    pub correction: Option<ReviewCorrection>,
}

/// The decisions by tier (left-out lines apart).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewGroups {
    pub error: Vec<ReviewDecision>,
    pub warning: Vec<ReviewDecision>,
    pub good: Vec<ReviewDecision>,
    pub left_out_warning: Vec<ReviewDecision>,
    pub left_out_good: Vec<ReviewDecision>,
}

/// Why a line is in the flagged / adjusted / left-out lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineStatus {
    NotAdded,
    NeedsReview,
    NeedsApproval,
    Adjusted,
    LeftOut,
}

/// A line in the flagged, adjustments or left-out lists.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewLine {
    pub line: usize,
    pub text: String,
    pub detail: String,
    pub status: LineStatus,
}

/// `build_report(plan)`: the structure the review page is built from (the web-only extras are added by the server).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewReport {
    pub source: String,
    pub summary: ReviewSummary,
    pub looks: Option<Value>,
    pub slides: Vec<ReviewSlide>,
    pub decisions: Vec<ReviewDecision>,
    pub review: ReviewGroups,
    /// Sorted by line (stable).
    pub flagged: Vec<ReviewLine>,
    pub adjustments: Vec<ReviewLine>,
    pub left_out: Vec<ReviewLine>,
    /// No flagged lines.
    pub clean: bool,
}

/// One line of `trace.jsonl`: `{"seq": 1, "stage": "extract", ...fields}`. The fields differ by stage (the table is in
/// `docs/PLAN_CONTRACT.md`), so they stay a JSON object here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceRecord {
    /// Counts from 1 with no gaps.
    pub seq: u64,
    pub stage: String,
    #[serde(flatten)]
    pub fields: Map<String, Value>,
}

/// What a run is told besides the notes and the config.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlanOptions {
    /// The version for a verse with none written and none above it (`""` or `None` = no default).
    pub default_version: Option<String>,
    /// Plan without any Bible text: the verses become empty placeholders.
    pub no_fetch: bool,
}

/// Review choices applied after planning (`choices.json`): decision ids.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ReviewChoices {
    #[serde(default)]
    pub skip: BTreeSet<usize>,
    #[serde(default)]
    pub include: BTreeSet<usize>,
    #[serde(default)]
    pub reject_fixes: BTreeSet<usize>,
}
