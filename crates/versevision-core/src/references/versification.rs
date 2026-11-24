//! `versification.py`: verses per chapter in the standard English (KJV) versification.

use super::models::VerseSpan;
use super::verse_counts::VERSE_COUNTS;

/// `VERSE_COUNTS[book_id]`: the number of verses of chapter 1, 2, ...
pub fn verse_counts(book_id: &str) -> Option<&'static [u16]> {
    VERSE_COUNTS
        .iter()
        .find(|(id, _)| *id == book_id)
        .map(|(_, counts)| *counts)
}

/// The `(chapter, verse)` pairs of a span, in order. A span inside one chapter is `start..=end`; a span across chapters is the
/// rest of the first chapter, every verse of the chapters between and `1..=end` of the last (the offline provider of the
/// oracle uses exactly this). A chapter the table does not have contributes nothing, where Python would raise.
pub fn expand_span(book_id: &str, span: &VerseSpan) -> Vec<(u64, u64)> {
    let start = span.start;
    let end = span.end.unwrap_or(start);
    if end.chapter == start.chapter {
        return (start.verse..=end.verse)
            .map(|v| (start.chapter, v))
            .collect();
    }
    let counts = verse_counts(book_id).unwrap_or(&[]);
    let count = |chapter: u64| -> u64 {
        chapter
            .checked_sub(1)
            .and_then(|i| counts.get(i as usize))
            .map_or(0, |&n| u64::from(n))
    };
    let mut out: Vec<(u64, u64)> = (start.verse..=count(start.chapter))
        .map(|v| (start.chapter, v))
        .collect();
    for chapter in start.chapter + 1..end.chapter {
        out.extend((1..=count(chapter)).map(|v| (chapter, v)));
    }
    out.extend((1..=end.verse).map(|v| (end.chapter, v)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::references::models::VersePoint;

    #[test]
    fn counts() {
        assert_eq!(verse_counts("PSA").unwrap().len(), 150);
        assert_eq!(verse_counts("PSA").unwrap()[22], 6);
        assert_eq!(verse_counts("PSA").unwrap()[118], 176);
        assert_eq!(verse_counts("JUD").unwrap(), &[25]);
        assert_eq!(verse_counts("XXX"), None);
        let total: u32 = VERSE_COUNTS
            .iter()
            .flat_map(|(_, c)| c.iter())
            .map(|&n| u32::from(n))
            .sum();
        assert_eq!(total, 31102);
    }

    #[test]
    fn expands_across_chapters() {
        let p = |c, v| VersePoint {
            chapter: c,
            verse: v,
        };
        let span = VerseSpan {
            start: p(1, 30),
            end: Some(p(2, 3)),
        };
        assert_eq!(
            expand_span("GEN", &span),
            vec![(1, 30), (1, 31), (2, 1), (2, 2), (2, 3)]
        );
        let span = VerseSpan {
            start: p(3, 5),
            end: None,
        };
        assert_eq!(expand_span("GEN", &span), vec![(3, 5)]);
        let span = VerseSpan {
            start: p(23, 5),
            end: Some(p(25, 1)),
        };
        assert_eq!(expand_span("PSA", &span).len(), 2 + 10 + 1);
    }
}
