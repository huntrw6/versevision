//! `slides/planner.py`: passages -> slides (the text of each verse cut to fit, the reference label of each slide), and the
//! placeholder passages a plan without Bible text uses. The shapes (`PlannedSlide`, `SlidePlan`) are in [`crate::plan`].

use crate::config::{ReferenceLabelMode, SlideConfig};
use crate::fitting::split_verse_text;
use crate::plan::{PlannedSlide, SlideKind, SlidePlan};
use crate::pystr::rstrip;
use crate::references::models::{BibleVerse, ResolvedPassage};
use crate::references::{PassageReference, VerseSpan};

/// `_span_label`: `Book 3:16`, `Book 3:16-18`, `Book 3:16-4:2`.
fn span_label(reference: &PassageReference, span: &VerseSpan) -> String {
    let mut value = format!(
        "{} {}:{}",
        reference.book_display_name, span.start.chapter, span.start.verse
    );
    if let Some(end) = span.end {
        let suffix = if end.chapter == span.start.chapter {
            end.verse.to_string()
        } else {
            format!("{}:{}", end.chapter, end.verse)
        };
        value.push('-');
        value.push_str(&suffix);
    }
    value
}

/// `_verse_reference`: the reference shown on the slide of one verse, by `slides.reference_label_mode`.
pub fn verse_reference(
    reference: &PassageReference,
    verse: &BibleVerse,
    mode: ReferenceLabelMode,
) -> String {
    match mode {
        ReferenceLabelMode::Source => {
            let mut base = reference.normalized_text.clone();
            if let Some(version) = reference.version.as_deref().filter(|v| !v.is_empty()) {
                if let Some(stripped) = base.strip_suffix(&format!(" {version}")) {
                    base = stripped.to_string();
                }
            }
            return base;
        }
        ReferenceLabelMode::Segment => {
            for span in &reference.spans {
                let end = span.end.unwrap_or(span.start);
                let here = (verse.chapter, verse.verse);
                if (span.start.chapter, span.start.verse) <= here
                    && here <= (end.chapter, end.verse)
                {
                    return span_label(reference, span);
                }
            }
        }
        ReferenceLabelMode::Verse => {}
    }
    format!(
        "{} {}:{}",
        reference.book_display_name, verse.chapter, verse.verse
    )
}

/// Python's `a or b` on two optional texts: the first that is not empty, else `b`.
pub(crate) fn first_truthy(a: &Option<String>, b: &Option<String>) -> Option<String> {
    match a {
        Some(text) if !text.is_empty() => Some(text.clone()),
        _ => b.clone(),
    }
}

/// `plan_slides`: every verse of every passage becomes one slide, or several when it is too long.
pub fn plan_slides(passages: &[ResolvedPassage], settings: &SlideConfig) -> SlidePlan {
    let mut slides = Vec::new();
    for (passage_index, passage) in passages.iter().enumerate() {
        for verse in &passage.verses {
            let body = if settings.include_verse_numbers {
                rstrip(&format!("{} {}", verse.verse, verse.text)).to_string()
            } else {
                verse.text.clone()
            };
            let chunks = split_verse_text(
                &body,
                settings.max_body_lines as usize,
                settings.line_width_units,
                settings.prefer_punctuation_breaks,
                settings.even_split_tolerance,
                settings.even_split_punctuation_tolerance,
            );
            let reference_text =
                verse_reference(&passage.reference, verse, settings.reference_label_mode);
            for (chunk_index, chunk) in chunks.into_iter().enumerate() {
                slides.push(PlannedSlide {
                    kind: SlideKind::Verse,
                    body_text: chunk,
                    reference_text: if settings.repeat_reference_on_continuation || chunk_index == 0
                    {
                        reference_text.clone()
                    } else {
                        String::new()
                    },
                    version_text: passage.resolved_version.clone(),
                    copyright_text: first_truthy(&verse.copyright, &verse.attribution),
                    source_passage_index: passage_index,
                    verse_keys: vec![verse.key()],
                });
            }
        }
    }
    SlidePlan {
        slides,
        attribution_slides: Vec::new(),
        looks: None,
    }
}

/// `placeholder_passages`: the verses of each reference with empty text, for planning without Bible text. A span across chapters
/// is only its two ends (the verse counts in between need a provider).
pub fn placeholder_passages(references: &[PassageReference]) -> Vec<ResolvedPassage> {
    references
        .iter()
        .map(|reference| {
            let version = reference.version.clone().unwrap_or_default();
            let blank = |chapter: u64, verse: u64| {
                BibleVerse::new(&reference.book_id, chapter, verse, "", &version)
            };
            let mut verses = Vec::new();
            for span in &reference.spans {
                let end = span.end.unwrap_or(span.start);
                if end.chapter == span.start.chapter {
                    verses.extend(
                        (span.start.verse..=end.verse).map(|v| blank(span.start.chapter, v)),
                    );
                } else {
                    verses.push(blank(span.start.chapter, span.start.verse));
                    verses.push(blank(end.chapter, end.verse));
                }
            }
            ResolvedPassage {
                reference: reference.clone(),
                resolved_version: version,
                verses,
            }
        })
        .collect()
}
