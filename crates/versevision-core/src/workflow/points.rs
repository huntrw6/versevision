//! Sermon points as slides: `fix_label_typos`, `format_point`/`format_point_slides`, `added_point_text` of `notes_workflow.py`.

use crate::config::{CaseMode, Numbering, PointsConfig};
use crate::fitting::{balance_lines, split_point_text};
use crate::notes::models::{DocumentItem, PointItem, PointRole};
use crate::plan::PlanError;
use crate::pyfmt::format_n;
use crate::pystr::{is_space, rstrip, split_ws, strip};
use crate::references::pytext::{casefold, decimal_value, is_digit, is_lower, is_upper};

fn ascii_letters(chars: &[char], from: usize) -> usize {
    chars[from..]
        .iter()
        .take_while(|c| c.is_ascii_alphabetic())
        .count()
}

fn skip_space(chars: &[char], mut at: usize) -> usize {
    while at < chars.len() && is_space(chars[at]) {
        at += 1;
    }
    at
}

fn digits(chars: &[char], from: usize) -> usize {
    chars[from..]
        .iter()
        .take_while(|c| decimal_value(**c).is_some())
        .count()
}

/// `_NUMBERED.match(label)`: `1)`, `#2:`, `a)`, `KEY #3)` (the whole label, nothing else).
pub fn is_numbered_label(label: &str) -> bool {
    let chars: Vec<char> = label.chars().collect();
    // #?\d+[).:]
    let mut at = usize::from(chars.first() == Some(&'#'));
    let n = digits(&chars, at);
    at += n;
    if n > 0 && at + 1 == chars.len() && matches!(chars[at], ')' | '.' | ':') {
        return true;
    }
    // [A-Za-z][).]
    if chars.len() == 2 && chars[0].is_ascii_alphabetic() && matches!(chars[1], ')' | '.') {
        return true;
    }
    // [A-Za-z]{2,}\s*#?\s*\d+[).:\-]
    let letters = ascii_letters(&chars, 0);
    if letters >= 2 {
        let mut at = skip_space(&chars, letters);
        if chars.get(at) == Some(&'#') {
            at += 1;
        }
        at = skip_space(&chars, at);
        let n = digits(&chars, at);
        at += n;
        return n > 0 && at + 1 == chars.len() && matches!(chars[at], ')' | '.' | ':' | '-');
    }
    false
}

/// `_KEY_LABEL`: `KEY #3)` taken apart.
struct KeyLabel {
    word: String,
    s1: String,
    hash: String,
    s2: String,
    n: String,
    s3: String,
    close: char,
}

fn key_label(label: &str) -> Option<KeyLabel> {
    let chars: Vec<char> = label.chars().collect();
    let letters = ascii_letters(&chars, 0);
    if letters < 2 {
        return None;
    }
    let mut at = letters;
    let take = |from: usize, to: usize| -> String { chars[from..to].iter().collect() };
    let s1_end = skip_space(&chars, at);
    let s1 = take(at, s1_end);
    at = s1_end;
    let hash = if chars.get(at) == Some(&'#') {
        at += 1;
        "#".to_string()
    } else {
        String::new()
    };
    let s2_end = skip_space(&chars, at);
    let s2 = take(at, s2_end);
    at = s2_end;
    let run = digits(&chars, at);
    if !(1..=3).contains(&run) {
        return None;
    }
    let n = take(at, at + run);
    at += run;
    let s3_end = skip_space(&chars, at);
    let s3 = take(at, s3_end);
    at = s3_end;
    if at + 1 != chars.len() || !matches!(chars[at], ')' | '.' | ':' | '-') {
        return None;
    }
    Some(KeyLabel {
        word: take(0, letters),
        s1,
        hash,
        s2,
        n,
        s3,
        close: chars[at],
    })
}

const SMALL: [&str; 15] = [
    "a", "an", "and", "as", "at", "but", "by", "for", "in", "of", "on", "or", "the", "to", "with",
];
const KEEP_PERIOD: [&str; 14] = [
    "etc", "vs", "mr", "mrs", "ms", "dr", "st", "jr", "sr", "no", "inc", "co", "ltd", "approx",
];

/// `_title_case`: capitalise each word except the small ones in the middle; a word that is mixed case stays as written.
pub fn title_case(text: &str) -> String {
    let words: Vec<&str> = text.split(' ').collect();
    let last = words.len() - 1;
    words
        .iter()
        .enumerate()
        .map(|(i, w)| {
            if i > 0 && i < last && SMALL.contains(&w.to_lowercase().as_str()) {
                (*w).to_string()
            } else if is_lower(w) || is_upper(w) {
                let mut chars = w.chars();
                match chars.next() {
                    Some(first) => {
                        first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase()
                    }
                    None => String::new(),
                }
            } else {
                (*w).to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn case_pattern(word: &str) -> &'static str {
    let mut chars = word.chars();
    let first_upper = chars.next().is_some_and(char::is_uppercase);
    if is_upper(word) {
        "upper"
    } else if first_upper && is_lower(chars.as_str()) {
        "title"
    } else if is_lower(word) {
        "lower"
    } else {
        "as_is"
    }
}

fn apply_case(word: &str, pattern: &str) -> String {
    match pattern {
        "upper" => word.to_uppercase(),
        // `str.title()` of an ASCII word: first letter up, the rest down
        "title" => {
            let mut chars = word.chars();
            chars.next().map_or(String::new(), |first| {
                first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase()
            })
        }
        "lower" => word.to_lowercase(),
        _ => word.to_string(),
    }
}

/// A point header that was corrected to match the others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelFix {
    pub line: usize,
    pub from: String,
    pub to: String,
    pub others: usize,
    pub of: usize,
}

type Style = (&'static str, String, String, String, String, char);

/// `fix_label_typos`: make point headers like `KEY #1)` look the same. When most points of one kind are written one way (a space
/// before `#`, the same capitals, the same `)`) and one is written differently (`KEY#1)`), that one is corrected. Needs 3 or more
/// such headers and a clear majority. Each change is recorded on the item (`label_fixed_from`) and returned.
pub fn fix_label_typos(items: &mut [DocumentItem], cfg: &PointsConfig) -> Vec<LabelFix> {
    if !cfg.fix_label_typos {
        return Vec::new();
    }
    // groups by the word, in the order the words first appear
    let mut groups: Vec<(String, Vec<(usize, KeyLabel)>)> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let DocumentItem::Point(point) = item else {
            continue;
        };
        if point.label.is_empty() {
            continue;
        }
        if let Some(m) = key_label(&point.label) {
            let key = casefold(&m.word);
            match groups.iter_mut().find(|(k, _)| *k == key) {
                Some((_, members)) => members.push((index, m)),
                None => groups.push((key, vec![(index, m)])),
            }
        }
    }
    let style = |m: &KeyLabel| -> Style {
        (
            case_pattern(&m.word),
            m.s1.clone(),
            m.hash.clone(),
            m.s2.clone(),
            m.s3.clone(),
            m.close,
        )
    };
    let mut fixes = Vec::new();
    for (_, members) in &groups {
        if members.len() < 3 {
            continue;
        }
        // Counter(...).most_common(1): the first style (in order of appearance) with the highest count
        let mut counts: Vec<(Style, usize)> = Vec::new();
        for (_, m) in members {
            let s = style(m);
            match counts.iter_mut().find(|(k, _)| *k == s) {
                Some((_, c)) => *c += 1,
                None => counts.push((s, 1)),
            }
        }
        let mut best = 0;
        for (i, (_, c)) in counts.iter().enumerate() {
            if *c > counts[best].1 {
                best = i;
            }
        }
        let (top, count) = counts[best].clone();
        if count < 2 || count * 2 <= members.len() {
            continue;
        }
        for (index, m) in members {
            if style(m) == top {
                continue;
            }
            let fixed = format!(
                "{}{}{}{}{}{}{}",
                apply_case(&m.word, top.0),
                top.1,
                top.2,
                top.3,
                m.n,
                top.4,
                top.5
            );
            if let DocumentItem::Point(point) = &mut items[*index] {
                if fixed != point.label {
                    fixes.push(LabelFix {
                        line: point.line,
                        from: point.label.clone(),
                        to: fixed.clone(),
                        others: count,
                        of: members.len(),
                    });
                    point.label_fixed_from = std::mem::replace(&mut point.label, fixed);
                }
            }
        }
    }
    fixes
}

/// `_strip_period`: drop one closing full stop from a point (`...the Lord.` -> `...the Lord`); keep ellipses, abbreviations and other
/// endings.
pub fn strip_period(text: &str) -> String {
    const CLOSERS: [char; 6] = ['"', '\'', '”', '’', ')', ']'];
    let mut stripped = rstrip(text);
    let mut closers = String::new();
    while let Some(last) = stripped.chars().next_back().filter(|c| CLOSERS.contains(c)) {
        closers.insert(0, last);
        stripped = &stripped[..stripped.len() - last.len_utf8()];
    }
    if !stripped.ends_with('.') || stripped.ends_with("..") {
        return text.to_string();
    }
    let body = &stripped[..stripped.len() - 1];
    let last = split_ws(body).last().copied().unwrap_or("");
    let bare = casefold(last.trim_matches(|c| matches!(c, '"' | '\'' | '“' | '‘' | '(' | '[')));
    let length = bare.chars().count();
    if bare.contains('.')
        || KEEP_PERIOD.contains(&bare.as_str())
        || (length == 1 && bare.chars().all(crate::references::pytext::is_alpha))
        || bare.is_empty()
        || (!bare.is_empty() && bare.chars().all(is_digit) && length <= 2 && closers.is_empty())
    {
        return text.to_string();
    }
    format!("{body}{closers}")
}

/// `format_point`: the text of a point slide (all of it, when `max_lines_per_slide` makes the point continue on more slides).
pub fn format_point(
    item: &PointItem,
    cfg: &PointsConfig,
    counter: usize,
) -> Result<String, PlanError> {
    Ok(format_point_slides(item, cfg, counter)?.join("\n\n"))
}

/// `format_point_slides`: the text of each slide of a point: its label (kept, dropped, renumbered, or the bullet mark) and the case
/// of the words. Normally one slide; with `points.max_lines_per_slide` a longer point continues on the next slide(s).
pub fn format_point_slides(
    item: &PointItem,
    cfg: &PointsConfig,
    counter: usize,
) -> Result<Vec<String>, PlanError> {
    let mut label = item.label.clone();
    if is_numbered_label(&label) {
        if cfg.numbering == Numbering::Strip {
            label.clear();
        } else if cfg.numbering == Numbering::Renumber && item.role == PointRole::Point {
            let letters = label.chars().take_while(char::is_ascii_alphabetic).count();
            if letters >= 2 {
                let word: String = label.chars().take(letters).collect();
                let close = label.chars().next_back().unwrap_or(' ');
                label = format!("{word} #{counter}{close}");
            } else {
                label = format_n(&cfg.number_style, counter as i64).map_err(|e| {
                    PlanError::NumberStyle {
                        style: cfg.number_style.clone(),
                        why: e.0,
                    }
                })?;
            }
        }
    } else if label == "•" {
        label = cfg.bullet_char.clone();
    }
    let mut text = item.text.clone();
    match cfg.case {
        CaseMode::Upper => text = text.to_uppercase(),
        CaseMode::Lower => text = text.to_lowercase(),
        CaseMode::Title => {
            text = text
                .split('\n')
                .map(title_case)
                .collect::<Vec<_>>()
                .join("\n");
        }
        CaseMode::AsWritten => {}
    }
    if cfg.strip_trailing_period {
        text = strip_period(&text);
    }
    let shown = strip(&format!("{label} {text}")).to_string();
    if !cfg.balance_lines {
        return Ok(vec![shown]);
    }
    let glued = if label.is_empty() { 0 } else { 2 };
    let caps = cfg.shown_in_capitals || cfg.case == CaseMode::Upper;
    let mut pieces = vec![shown.clone()];
    if cfg.max_lines_per_slide > 0 {
        pieces = split_point_text(
            &shown,
            cfg.line_width_units,
            cfg.max_lines_per_slide as usize,
            caps,
            glued,
            cfg.line_tolerance,
            cfg.line_tolerance_at_punctuation,
        );
    }
    Ok(pieces
        .iter()
        .enumerate()
        .map(|(i, piece)| {
            balance_lines(
                piece,
                cfg.line_width_units,
                caps,
                if i == 0 { glued } else { 0 },
                cfg.max_lines as usize,
                cfg.line_tolerance,
                cfg.line_tolerance_at_punctuation,
            )
        })
        .collect())
}

/// `added_point_text`: a line you chose to add as a slide in the review, formatted like any other plain point (`\f` separates
/// slides).
pub fn added_point_text(text: &str, cfg: &PointsConfig) -> Result<String, PlanError> {
    let item = PointItem {
        line: 0,
        role: PointRole::ListItem,
        label: "•".to_string(),
        text: split_ws(text).join(" "),
        warnings: Vec::new(),
        detail_end: 0,
        label_fixed_from: String::new(),
    };
    Ok(format_point_slides(&item, cfg, 0)?.join("\u{c}"))
}
