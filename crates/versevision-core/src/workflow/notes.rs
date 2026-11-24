//! `plan_notes`: the notes of a document -> slides, decisions (with how sure the planner is of each) and the trace of how it got there.

use serde_json::{json, Map, Value};

use super::points::{
    added_point_text, fix_label_typos, format_point_slides, is_numbered_label, LabelFix,
};
use super::versions::{check_version_against_quote, resolve_note_versions};
use crate::bibles::{BiblePassages, ResolvedPassage};
use crate::config::{AppConfig, SlideConfig};
use crate::fitting::{text_width_units, wrap_body_lines};
use crate::notes::models::{DocumentItem, PointItem, SkipCategory, VerseItem};
use crate::notes::trace::emit;
use crate::notes::{classify_document_traced, Shortcuts, TraceSink};
use crate::plan::{
    Correction, Decision, DecisionKind, NotesImportResult, PlanError, PlanOptions, PlannedSlide,
    ProblemEntry, SlideKind, SlidePlan, Tier,
};
use crate::pystr::{flatten, strip};
use crate::references::pytext::casefold;
use crate::references::PassageReference;
use crate::slides::{first_truthy, placeholder_passages, plan_slides};
use crate::trace_fields;
use crate::versions::canonical_version;

/// The reasons of a left-out line that are firm: nothing to look at.
const FIRM_LEFT_OUT: [&str; 8] = [
    "stage/production cue",
    "title / date line",
    "recap heading",
    "document title (first line)",
    "email header line",
    "verse text quoted above its reference",
    "title / heading line",
    "[slide] tag that points",
];

/// `_left_out_tier`: a line left out for a firm reason is `good`; any other is a `warning` (look at it).
pub fn left_out_tier(reason: &str) -> Tier {
    if FIRM_LEFT_OUT.iter().any(|f| reason.contains(f)) {
        Tier::Good
    } else {
        Tier::Warning
    }
}

fn category_name(category: SkipCategory) -> &'static str {
    match category {
        SkipCategory::StageNote => "stage_note",
        SkipCategory::Header => "header",
        SkipCategory::MediaMarker => "media_marker",
        SkipCategory::QuotedText => "quoted_text",
    }
}

/// `text[:count]` in code points.
fn head(text: &str, count: usize) -> String {
    text.chars().take(count).collect()
}

fn same_verses(a: &[String], key: &str) -> bool {
    a.len() == 1 && a[0] == key
}

/// `_trace_verse`: what the Bible provider returned for one verse entry and how it became slides.
fn trace_verse(
    item: &VerseItem,
    reference: &PassageReference,
    passage: &ResolvedPassage,
    planned: &[PlannedSlide],
    slides: &SlideConfig,
    trace: &mut dyn TraceSink,
) {
    let verses = &passage.verses;
    let characters: usize = verses.iter().map(|v| v.text.chars().count()).sum();
    let first_verse = verses
        .first()
        .map(|v| head(&v.text, 100))
        .unwrap_or_default();
    let last_verse = verses
        .last()
        .map(|v| {
            let count = v.text.chars().count();
            v.text
                .chars()
                .skip(count.saturating_sub(100))
                .collect::<String>()
        })
        .unwrap_or_default();
    let copyright = verses
        .iter()
        .map(|v| first_truthy(&v.copyright, &v.attribution))
        .find(|c| c.as_deref().is_some_and(|c| !c.is_empty()))
        .flatten();
    emit(
        trace,
        "fetch_result",
        trace_fields! {
            "line" => item.line,
            "reference" => reference.normalized_text,
            "version_asked" => reference.version,
            "version_returned" => passage.resolved_version,
            "verses" => verses.len(),
            "characters" => characters,
            "first_verse" => first_verse,
            "last_verse" => last_verse,
            "copyright" => copyright,
            "note" => "which provider answered is in events.jsonl (step bible_fetch)",
        },
    );
    let per_verse: Vec<Value> = verses
        .iter()
        .map(|v| {
            let key = v.key();
            let mine: Vec<&PlannedSlide> = planned
                .iter()
                .filter(|x| same_verses(&x.verse_keys, &key))
                .collect();
            let body = if slides.include_verse_numbers {
                crate::pystr::rstrip(&format!("{} {}", v.verse, v.text)).to_string()
            } else {
                v.text.clone()
            };
            json!({
                "verse": key,
                "characters": body.chars().count(),
                "wrapped_lines": wrap_body_lines(&flatten(&body), slides.line_width_units).len(),
                "slides": mine.len(),
                "pieces": mine.iter().map(|x| x.body_text.clone()).collect::<Vec<_>>(),
            })
        })
        .collect();
    emit(
        trace,
        "split",
        trace_fields! {
            "line" => item.line,
            "reference" => reference.normalized_text,
            "slides" => planned.len(),
            "settings" => json!({
                "max_body_lines": slides.max_body_lines,
                "line_width_units": slides.line_width_units,
                "prefer_punctuation_breaks": slides.prefer_punctuation_breaks,
                "include_verse_numbers": slides.include_verse_numbers,
                "break_on_new_verse": slides.break_on_new_verse,
                "reference_label_mode": slides.reference_label_mode,
            }),
            "verses" => per_verse,
            "note" => "a verse is cut into several slides only when it needs more wrapped lines than max_body_lines; each cut prefers a punctuation break near the middle",
        },
    );
}

/// What `add(...)` of `plan_notes` needs about the document item a decision is made for.
struct Source<'a> {
    line: usize,
    /// `"point"`, `"verse"`, `"skipped"`, `"review"`.
    kind: &'a str,
}

/// The decision list while the plan is built.
struct Decisions<'a> {
    list: Vec<Decision>,
    points: &'a crate::config::PointsConfig,
}

impl Decisions<'_> {
    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        item: &Source<'_>,
        kind: DecisionKind,
        title: String,
        text: String,
        tier: Tier,
        reasons: &[String],
        first: usize,
        count: usize,
        correction: Option<Correction>,
        slides_so_far: usize,
    ) -> Result<(), PlanError> {
        let mut seen: Vec<String> = Vec::new();
        for r in reasons {
            if !r.is_empty() && !seen.contains(r) {
                seen.push(r.clone());
            }
        }
        let (mut first_slide, mut add_text) = (first, String::new());
        if count == 0
            && matches!(kind, DecisionKind::LeftOut | DecisionKind::Error)
            && matches!(item.kind, "skipped" | "review")
            && !strip(&text).is_empty()
        {
            first_slide = slides_so_far;
            add_text = added_point_text(&text, self.points)?;
        }
        self.list.push(Decision {
            id: self.list.len(),
            line: item.line,
            kind,
            title,
            text,
            tier,
            reasons: seen,
            first_slide,
            slide_count: count,
            correction,
            add_text,
        });
        Ok(())
    }
}

/// Plan a notes document (already read as text).
///
/// * `bible`: where the verse text comes from. `None` plans without any Bible text (the verses become empty placeholders), which
///   is what Python does when it is given no registry and `no_fetch` is set; with `options.no_fetch` unset a source is required.
/// * `trace`: receives every record of the run after the reader's `extract`: the classifier's, then `label_fix`, `version`,
///   `point_format`, `provider`, `quote_check`, `fetch_result`, `split`, `decision` and `plan`.
///
/// Errors are what makes Python raise (see [`PlanError`]); a verse that cannot be had is a decision of tier `error`, not an `Err`.
pub async fn plan_notes<B: BiblePassages + ?Sized>(
    source: &str,
    config: &AppConfig,
    options: &PlanOptions,
    bible: Option<&B>,
    trace: &mut dyn TraceSink,
) -> Result<NotesImportResult, PlanError> {
    if bible.is_none() && !options.no_fetch {
        return Err(PlanError::NoBibleSource);
    }
    let mut document = classify_document_traced(
        source,
        Some(&config.bible.version_aliases),
        Some(&config.notes),
        Some(&config.points),
        Shortcuts::default(),
        trace,
    )?;
    let mut result = NotesImportResult {
        fetched: !options.no_fetch,
        verse_count: 0,
        point_count: 0,
        slide_plan: SlidePlan {
            slides: Vec::new(),
            attribution_slides: Vec::new(),
            looks: None,
        },
        decisions: Vec::new(),
        notices: Vec::new(),
        skipped: Vec::new(),
        review: Vec::new(),
        failed: Vec::new(),
        looks: None,
    };
    // the fixes by line; a later fix on the same line replaces the earlier one (a dict keyed by line)
    let mut label_fixes: Vec<LabelFix> = Vec::new();
    for fix in fix_label_typos(&mut document.items, &config.points) {
        match label_fixes.iter_mut().find(|f| f.line == fix.line) {
            Some(slot) => *slot = fix,
            None => label_fixes.push(fix),
        }
    }
    for fix in &label_fixes {
        emit(
            trace,
            "label_fix",
            trace_fields! {
                "line" => fix.line,
                "written" => fix.from,
                "corrected_to" => fix.to,
                "rule" => format!("{} of {} such point headers are written \"{}\"", fix.others, fix.of, fix.to),
            },
        );
    }
    for item in document.skipped() {
        result.skipped.push(ProblemEntry {
            line: item.line,
            text: item.text.clone(),
            problem: format!("{}: {}", category_name(item.category), item.reason),
        });
    }
    for item in document.review() {
        result.review.push(ProblemEntry {
            line: item.line,
            text: item.text.clone(),
            problem: item.reason.clone(),
        });
    }
    for issue in &document.issues {
        result.failed.push(ProblemEntry {
            line: 0,
            text: issue.source_text.clone(),
            problem: format!("invalid reference: {}", issue.message),
        });
    }

    let default = options
        .default_version
        .as_deref()
        .filter(|d| !d.is_empty())
        .or(config
            .bible
            .default_version
            .as_deref()
            .filter(|d| !d.is_empty()));
    if let Some(default) = default {
        if canonical_version(default).is_none() {
            return Err(PlanError::UnknownVersion(default.to_string()));
        }
    }
    let (versions, notices) =
        resolve_note_versions(&document, default, config.verses.version_source, trace);
    result.notices = notices;
    let mut reasons_by_line: Vec<(usize, Vec<String>)> = Vec::new();
    for n in &result.notices {
        match reasons_by_line.iter_mut().find(|(line, _)| *line == n.line) {
            Some((_, reasons)) => reasons.push(n.problem.clone()),
            None => reasons_by_line.push((n.line, vec![n.problem.clone()])),
        }
    }
    let noticed = |line: usize| -> Vec<String> {
        reasons_by_line
            .iter()
            .find(|(l, _)| *l == line)
            .map(|(_, r)| r.clone())
            .unwrap_or_default()
    };

    let mut slides: Vec<PlannedSlide> = Vec::new();
    let mut decisions = Decisions {
        list: Vec::new(),
        points: &config.points,
    };
    let mut group_index = 0;
    let mut numbered_points = 0;
    let mut verse_number = 0;
    for item in &document.items {
        match item {
            DocumentItem::Point(point) => {
                let first = slides.len();
                if point.role == crate::notes::PointRole::Point
                    && !point.label.is_empty()
                    && is_numbered_label(&point.label)
                {
                    numbered_points += 1;
                }
                let parts = format_point_slides(point, &config.points, numbered_points)?;
                let shown_text = parts.join("\n\n");
                for part in &parts {
                    slides.push(PlannedSlide {
                        kind: SlideKind::Point,
                        body_text: part.clone(),
                        reference_text: String::new(),
                        version_text: String::new(),
                        copyright_text: None,
                        source_passage_index: group_index,
                        verse_keys: Vec::new(),
                    });
                }
                group_index += 1;
                result.point_count += 1;
                for warning in &point.warnings {
                    result.notices.push(ProblemEntry {
                        line: point.line,
                        text: head(&point.display(), 60),
                        problem: warning.clone(),
                    });
                }
                emit(
                    trace,
                    "point_format",
                    trace_fields! {
                        "line" => point.line,
                        "role" => point.role,
                        "label_in_notes" => point.label,
                        "text_in_notes" => point.text,
                        "slide_text" => shown_text,
                        "slides" => parts.len(),
                        "settings" => json!({
                            "numbering": config.points.numbering,
                            "case": config.points.case,
                            "bullet_char": config.points.bullet_char,
                            "number_style": config.points.number_style,
                            "max_lines_per_slide": config.points.max_lines_per_slide,
                        }),
                    },
                );
                let source = Source {
                    line: point.line,
                    kind: "point",
                };
                let fix = if point.label_fixed_from.is_empty() {
                    None
                } else {
                    label_fixes.iter().find(|f| f.line == point.line)
                };
                let title = head(shown_text.split('\n').next().unwrap_or(""), 80);
                if let Some(fix) = fix {
                    let original_item = PointItem {
                        label: point.label_fixed_from.clone(),
                        label_fixed_from: String::new(),
                        ..point.clone()
                    };
                    let original =
                        format_point_slides(&original_item, &config.points, numbered_points)?;
                    let mut reasons = vec![format!(
                        "Corrected \"{}\" to \"{}\" to match the other points ({} of {} are written that way).",
                        fix.from, fix.to, fix.others, fix.of
                    )];
                    reasons.extend(point.warnings.iter().cloned());
                    decisions.add(
                        &source,
                        DecisionKind::Point,
                        title,
                        shown_text,
                        Tier::Error,
                        &reasons,
                        first,
                        parts.len(),
                        Some(Correction {
                            from: fix.from.clone(),
                            to: fix.to.clone(),
                            original_slides: original,
                        }),
                        slides.len(),
                    )?;
                } else {
                    let tier = if point.warnings.is_empty() {
                        Tier::Good
                    } else {
                        Tier::Warning
                    };
                    decisions.add(
                        &source,
                        DecisionKind::Point,
                        title,
                        shown_text,
                        tier,
                        &point.warnings,
                        first,
                        parts.len(),
                        None,
                        slides.len(),
                    )?;
                }
            }
            DocumentItem::Verse(verse) => {
                let resolved = &versions[verse_number];
                verse_number += 1;
                let source = Source {
                    line: verse.line,
                    kind: "verse",
                };
                let line = resolved.line;
                let shown = verse.reference.normalized_text.clone();
                let noted = noticed(verse.line);
                let mut extra: Vec<String> = noted.clone();
                extra.extend(
                    verse
                        .warnings
                        .iter()
                        .filter(|w| !noted.contains(w))
                        .cloned(),
                );
                let source_text = strip(&verse.reference.source_text).to_string();
                let Some(reference) = resolved.reference.clone() else {
                    result.failed.push(ProblemEntry {
                        line,
                        text: shown.clone(),
                        problem: resolved.why.clone(),
                    });
                    let mut reasons = vec![resolved.why.clone()];
                    reasons.extend(extra);
                    decisions.add(
                        &source,
                        DecisionKind::Error,
                        shown,
                        source_text,
                        Tier::Error,
                        &reasons,
                        0,
                        0,
                        None,
                        slides.len(),
                    )?;
                    continue;
                };
                let fetched: Result<(PassageReference, ResolvedPassage), String> = async {
                    let passage = match bible {
                        Some(registry) => registry
                            .fetch(&reference, trace)
                            .await
                            .map_err(|e| e.to_string())?,
                        None => placeholder_passages(std::slice::from_ref(&reference)).remove(0),
                    };
                    if passage.verses.is_empty() {
                        return Err("provider returned no verses".to_string());
                    }
                    match bible {
                        Some(registry) if !options.no_fetch => Ok(check_version_against_quote(
                            verse,
                            reference.clone(),
                            passage,
                            registry,
                            config,
                            &mut extra,
                            trace,
                        )
                        .await),
                        _ => Ok((reference.clone(), passage)),
                    }
                }
                .await;
                let (reference, passage) = match fetched {
                    Ok(pair) => pair,
                    Err(why) => {
                        let inherited = reference.version != verse.reference.version;
                        let hint = if inherited {
                            format!(
                                " (version {} was taken from the verse above; add a version to this verse to change it)",
                                reference.version.as_deref().unwrap_or("None")
                            )
                        } else {
                            String::new()
                        };
                        let message = format!("not added: {why}{hint}");
                        result.failed.push(ProblemEntry {
                            line,
                            text: reference.normalized_text.clone(),
                            problem: message.clone(),
                        });
                        decisions.add(
                            &source,
                            DecisionKind::Error,
                            reference.normalized_text.clone(),
                            source_text,
                            Tier::Error,
                            &[message],
                            0,
                            0,
                            None,
                            slides.len(),
                        )?;
                        continue;
                    }
                };
                let first = slides.len();
                let mut planned =
                    plan_slides(std::slice::from_ref(&passage), &config.slides).slides;
                trace_verse(verse, &reference, &passage, &planned, &config.slides, trace);
                let limit = f64::from(config.slides.max_reference_lines)
                    * config.slides.reference_width_units;
                for slide in &planned {
                    let shown_ref = if slide.version_text.is_empty() {
                        slide.reference_text.clone()
                    } else {
                        format!("{} ({})", slide.reference_text, slide.version_text)
                    };
                    if text_width_units(&shown_ref) > limit {
                        let lines = config.slides.max_reference_lines;
                        extra.push(format!(
                            "the reference \"{shown_ref}\" is longer than {lines} line{} in the slide's reference box and may be cut off or shrink",
                            if lines > 1 { "s" } else { "" }
                        ));
                        break;
                    }
                }
                for slide in &mut planned {
                    slide.source_passage_index = group_index;
                }
                let count = planned.len();
                slides.extend(planned);
                group_index += 1;
                result.verse_count += 1;
                let norm = &reference.normalized_text;
                let label = match reference.version.as_deref().filter(|v| !v.is_empty()) {
                    Some(version) if !casefold(norm).contains(&casefold(version)) => {
                        format!("{norm} ({version})")
                    }
                    _ => norm.clone(),
                };
                let tier = if extra.is_empty() {
                    Tier::Good
                } else {
                    Tier::Warning
                };
                decisions.add(
                    &source,
                    DecisionKind::Verse,
                    label,
                    source_text,
                    tier,
                    &extra,
                    first,
                    count,
                    None,
                    slides.len(),
                )?;
            }
            DocumentItem::Review(review) => {
                let source = Source {
                    line: review.line,
                    kind: "review",
                };
                decisions.add(
                    &source,
                    DecisionKind::Error,
                    head(&review.text, 80),
                    review.text.clone(),
                    Tier::Error,
                    std::slice::from_ref(&review.reason),
                    0,
                    0,
                    None,
                    slides.len(),
                )?;
            }
            DocumentItem::Skipped(skipped) => {
                let source = Source {
                    line: skipped.line,
                    kind: "skipped",
                };
                let tier = left_out_tier(&skipped.reason);
                let reasons = if tier == Tier::Warning {
                    vec![format!(
                        "{}: {}",
                        category_name(skipped.category).replace('_', " "),
                        skipped.reason
                    )]
                } else {
                    vec![skipped.reason.clone()]
                };
                decisions.add(
                    &source,
                    DecisionKind::LeftOut,
                    head(&skipped.text, 80),
                    skipped.text.clone(),
                    tier,
                    &reasons,
                    0,
                    0,
                    None,
                    slides.len(),
                )?;
            }
        }
    }
    for issue in &document.issues {
        let id = decisions.list.len();
        decisions.list.push(Decision {
            id,
            line: 0,
            kind: DecisionKind::Error,
            title: head(&issue.source_text, 80),
            text: issue.source_text.clone(),
            tier: Tier::Error,
            reasons: vec![format!("invalid reference: {}", issue.message)],
            first_slide: 0,
            slide_count: 0,
            correction: None,
            add_text: String::new(),
        });
    }
    result.decisions = decisions.list;
    result.slide_plan = SlidePlan {
        slides,
        attribution_slides: Vec::new(),
        looks: None,
    };
    for d in &result.decisions {
        emit(
            trace,
            "decision",
            trace_fields! {
                "id" => d.id,
                "line" => d.line,
                "kind" => d.kind,
                "tier" => d.tier,
                "title" => d.title,
                "reasons" => d.reasons,
                "slides" => (d.first_slide..d.first_slide + d.slide_count).map(|n| n + 1).collect::<Vec<_>>(),
            },
        );
    }
    let all = &result.slide_plan.slides;
    let listed: Vec<Value> = all
        .iter()
        .enumerate()
        .map(|(i, x)| {
            let mut slide = Map::new();
            slide.insert("n".into(), json!(i + 1));
            slide.insert("kind".into(), json!(x.kind));
            slide.insert("reference".into(), json!(x.reference_text));
            slide.insert("version".into(), json!(x.version_text));
            slide.insert("text".into(), json!(x.body_text));
            slide.insert("verses".into(), json!(x.verse_keys));
            Value::Object(slide)
        })
        .collect();
    emit(
        trace,
        "plan",
        trace_fields! {
            "slide_count" => all.len(),
            "verse_slides" => all.iter().filter(|x| x.kind == SlideKind::Verse).count(),
            "point_slides" => all.iter().filter(|x| x.kind == SlideKind::Point).count(),
            "slides" => listed,
        },
    );
    Ok(result)
}
