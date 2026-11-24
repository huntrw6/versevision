//! `title.py`: name a new presentation after the document it came from.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use super::pyre::PyRegex;
use crate::pystr;
use crate::references::parse_references_with;
use crate::references::pytext::casefold;

pub const MAX_NAME: usize = 100;

struct Patterns {
    email_header: PyRegex,
    date: PyRegex,
    not_a_title: PyRegex,
    illegal: PyRegex,
    whitespace_run: PyRegex,
    spaced_dashes: PyRegex,
    underscores: PyRegex,
    trailing_number: PyRegex,
}

fn patterns() -> &'static Patterns {
    static PATTERNS: OnceLock<Patterns> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        let build = |pattern: &str, ignore_case: bool| {
            PyRegex::new(pattern, ignore_case)
                .unwrap_or_else(|e| panic!("built-in pattern {pattern:?}: {e}"))
        };
        Patterns {
            email_header: build(
                r"^\s*(from|to|cc|bcc|subject|date|sent)\s*:|<[^>\s]+@[^>\s]+>|@|^\[quoted text hidden\]$",
                true,
            ),
            date: build(r"\d{1,2}/\d{1,2}/\d{2,4}", false),
            not_a_title: build(
                r"^\s*(\[\s*slide\s*\]|#\s*\d|\d{1,3}\s*[).:]|[a-zA-Z]\s*[).]\s|[-•▪◦●*]\s|show\b|play\b|start\b|when i\b|note\b|todo\b|recap\b|https?://)",
                true,
            ),
            illegal: build(r#"[\\/:*?"<>|\x00-\x1f]"#, false),
            whitespace_run: build(r"\s+", false),
            spaced_dashes: build(r"\s+-\s+-\s+", false),
            underscores: build(r"[_]+", false),
            trailing_number: build(r"\s+\d+$", false),
        }
    })
}

/// `pattern.sub(replacement, text)` for the few patterns here (every non-overlapping match, left to right).
fn sub(pattern: &PyRegex, replacement: &str, text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut at = 0usize;
    while at <= chars.len() {
        let rest: String = chars[at..].iter().collect();
        let Some(found) = pattern.search(&rest) else {
            break;
        };
        let (start, end) = found.span();
        out.extend(&chars[at..at + start]);
        out.push_str(replacement);
        if end == start {
            if at + end < chars.len() {
                out.push(chars[at + end]);
            }
            at += end + 1;
        } else {
            at += end;
        }
    }
    if at <= chars.len() {
        out.extend(&chars[at.min(chars.len())..]);
    }
    out
}

fn plausible(line: &str) -> Result<bool, crate::references::ReferenceError> {
    let line = pystr::strip(line);
    let length = line.chars().count();
    if !(3..=100).contains(&length) || [".", "?", "!", "”", "\""].iter().any(|e| line.ends_with(e))
    {
        return Ok(false);
    }
    if pystr::split_ws(line).len() > 14 || ["“", "\"", "‘"].iter().any(|q| line.starts_with(q))
    {
        return Ok(false);
    }
    let p = patterns();
    if p.not_a_title.is_match(line) {
        return Ok(false);
    }
    if p.date.is_match(line) {
        return Ok(true);
    }
    let references = parse_references_with(line, None)?.references;
    Ok(!references.first().is_some_and(|r| r.source_start <= 2))
}

/// A name that is safe as a file name on macOS, Windows and Linux (and shows well in ProPresenter).
pub fn safe_presentation_name(title: &str, fallback: &str) -> String {
    let p = patterns();
    let name = title
        .replace('\u{a0}', " ")
        .replace(['/', '\\'], "-")
        .replace(':', " -");
    let name = sub(&p.illegal, "", &name);
    let name = sub(&p.whitespace_run, " ", &name);
    let strip_marks = |text: &str| -> String { text.trim_matches([' ', '.', '-']).to_string() };
    let name = strip_marks(&name);
    let mut name = sub(&p.spaced_dashes, " - ", &name);
    if name.chars().count() > MAX_NAME {
        let cut: String = name.chars().take(MAX_NAME).collect();
        let head = match cut.rsplit_once(' ') {
            Some((head, _)) => head.to_string(),
            None => cut.clone(),
        };
        let shortened = strip_marks(&head);
        name = if shortened.is_empty() { cut } else { shortened };
    }
    if name.is_empty() {
        fallback.to_string()
    } else {
        name
    }
}

/// `Path(filename).stem`, with underscores and runs of spaces turned into single spaces.
pub fn title_from_filename(filename: &str) -> String {
    if filename.is_empty() || filename == "text" {
        return String::new();
    }
    let parts: Vec<&str> = filename
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    let name = parts.last().copied().unwrap_or("");
    let stem = match name.rfind('.') {
        Some(i) if 0 < i && i < name.len() - 1 => &name[..i],
        _ => name,
    };
    let p = patterns();
    let stem = sub(&p.underscores, " ", stem);
    let stem = sub(&p.whitespace_run, " ", &stem);
    pystr::strip(&stem).to_string()
}

/// The first line of the document when it reads like a title, otherwise the file name.
pub fn document_title(
    text: &str,
    filename: &str,
) -> Result<String, crate::references::ReferenceError> {
    let normalized = text.replace('\u{a0}', " ");
    for raw in pystr::splitlines(&normalized) {
        let line = pystr::strip(raw);
        if line.is_empty() {
            continue;
        }
        if patterns().email_header.is_match(line) {
            continue; // skip email From/To/Cc lines and keep looking
        }
        if plausible(line)? {
            return Ok(safe_presentation_name(line, "Sermon Notes"));
        }
        break; // the first real line is not a title: use the file name
    }
    let from_file = title_from_filename(filename);
    let name = if from_file.is_empty() {
        "Sermon Notes".to_string()
    } else {
        from_file
    };
    Ok(safe_presentation_name(&name, "Sermon Notes"))
}

/// 'Name', then 'Name (2)', 'Name (3)'... so an existing presentation is never overwritten.
pub fn unique_name(name: &str, existing: &BTreeSet<String>) -> String {
    let taken: BTreeSet<String> = existing.iter().map(|item| casefold(item)).collect();
    if !taken.contains(&casefold(name)) {
        return name.to_string();
    }
    let mut counter = 2;
    while taken.contains(&casefold(&format!("{name} ({counter})"))) {
        counter += 1;
    }
    format!("{name} ({counter})")
}

/// 'Name' -> 'Name 2', 'Name 3'... (the number that follows the highest one already used).
pub fn numbered_name(name: &str, existing: &BTreeSet<String>) -> String {
    let base = sub(&patterns().trailing_number, "", name);
    let taken: BTreeSet<String> = existing.iter().map(|item| casefold(item)).collect();
    let mut counter = 2;
    while taken.contains(&casefold(&format!("{base} {counter}"))) {
        counter += 1;
    }
    format!("{base} {counter}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    // tests/test_pipeline.py::test_document_title_rules
    #[test]
    fn document_title_rules() {
        let title = |text: &str, file: &str| document_title(text, file).unwrap();
        assert_eq!(
            title("JPL 8/23/26 Summer in the Psalms\nJohn 3:16 NIV", "x.pdf"),
            "JPL 8-23-26 Summer in the Psalms"
        );
        assert_eq!(
            title(
                "Illuminary <a@b.com>\nSunday Scripture List\nJohn 3:16",
                "x.pdf"
            ),
            "Sunday Scripture List"
        );
        assert_eq!(
            title("John 3:16 NIV\n1) A point", "Easter_Notes.pdf"),
            "Easter Notes"
        );
        assert_eq!(title("“A quote.”\nJohn 3:16", ""), "Sermon Notes");
    }

    // tests/test_pipeline.py: names
    #[test]
    fn names() {
        assert_eq!(
            safe_presentation_name("A/B: C*D?", "Sermon Notes"),
            "A-B - CD"
        );
        assert_eq!(unique_name("Verses", &set(&["verses"])), "Verses (2)");
        assert_eq!(unique_name("X", &set(&["x", "x (2)"])), "X (3)");
        assert!(
            safe_presentation_name(&"word ".repeat(80), "Sermon Notes")
                .chars()
                .count()
                <= 100
        );
        assert_eq!(numbered_name("Notes", &set(&["Notes 2"])), "Notes 3");
        assert_eq!(numbered_name("Notes 7", &set(&[])), "Notes 2");
    }

    #[test]
    fn file_names() {
        assert_eq!(
            title_from_filename("a/b/Easter__Notes.final.pdf"),
            "Easter Notes.final"
        );
        assert_eq!(title_from_filename("text"), "");
        assert_eq!(title_from_filename(".hidden"), ".hidden");
        assert_eq!(title_from_filename(""), "");
    }
}
