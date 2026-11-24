//! `validator.py`: chapter and verse bounds. The messages are Python's, word for word.
//!
//! A verse is allowed one beyond the KJV count (modern translations differ by a verse in a few places).

use super::books::Book;
use super::models::VerseSpan;
use super::versification::verse_counts;

/// The few bounds Python keeps for a book `VERSE_COUNTS` does not have (never used while the table covers all 66 books).
const KNOWN_VERSE_COUNTS: &[(&str, u64, u64)] = &[
    ("JHN", 1, 51),
    ("JHN", 3, 36),
    ("JHN", 4, 54),
    ("ROM", 8, 39),
    ("PSA", 23, 6),
    ("1JN", 4, 21),
    ("2CO", 5, 21),
    ("SNG", 2, 17),
    ("JUD", 1, 25),
];

pub fn validate_spans(book: &Book, spans: &[VerseSpan]) -> Vec<String> {
    let mut errors = Vec::new();
    let mut previous_end: Option<(u64, u64)> = None;
    for span in spans {
        let end = span.end.unwrap_or(span.start);
        let mut points = vec![span.start];
        if span.end.is_some() {
            points.push(end);
        }
        for point in points {
            if point.chapter > u64::from(book.chapters) {
                errors.push(format!("{} has no chapter {}", book.name, point.chapter));
            }
            let counts = verse_counts(book.id);
            let maximum = match counts {
                Some(counts) if (1..=counts.len() as u64).contains(&point.chapter) => {
                    Some(u64::from(counts[point.chapter as usize - 1]) + 1)
                }
                _ => KNOWN_VERSE_COUNTS
                    .iter()
                    .find(|(id, chapter, _)| *id == book.id && *chapter == point.chapter)
                    .map(|&(_, _, max)| max),
            };
            if let Some(maximum) = maximum {
                if point.verse > maximum {
                    errors.push(format!(
                        "{} {} has no verse {}",
                        book.name, point.chapter, point.verse
                    ));
                }
            }
        }
        if (end.chapter, end.verse) < (span.start.chapter, span.start.verse) {
            errors.push("range end precedes range start".to_string());
        }
        let current_start = (span.start.chapter, span.start.verse);
        if previous_end.is_some_and(|previous| current_start <= previous) {
            errors.push("duplicate or overlapping spans".to_string());
        }
        previous_end = Some((end.chapter, end.verse));
    }
    errors
}
