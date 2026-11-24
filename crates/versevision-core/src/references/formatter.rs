//! `formatter.py`: the normalized text of a reference, `John 1:14-18 & 20-23 NLT`.

use super::models::{PassageReference, VersePoint};

fn point(point: VersePoint, inherited_chapter: Option<u64>) -> String {
    if inherited_chapter == Some(point.chapter) {
        point.verse.to_string()
    } else {
        format!("{}:{}", point.chapter, point.verse)
    }
}

pub fn format_reference(reference: &PassageReference) -> String {
    let mut pieces: Vec<String> = Vec::new();
    let mut chapter: Option<u64> = None;
    for span in &reference.spans {
        let mut start = point(span.start, chapter);
        if chapter.is_none() {
            start = format!("{}:{}", span.start.chapter, span.start.verse);
        }
        let end = span.end.map_or(String::new(), |end| {
            format!("-{}", point(end, Some(span.start.chapter)))
        });
        pieces.push(start + &end);
        chapter = Some(span.start.chapter);
    }
    let value = format!("{} {}", reference.book_display_name, pieces.join(" & "));
    match reference.version.as_deref() {
        Some(version) if !version.is_empty() => format!("{value} {version}"),
        _ => value,
    }
}
