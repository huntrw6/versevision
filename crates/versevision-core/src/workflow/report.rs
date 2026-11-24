//! `workflow/report.py`: the plan as a plain structure for people, scripts and the review page.

use crate::plan::{
    DecisionKind, LineStatus, NotesImportResult, ProblemEntry, ReviewCorrection, ReviewDecision,
    ReviewGroups, ReviewLine, ReviewReport, ReviewSlide, ReviewSummary, SlideKind, Tier,
};

fn rows(entries: &[ProblemEntry], status: LineStatus) -> Vec<ReviewLine> {
    entries
        .iter()
        .map(|e| ReviewLine {
            line: e.line,
            text: e.text.clone(),
            detail: e.problem.clone(),
            status,
        })
        .collect()
}

/// `build_report(plan, source_name=...)`.
pub fn build_report(plan: &NotesImportResult, source_name: &str) -> ReviewReport {
    let slides: Vec<ReviewSlide> = plan
        .slide_plan
        .slides
        .iter()
        .enumerate()
        .map(|(i, slide)| {
            let title = match slide.kind {
                SlideKind::Point => slide.body_text.split('\n').next().unwrap_or("").to_string(),
                SlideKind::Verse if !slide.version_text.is_empty() => {
                    format!("{} ({})", slide.reference_text, slide.version_text)
                }
                SlideKind::Verse => slide.reference_text.clone(),
            };
            ReviewSlide {
                n: i + 1,
                kind: slide.kind,
                title,
                text: slide.body_text.clone(),
            }
        })
        .collect();
    let decisions: Vec<ReviewDecision> = plan
        .decisions
        .iter()
        .map(|d| {
            let first = if d.slide_count > 0 {
                d.first_slide + 1
            } else {
                0
            };
            ReviewDecision {
                id: d.id,
                line: d.line,
                kind: d.kind,
                title: d.title.clone(),
                text: d.text.clone(),
                tier: d.tier,
                reasons: d.reasons.clone(),
                slides: (first..first + d.slide_count).collect(),
                addable: !d.add_text.is_empty() && d.slide_count == 0,
                add_text: d.add_text.clone(),
                correction: d.correction.as_ref().map(|c| ReviewCorrection {
                    from: c.from.clone(),
                    to: c.to.clone(),
                }),
            }
        })
        .collect();
    let tier_of = |tier: Tier| -> Vec<ReviewDecision> {
        decisions
            .iter()
            .filter(|d| d.tier == tier && d.kind != DecisionKind::LeftOut)
            .cloned()
            .collect()
    };
    let left_out = |tier: Tier| -> Vec<ReviewDecision> {
        decisions
            .iter()
            .filter(|d| d.kind == DecisionKind::LeftOut && d.tier == tier)
            .cloned()
            .collect()
    };
    let review = ReviewGroups {
        error: tier_of(Tier::Error),
        warning: tier_of(Tier::Warning),
        good: tier_of(Tier::Good),
        left_out_warning: left_out(Tier::Warning),
        left_out_good: left_out(Tier::Good),
    };
    let mut flagged = rows(&plan.failed, LineStatus::NotAdded);
    flagged.extend(rows(&plan.review, LineStatus::NeedsReview));
    flagged.extend(decisions.iter().filter_map(|d| {
        d.correction.as_ref().map(|c| ReviewLine {
            line: d.line,
            text: c.from.clone(),
            detail: format!("automatic fix to approve: {} -> {}", c.from, c.to),
            status: LineStatus::NeedsApproval,
        })
    }));
    flagged.sort_by_key(|row| row.line);
    ReviewReport {
        source: source_name.to_string(),
        summary: ReviewSummary {
            slides: slides.len(),
            verse_slides: slides.iter().filter(|s| s.kind == SlideKind::Verse).count(),
            point_slides: slides.iter().filter(|s| s.kind == SlideKind::Point).count(),
            flagged: flagged.len(),
            not_added: plan.failed.len(),
            needs_review: plan.review.len(),
            left_out: plan.skipped.len(),
            adjustments: plan.notices.len(),
            errors: review.error.len(),
            fixes: decisions.iter().filter(|d| d.correction.is_some()).count(),
            warnings: review.warning.len() + review.left_out_warning.len(),
            good: review.good.len(),
        },
        looks: plan
            .slide_plan
            .looks
            .as_ref()
            .and_then(|l| serde_json::to_value(l.describe()).ok()),
        decisions,
        review,
        slides,
        clean: flagged.is_empty(),
        flagged,
        adjustments: rows(&plan.notices, LineStatus::Adjusted),
        left_out: rows(&plan.skipped, LineStatus::LeftOut),
    }
}
