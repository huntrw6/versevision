//! `parser.py`: references in a line of text.
//!
//! Python finds book names with `re.finditer(r"(?<![A-Za-z])(?:(?:[1-3]|I{1,3}|First|Second|Third)[ \t]*)?[A-Za-z]", ..., re.IGNORECASE)`.
//! That uses a look-behind, which the `regex` crate does not have, so [`find_candidate`] scans for it by hand (same alternatives, same
//! order, same non-overlapping search). The rest of the file follows the Python functions one for one, including the places where
//! Python leaves the cursor where it was after a failed attempt.

use indexmap::IndexMap;

use super::books::{recognize_book, Book};
use super::formatter::format_reference;
use super::models::{ParseIssue, ParseResult, PassageReference, VersePoint, VerseSpan};
use super::normalizer::normalize_fragment;
use super::pytext::{casefold, fold, is_letter_ic, is_upper};
use super::tokenizer::{tokenize, Kind, Token};
use super::validator::validate_spans;
use super::{int_value, ReferenceError};
use crate::versions::{canonical_version, correct_version, is_real_version};

struct Cursor {
    tokens: Vec<Token>,
    i: usize,
    warnings: Vec<String>,
}

impl Cursor {
    fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            i: 0,
            warnings: Vec::new(),
        }
    }

    fn peek(&self, kinds: &[Kind]) -> Option<&Token> {
        self.tokens.get(self.i).filter(|t| kinds.contains(&t.kind))
    }

    fn take(&mut self, kinds: &[Kind]) -> Option<Token> {
        let token = self.peek(kinds).cloned();
        if token.is_some() {
            self.i += 1;
        }
        token
    }

    /// `word(*values)`: a WORD whose case-folded text is one of `values`.
    fn word(&mut self, values: &[&str]) -> Option<Token> {
        let token = self.peek(&[Kind::Word])?;
        if values.contains(&casefold(&token.value).as_str()) {
            self.i += 1;
            return self.tokens.get(self.i - 1).cloned();
        }
        None
    }

    fn at(&self, index: usize) -> Option<&Token> {
        self.tokens.get(index)
    }
}

const OPEN_BRACKETS: [Kind; 2] = [Kind::Punct('('), Kind::Punct('[')];

fn version(
    cursor: &mut Cursor,
    source: &[char],
    user_aliases: &IndexMap<String, String>,
) -> Option<String> {
    let opening = cursor.take(&OPEN_BRACKETS);
    let start = cursor.i;
    let Some(token) = cursor.peek(&[Kind::Word]).cloned() else {
        if opening.is_some() {
            cursor.i -= 1;
        }
        return None;
    };
    let mut label = token.value.clone();
    let mut end_index = cursor.i + 1;
    if let Some(next) = cursor.at(end_index) {
        if next.kind == Kind::Number && next.start == token.end {
            // NASB1995, NIV11
            label.push_str(&next.value);
            end_index += 1;
        }
    }
    let label_chars = label.chars().count();
    let mut version = canonical_version(&label).map(str::to_string);
    let folded_label = casefold(&label);
    let overridden = user_aliases
        .iter()
        .find(|(k, _)| casefold(k) == folded_label)
        .map(|(_, v)| v.as_str());
    let configured = overridden
        .filter(|v| !v.is_empty())
        .and_then(canonical_version);
    if let Some(configured) = configured {
        version = Some(configured.to_string());
        cursor.warnings.push(format!(
            "version '{}' read as {} (configured alias)",
            label.to_uppercase(),
            configured
        ));
    } else if version.is_none()
        && (opening.is_some()
            || (is_upper(&label)
                && (2..=10).contains(&label_chars)
                && source
                    .get(token.start..token.start + label_chars)
                    .is_some_and(|slice| slice.iter().copied().eq(label.chars()))))
    {
        if is_real_version(&label) {
            version = Some(label.to_uppercase()); // a real translation we may not have: keep as typed
        } else if opening.is_some() {
            if let Some(fixed) = correct_version(&label, None) {
                cursor.warnings.push(format!(
                    "version '{label}' read as {fixed} (spelling corrected)"
                ));
                version = Some(fixed);
            } else {
                cursor
                    .warnings
                    .push(format!("unrecognized version label '{label}'"));
                cursor.i = start - 1;
                return None;
            }
        } else if let Some(fixed) = correct_version(&label, None) {
            cursor.warnings.push(format!(
                "version '{label}' read as {fixed} (spelling corrected)"
            ));
            version = Some(fixed);
        } else {
            cursor
                .warnings
                .push(format!("unrecognized version label '{label}'")); // ignored, but never silently
        }
    }
    let Some(version) = version else {
        if opening.is_some() {
            cursor.i -= 1;
        }
        return None;
    };
    cursor.i = end_index;
    if let Some(opening) = &opening {
        let expected = if opening.kind == Kind::Punct('(') {
            Kind::Punct(')')
        } else {
            Kind::Punct(']')
        };
        if cursor.take(&[expected]).is_none() {
            cursor.i = start - 1;
            return None;
        }
    }
    if label.to_uppercase() == "NASB" && version == "NASB2020" {
        cursor.warnings.push("NASB read as NASB2020".to_string());
    }
    Some(version)
}

fn consume_verse_suffix(cursor: &mut Cursor, number: &Token) {
    let Some(suffix) = cursor.peek(&[Kind::Word]) else {
        return;
    };
    let folded = casefold(&suffix.value);
    if suffix.start == number.end && matches!(folded.as_str(), "a" | "b" | "c") {
        cursor.i += 1;
        cursor.warnings.push(format!(
            "verse-part suffix '{folded}' detected; the provider will retrieve the full verse"
        ));
    }
}

/// `_span`: one `ch:v`, `v` or `ch` with an optional `-end` / `to end`. `None` is Python's `None` return.
fn span(
    cursor: &mut Cursor,
    inherited_chapter: Option<u64>,
) -> Result<Option<(VerseSpan, u64)>, ReferenceError> {
    let Some(first) = cursor.take(&[Kind::Number]) else {
        return Ok(None);
    };
    let (chapter, verse);
    if cursor.take(&[Kind::Punct(':')]).is_some() {
        chapter = int_value(&first.value)?;
        let Some(verse_token) = cursor.take(&[Kind::Number]) else {
            return Ok(None);
        };
        verse = int_value(&verse_token.value)?;
        consume_verse_suffix(cursor, &verse_token);
    } else if let Some(inherited) = inherited_chapter {
        chapter = inherited;
        verse = int_value(&first.value)?;
        consume_verse_suffix(cursor, &first);
    } else {
        // A bare number is a whole chapter. Represent it from verse 1 and warn later.
        chapter = int_value(&first.value)?;
        verse = 1;
    }
    finish_span(cursor, chapter, verse)
}

fn finish_span(
    cursor: &mut Cursor,
    chapter: u64,
    verse: u64,
) -> Result<Option<(VerseSpan, u64)>, ReferenceError> {
    let start = VersePoint::new(chapter, verse)?;
    if cursor.take(&[Kind::Punct('-')]).is_some() || cursor.word(&["to", "through"]).is_some() {
        let Some(end_first) = cursor.take(&[Kind::Number]) else {
            return Ok(None);
        };
        let following_is_number = cursor
            .at(cursor.i + 1)
            .is_some_and(|t| t.kind == Kind::Number);
        let (end_chapter, end_verse);
        if cursor.peek(&[Kind::Punct(':')]).is_some() && following_is_number {
            cursor.take(&[Kind::Punct(':')]);
            end_chapter = int_value(&end_first.value)?;
            let Some(end_verse_token) = cursor.take(&[Kind::Number]) else {
                return Ok(None);
            };
            end_verse = int_value(&end_verse_token.value)?;
            consume_verse_suffix(cursor, &end_verse_token);
        } else {
            end_chapter = chapter;
            end_verse = int_value(&end_first.value)?;
            consume_verse_suffix(cursor, &end_first);
        }
        let end = VersePoint::new(end_chapter, end_verse)?;
        return Ok(Some((
            VerseSpan {
                start,
                end: Some(end),
            },
            chapter,
        )));
    }
    Ok(Some((VerseSpan { start, end: None }, chapter)))
}

/// `'James 2:17, KJV'`: a comma between the reference and a version we know belongs to the reference.
fn skip_comma_before_version(cursor: &mut Cursor) {
    if cursor.peek(&[Kind::Punct(',')]).is_none() {
        return;
    }
    let Some(after) = cursor.at(cursor.i + 1) else {
        return;
    };
    if after.kind != Kind::Word {
        return;
    }
    let mut label = after.value.clone();
    if let Some(next) = cursor.at(cursor.i + 2) {
        if next.kind == Kind::Number && next.start == after.end {
            label.push_str(&next.value); // NASB1995
        }
    }
    if canonical_version(&label).is_some() || (is_upper(&label) && is_real_version(&label)) {
        cursor.i += 1;
    }
}

fn slice(chars: &[char], start: usize, end: usize) -> String {
    let end = end.min(chars.len());
    let start = start.min(end);
    chars[start..end].iter().collect()
}

#[allow(clippy::too_many_arguments)]
fn parse_after_book(
    text: &[char],
    source: &[char],
    book: &Book,
    book_start: usize,
    content_start: usize,
    aliases: &IndexMap<String, String>,
) -> Result<(Vec<PassageReference>, Vec<ParseIssue>, usize), ReferenceError> {
    let mut cursor = Cursor::new(tokenize(
        &text[content_start.min(text.len())..],
        content_start,
    ));
    let mut passages = Vec::new();
    let mut issues = Vec::new();
    // In one-chapter books, the conventional bare number is a verse (Jude 5),
    // while in multi-chapter books it denotes the whole chapter (Psalm 23).
    let mut chapter: Option<u64> = if book.chapters == 1 { Some(1) } else { None };
    let mut logical_start = book_start;
    loop {
        let warning_start = cursor.warnings.len();
        let mut spans: Vec<VerseSpan> = Vec::new();
        let Some((first, ch)) = span(&mut cursor, chapter)? else {
            break;
        };
        chapter = Some(ch);
        spans.push(first);
        while cursor.take(&[Kind::Punct('&'), Kind::Punct(',')]).is_some()
            || cursor.word(&["and"]).is_some()
        {
            let checkpoint = cursor.i;
            let Some((next, ch)) = span(&mut cursor, chapter)? else {
                cursor.i = checkpoint - 1;
                break;
            };
            chapter = Some(ch);
            spans.push(next);
        }
        skip_comma_before_version(&mut cursor);
        let version = version(&mut cursor, text, aliases);
        let end = if cursor.i > 0 {
            cursor.tokens[cursor.i - 1].end
        } else {
            content_start
        };
        let raw = slice(source, logical_start, end);
        let mut reference = PassageReference {
            book_id: book.id.to_string(),
            book_display_name: book.name.to_string(),
            spans,
            version,
            source_text: raw.clone(),
            source_start: logical_start,
            source_end: end,
            normalized_text: String::new(),
            warnings: cursor.warnings[warning_start..].to_vec(),
        };
        let errors = validate_spans(book, &reference.spans);
        if errors.is_empty() {
            if book.chapters > 1
                && reference.spans.len() == 1
                && reference.spans[0].start.verse == 1
                && !raw.contains(':')
            {
                reference.warnings.push(
                    "whole chapter reference; provider must expand chapter verses".to_string(),
                );
            }
            reference.normalized_text = format_reference(&reference);
            passages.push(reference);
        } else {
            issues.push(ParseIssue {
                source_text: raw,
                source_start: logical_start,
                source_end: end,
                message: errors.join("; "),
                suggestion: None,
            });
        }
        if cursor.take(&[Kind::Punct(';')]).is_none() {
            break;
        }
        logical_start = cursor.at(cursor.i).map_or(end, |t| t.start);
        // A named book after a semicolon is handled by the outer scanner.
        if cursor.peek(&[Kind::Word]).is_some() {
            break;
        }
    }
    let consumed = if cursor.i > 0 {
        cursor.tokens[cursor.i - 1].end
    } else {
        content_start
    };
    Ok((passages, issues, consumed))
}

/// The `[ \t]*` after a book number or word.
fn skip_blanks(text: &[char], mut p: usize) -> usize {
    while p < text.len() && (text[p] == ' ' || text[p] == '\t') {
        p += 1;
    }
    p
}

/// One match of `(?:(?:[1-3]|I{1,3}|First|Second|Third)[ \t]*)?[A-Za-z]` (ignoring case) at `pos`: where it ends.
fn match_at(text: &[char], pos: usize) -> Option<usize> {
    let n = text.len();
    let letter_at = |p: usize| p < n && is_letter_ic(text[p]);
    // the optional prefix, alternatives in the regex's order; each is followed by blanks and a letter
    if matches!(text[pos], '1'..='3') {
        let p = skip_blanks(text, pos + 1);
        if letter_at(p) {
            return Some(p + 1);
        }
    }
    let roman = text[pos..]
        .iter()
        .take(3)
        .take_while(|&&c| fold(c) == 'i')
        .count();
    for count in (1..=roman).rev() {
        let p = skip_blanks(text, pos + count);
        if letter_at(p) {
            return Some(p + 1);
        }
    }
    for word in ["first", "second", "third"] {
        let len = word.len();
        if pos + len <= n
            && text[pos..pos + len]
                .iter()
                .zip(word.chars())
                .all(|(&c, w)| fold(c) == w)
        {
            let p = skip_blanks(text, pos + len);
            if letter_at(p) {
                return Some(p + 1);
            }
        }
    }
    letter_at(pos).then_some(pos + 1)
}

/// The next match at or after `from` (`finditer`): not directly after a letter.
fn find_candidate(text: &[char], from: usize) -> Option<(usize, usize)> {
    (from..text.len()).find_map(|pos| {
        if pos > 0 && is_letter_ic(text[pos - 1]) {
            return None;
        }
        match_at(text, pos).map(|end| (pos, end))
    })
}

/// `DEFAULT_USER_ALIASES`: operator overrides applied when the caller passes none.
pub fn default_user_aliases() -> IndexMap<String, String> {
    IndexMap::from([("YLT".to_string(), "NLT".to_string())])
}

/// `parse_references(source, aliases)`. `aliases` is `None` for the default (`{"YLT": "NLT"}`); an empty map means none.
pub fn parse_references_with(
    source: &str,
    aliases: Option<&IndexMap<String, String>>,
) -> Result<ParseResult, ReferenceError> {
    let defaults;
    let aliases = match aliases {
        Some(aliases) => aliases,
        None => {
            defaults = default_user_aliases();
            &defaults
        }
    };
    let normalized: Vec<char> = normalize_fragment(source).chars().collect();
    let source_chars: Vec<char> = source.chars().collect();
    let mut result = ParseResult::default();
    let mut occupied_until = 0;
    let mut search_from = 0;
    while let Some((start, end)) = find_candidate(&normalized, search_from) {
        search_from = end;
        if start < occupied_until {
            continue;
        }
        let Some((book, after)) = recognize_book(&normalized, start) else {
            continue;
        };
        let (references, issues, consumed) =
            parse_after_book(&normalized, &source_chars, book, start, after, aliases)?;
        if !references.is_empty() || !issues.is_empty() {
            result.references.extend(references);
            result.issues.extend(issues);
            occupied_until = consumed;
        }
    }
    Ok(result)
}
