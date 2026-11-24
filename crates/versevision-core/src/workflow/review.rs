//! `apply_review_choices`: the slides after the choices made in the review.

use serde_json::json;

use crate::notes::trace::emit;
use crate::notes::TraceSink;
use crate::plan::{NotesImportResult, PlannedSlide, ReviewChoices, SlideKind, SlidePlan};
use crate::trace_fields;

/// The slides after your choices in the review. `choices.skip`: decisions whose slides you switched off. `choices.include`: left-out
/// lines you chose to add as a slide; each goes where the line stood in the document (`\f` in `add_text` separates slides).
/// `choices.reject_fixes`: an automatic fix you did not approve, so the point goes in as you wrote it. Ids are the decision ids of
/// `result.decisions`. Writes `review_fix_rejected` and `review_included` trace records.
pub fn apply_review_choices(
    result: &NotesImportResult,
    choices: &ReviewChoices,
    trace: &mut dyn TraceSink,
) -> SlidePlan {
    let mut slides = result.slide_plan.slides.clone();
    for d in &result.decisions {
        if !choices.reject_fixes.contains(&d.id) {
            continue;
        }
        let Some(correction) = &d.correction else {
            continue;
        };
        for (offset, original) in correction.original_slides.iter().enumerate() {
            let index = d.first_slide + offset;
            if index < slides.len() && offset < d.slide_count {
                slides[index].body_text = original.clone();
            }
        }
        emit(
            trace,
            "review_fix_rejected",
            trace_fields! { "id" => d.id, "line" => d.line, "kept" => correction.from, "instead_of" => correction.to },
        );
    }
    let mut drop = std::collections::BTreeSet::new();
    for d in &result.decisions {
        if choices.skip.contains(&d.id) {
            drop.extend(d.first_slide..d.first_slide + d.slide_count);
        }
    }
    // insert position -> slides, positions in first-use order does not matter: each position is read by its index
    let mut inserts: std::collections::BTreeMap<usize, Vec<PlannedSlide>> = Default::default();
    let mut added = Vec::new();
    for d in &result.decisions {
        if choices.include.contains(&d.id) && d.slide_count == 0 && !d.add_text.is_empty() {
            let at = d.first_slide.min(slides.len());
            for part in d.add_text.split('\u{c}') {
                inserts.entry(at).or_default().push(PlannedSlide {
                    kind: SlideKind::Point,
                    body_text: part.to_string(),
                    reference_text: String::new(),
                    version_text: String::new(),
                    copyright_text: None,
                    source_passage_index: 100_000 + d.id,
                    verse_keys: Vec::new(),
                });
            }
            added.push(json!({"id": d.id, "line": d.line, "text": d.add_text, "at_slide": at + 1}));
        }
    }
    let mut out = Vec::new();
    for index in 0..=slides.len() {
        if let Some(extra) = inserts.remove(&index) {
            out.extend(extra);
        }
        if index < slides.len() && !drop.contains(&index) {
            out.push(slides[index].clone());
        }
    }
    if !added.is_empty() {
        emit(
            trace,
            "review_included",
            trace_fields! {
                "added" => added,
                "note" => "left-out lines you chose to add as slides in the review (at the place they stood in the document)",
            },
        );
    }
    SlidePlan {
        slides: out,
        attribution_slides: Vec::new(),
        looks: result.slide_plan.looks.clone(),
    }
}
