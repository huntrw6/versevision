//! `classifier.py`: split a sermon document into verse slides, sermon-point slides and skipped notes.
//!
//! The classifier is deliberately conservative: a line only becomes a slide on an explicit signal (a reference, a numbered/lettered
//! marker, or a `[slide]` tag). Everything else is reported as skipped or as needing review, never guessed into a slide.
//!
//! The functions follow the Python one for one (`_label` is [`label`], `_build` is [`build`], ...), in the same order, with the same rule
//! names and the same comments, so a format fix made in the Python can be made here the same way.

use std::sync::OnceLock;

use indexmap::IndexMap;
use serde::Serialize;
use serde_json::{json, Map, Value};
use thiserror::Error;

use super::models::{
    DocumentItem, NoteDocument, PointItem, PointRole, ReviewItem, SkipCategory, SkippedItem,
    VerseItem,
};
use super::pyre::{PatternError, PyRegex};
use super::trace::{clean_value, NoTrace, TraceLog, TraceSink};
use crate::config::{
    CapsLines, Duplicates, Handling, Headings, LineBreaks, NotesConfig, PointsConfig,
    QuoteAboveReference, ReferenceInText, WrappedLines,
};
use crate::pystr;
use crate::references::pytext::{casefold, decimal_value, is_alpha, is_digit, is_lower, is_upper};
use crate::references::{
    int_value, parse_references_with, ParseIssue, PassageReference, ReferenceError,
};
use crate::versions::is_real_version;

/// What makes Python's `classify_document` raise.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ClassifyError {
    /// The reference parser raised (`John 0:1`): pydantic `ValidationError`.
    #[error("{0}")]
    Reference(ReferenceError),
    /// `re.compile` of a pattern from the config failed (`notes.skip_patterns`).
    #[error("{0}")]
    Pattern(PatternError),
    /// `AttributeError: 'ReviewItem' object has no attribute 'warnings'`: a numbered line that holds a whole-chapter reference
    /// (`1. Psalm 23`) or a verse the duplicates setting dropped. A bug of the Python app, reproduced (see the module notes of `notes`).
    #[error("'{item}' object has no attribute 'warnings'")]
    NoWarnings { item: &'static str },
}

impl ClassifyError {
    /// The name of the exception Python raises for the same input (the `raises` field of `oracle/document_errors.jsonl`).
    pub fn python_exception(&self) -> &'static str {
        match self {
            Self::Reference(e) => e.python_exception(),
            Self::Pattern(e) => e.exception,
            Self::NoWarnings { .. } => "AttributeError",
        }
    }
}

impl From<ReferenceError> for ClassifyError {
    fn from(error: ReferenceError) -> Self {
        Self::Reference(error)
    }
}

impl From<PatternError> for ClassifyError {
    fn from(error: PatternError) -> Self {
        Self::Pattern(error)
    }
}

type Aliases<'a> = Option<&'a IndexMap<String, String>>;

/// The older keyword shortcuts of `classify_document`: `line_mode=False` is `line_breaks="paragraph"`, `unlabeled_bullets=False` sends
/// plain lines to review.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Shortcuts {
    pub line_mode: Option<bool>,
    pub unlabeled_bullets: Option<bool>,
}

// ------------------------------------------------------------------------------------------------ the patterns

const STAGE_WORDS: &str = concat!(
    r"show|play|put|start|cue|fade|transition|bring up|display|run|roll|when i (?:mention(?:ed)?|sa(?:y|id)|get|got|talk(?:ed)?|read|come|came)|",
    r"after (?:this|that)|at the end|end with|stop"
);

/// The patterns that do not depend on the options, compiled once. Python source text, read by [`PyRegex`].
struct Fixed {
    /// A numbered / lettered marker at the start of a line: "1)", "1.", "#1:", "a)", "2:".
    marker: PyRegex,
    bullet: PyRegex,
    slide_tag: PyRegex,
    note: PyRegex,
    url: PyRegex,
    media_words: PyRegex,
    recap: PyRegex,
    email_header: PyRegex,
    date: PyRegex,
    /// "KEY #3) ..." starts a new point.
    keyed: PyRegex,
    keyed_point: PyRegex,
    paren: PyRegex,
    digit_colon_digit: PyRegex,
    digits_colon_digits: PyRegex,
    part_week: PyRegex,
    word_colon: PyRegex,
    first_word: PyRegex,
}

fn fixed() -> &'static Fixed {
    static FIXED: OnceLock<Fixed> = OnceLock::new();
    FIXED.get_or_init(|| {
        let build = |pattern: &str, ignore_case: bool| {
            PyRegex::new(pattern, ignore_case)
                .unwrap_or_else(|e| panic!("built-in pattern {pattern:?}: {e}"))
        };
        Fixed {
            marker: build(
                r"^\s*(?P<hash>#\s*)?(?P<label>\d{1,3}|[A-Za-z](?=[).]\s))\s*(?P<punct>[).:])\s*(?P<rest>.*)$",
                false,
            ),
            bullet: build(r"^\s*[•▪◦●‣*\-–—]\s+(?P<rest>\S.*)$", false),
            slide_tag: build(
                r"^\s*\[\s*slide\s*\]\s*[-–—:]*\s*(?P<rest>.*)$",
                true,
            ),
            note: build(
                r"^\s*(\(.*\)\s*$|\[(?!\s*slide)[^\]]*\]\s*$|(note|notes|todo|to do|reminder|remember|ask|tell|check|fix|tbd|fyi)\b\s*[:\-]?)",
                true,
            ),
            url: build(r"https?://|www\.", true),
            media_words: build(
                r"\b(slide|photo|picture|image|video|clip|screen|portrait|graphic|manuscript)\b",
                true,
            ),
            recap: build(r"^\s*recap\b", true),
            email_header: build(
                r"^\s*(from|to|cc|bcc|subject|date|sent)\s*:|<[^>\s]+@[^>\s]+>|^\[quoted text hidden\]$",
                true,
            ),
            date: build(r"\d{1,2}/\d{1,2}/\d{2,4}", false),
            keyed: build(
                r"^\s*(key|point|step|principle|reason|way|tip|lesson|truth)\s*#?\s*\d+\s*[).:\-]",
                true,
            ),
            keyed_point: build(
                r"^(?P<label>\s*(?P<word>key|point|step|principle|reason|way|tip|lesson|truth)\s*#?\s*(?P<n>\d{1,3})\s*[).:\-])\s*(?P<rest>\S.*)$",
                true,
            ),
            paren: build(
                r"^\s*[(\[]\s*(?P<inner>[^()\[\]]+?)\s*[)\]]\s*$",
                false,
            ),
            digit_colon_digit: build(r"\d:\d", false),
            digits_colon_digits: build(r"\d+\s*:\s*\d+", false),
            part_week: build(r"^(part|week|session|series)\s*\d+\s*[:.-]", true),
            word_colon: build(r"^\s*\w+\s*[:\-]", false),
            first_word: build(r"[\[(]?([A-Za-z0-9]+)", false),
        }
    })
}

const QUOTE_OPEN: [&str; 5] = ["“", "\"", "‘", "'", "«"];
const SENTENCE_END: [&str; 7] = [".", "?", "!", "”", "\"", ":", ";"];
const CLOSERS: [&str; 4] = ["”", "\"", "’", "'"];

fn starts_with_any(text: &str, set: &[&str]) -> bool {
    set.iter().any(|s| text.starts_with(s))
}

fn ends_with_any(text: &str, set: &[&str]) -> bool {
    set.iter().any(|s| text.ends_with(s))
}

/// `re.escape(text)` (Python 3.7+: only the characters that are special in a pattern).
fn re_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if "()[]{}?*+-|^$\\.&~# \t\n\r\u{b}\u{c}".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The options a run uses, compiled once.
struct Rules<'a> {
    notes: NotesConfig,
    points: &'a PointsConfig,
    stage: PyRegex,
    skip: Vec<PyRegex>,
    labelled: Option<PyRegex>,
}

impl<'a> Rules<'a> {
    fn new(notes: NotesConfig, points: &'a PointsConfig) -> Result<Self, ClassifyError> {
        let extra = notes
            .extra_cue_words
            .iter()
            .map(|w| pystr::strip(w))
            .filter(|w| !w.is_empty())
            .map(re_escape)
            .collect::<Vec<_>>()
            .join("|");
        let stage_pattern = format!(
            r"^\s*({}{})\b",
            STAGE_WORDS,
            if extra.is_empty() {
                String::new()
            } else {
                format!("|{extra}")
            }
        );
        let stage = PyRegex::new(&stage_pattern, true)?;
        let skip = notes
            .skip_patterns
            .iter()
            .map(|x| PyRegex::new(x, true))
            .collect::<Result<Vec<_>, _>>()?;
        let words = notes
            .label_words
            .iter()
            .map(|w| pystr::strip(w))
            .filter(|w| !w.is_empty())
            .map(re_escape)
            .collect::<Vec<_>>()
            .join("|");
        let labelled = if words.is_empty() {
            None
        } else {
            Some(PyRegex::new(
                &format!(r"^\s*({words})\s*[:\-–—]\s*\S"),
                true,
            )?)
        };
        Ok(Self {
            notes,
            points,
            stage,
            skip,
            labelled,
        })
    }
}

// ------------------------------------------------------------------------------------------------ small Python helpers

fn first_char(text: &str) -> Option<char> {
    text.chars().next()
}

/// `text[:1].isupper()`
fn first_is_upper(text: &str) -> bool {
    first_char(text).is_some_and(|c| is_upper(&c.to_string()))
}

/// `text[:1].islower()`
fn first_is_lower(text: &str) -> bool {
    first_char(text).is_some_and(|c| is_lower(&c.to_string()))
}

/// `text[:1].isdigit()`
fn first_is_digit(text: &str) -> bool {
    first_char(text).is_some_and(is_digit)
}

fn char_count(text: &str) -> usize {
    text.chars().count()
}

/// `int(text)` of a string of decimal digits.
fn py_int(text: &str) -> u64 {
    int_value(text).unwrap_or(0)
}

fn enum_value<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

// ------------------------------------------------------------------------------------------------ references

/// References that open a line (and any chained with ';' or ',' directly after).
fn lead_references(
    text: &str,
    aliases: Aliases,
) -> Result<(Vec<PassageReference>, Vec<ParseIssue>), ClassifyError> {
    let parsed = parse_references_with(text, aliases)?;
    let mut taken: Vec<PassageReference> = Vec::new();
    let mut end = 0usize;
    for reference in &parsed.references {
        if taken.is_empty() && reference.source_start > 2 {
            break;
        }
        if !taken.is_empty() && reference.source_start as i64 - end as i64 > 3 {
            break;
        }
        taken.push(reference.clone());
        end = reference.source_end;
    }
    Ok((taken, parsed.issues))
}

/// A reference at the END of a quoted block (`“verse text” John 3:16 NIV`).
fn trailing_references(
    text: &str,
    aliases: Aliases,
) -> Result<Vec<PassageReference>, ClassifyError> {
    let parsed = parse_references_with(text, aliases)?;
    let Some(last) = parsed.references.last() else {
        return Ok(Vec::new());
    };
    if char_count(text) as i64 - last.source_end as i64 > 4 {
        return Ok(Vec::new());
    }
    let mut chain = vec![last.clone()];
    for earlier in parsed.references[..parsed.references.len() - 1]
        .iter()
        .rev()
    {
        if chain[0].source_start as i64 - earlier.source_end as i64 <= 3 {
            chain.insert(0, earlier.clone());
        } else {
            break;
        }
    }
    Ok(chain)
}

fn is_reference_start(text: &str, aliases: Aliases) -> Result<bool, ClassifyError> {
    Ok(!lead_references(text, aliases)?.0.is_empty())
}

// ------------------------------------------------------------------------------------------------ paragraphs

/// What a paragraph was recognised as (`_Para.kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Other,
    SlideTag,
    Verse,
    Stage,
    Marker,
    Header,
    Midref,
    Badref,
    Quote,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Other => "other",
            Self::SlideTag => "slide_tag",
            Self::Verse => "verse",
            Self::Stage => "stage",
            Self::Marker => "marker",
            Self::Header => "header",
            Self::Midref => "midref",
            Self::Badref => "badref",
            Self::Quote => "quote",
        }
    }
}

/// `_Para.extra`.
#[derive(Debug, Clone, Default)]
struct Extra {
    rule: String,
    why: Option<String>,
    /// The marker as written ("1)", "#2:", "a)", "•"); `None` for a paragraph that is not a marker.
    label: Option<String>,
    number: Option<u64>,
    numbered_verse: bool,
    seen_content: bool,
}

#[derive(Debug, Clone)]
struct Para {
    line: usize,
    end: usize,
    text: String,
    last_len: i64,
    kind: Kind,
    refs: Vec<PassageReference>,
    extra: Extra,
    first: bool,
}

impl Para {
    fn new(line: usize, text: &str, last_len: i64) -> Self {
        Self {
            line,
            end: line,
            text: pystr::strip(text).to_string(),
            last_len,
            kind: Kind::Other,
            refs: Vec::new(),
            extra: Extra::default(),
            first: false,
        }
    }

    fn append(&mut self, text: &str, last_len: i64, line: usize) {
        self.text = pystr::strip(&format!("{} {}", self.text, pystr::strip(text))).to_string();
        self.last_len = last_len;
        if line != 0 {
            self.end = line;
        }
    }
}

fn header_like(words: &[&str]) -> bool {
    !words.is_empty() && words.iter().all(|w| first_is_upper(w) || first_is_digit(w))
}

/// A numbered/lettered point marker, or `None`. Rejects '12: 9 NKJV'-style verse fragments.
struct MarkerMatch {
    hash: Option<String>,
    label: String,
    punct: String,
    rest: String,
}

fn marker(stripped: &str, points: &PointsConfig) -> Option<MarkerMatch> {
    let caps = fixed().marker.search(stripped)?;
    let rest = caps.name("rest").unwrap_or_default().to_string();
    if pystr::strip(&rest).is_empty() {
        return None;
    }
    let label = caps.name("label").unwrap_or_default().to_string();
    let is_numeric = label.chars().all(is_digit) && !label.is_empty();
    if !(if is_numeric {
        points.numbered
    } else {
        points.lettered
    }) {
        return None;
    }
    let punct = caps.name("punct").unwrap_or_default().to_string();
    if punct == ":" && first_is_digit(rest.trim_start_matches(pystr::is_space)) {
        return None;
    }
    Some(MarkerMatch {
        hash: caps.name("hash").map(str::to_string),
        label,
        punct,
        rest,
    })
}

fn is_caps(text: &str) -> bool {
    let letters: Vec<char> = text.chars().filter(|c| is_alpha(*c)).collect();
    letters.len() >= 3 && letters.iter().all(|c| is_upper(&c.to_string()))
}

const NARROW: &str = "iltfjI.,;:'’!|() ";

/// How wide a line looks on the page, in plain-letter units: capitals are wider and i, l, t, punctuation and spaces are narrower.
/// A line of capitals fills the page with far fewer characters, so measuring characters misses wrapped headings.
fn width(text: &str) -> i64 {
    let values: Vec<f64> = pystr::rstrip(text)
        .chars()
        .map(|c| {
            if is_upper(&c.to_string()) {
                1.3
            } else if NARROW.contains(c) {
                0.6
            } else {
                1.0
            }
        })
        .collect();
    pystr::sum(&values).round_ties_even() as i64
}

/// Detect hard-wrapped text (PDF/email) and the width at which a line counts as full.
fn wrap_geometry(lines: &[&str]) -> (bool, i64) {
    let mut lengths: Vec<i64> = lines
        .iter()
        .filter(|l| !pystr::strip(l).is_empty())
        .map(|l| width(l))
        .collect();
    lengths.sort_unstable();
    if lengths.len() < 8 {
        return (false, 0);
    }
    let index = ((lengths.len() as f64 * 0.95) as usize).saturating_sub(1);
    let p95 = lengths[index];
    let near = lengths
        .iter()
        .filter(|n| 0.86 * p95 as f64 <= **n as f64 && **n as f64 <= 1.05 * p95 as f64)
        .count();
    (
        (50..=140).contains(&p95) && near as f64 / lengths.len() as f64 >= 0.15,
        (0.86 * p95 as f64) as i64,
    )
}

/// Why this line is the rest of the line above (the page wrapped it mid-sentence), or `None` when it is a new line of its own.
///
/// 1. A sentence that has already ended (. ! ?) is never continued, however long the line was: the next line is something new
///    (unless the sentence sits inside a quotation that is still open, or the next line starts in lower case).
/// 2. Otherwise it is the rest of the line above when that line is full width, or when it is a tail of a few words and the line above
///    is at least over half full.
fn wrap_reason(
    current: &Para,
    stripped: &str,
    threshold: i64,
    notes: &NotesConfig,
    points: &PointsConfig,
    is_reference: bool,
) -> Option<String> {
    let above = pystr::rstrip(&current.text);
    let ended = ends_with_any(
        above.trim_end_matches(['”', '"', '’', '\'']),
        &[".", "?", "!"],
    );
    let in_open_quote = starts_with_any(above, &QUOTE_OPEN) && !ends_with_any(above, &CLOSERS);
    let lower = first_is_lower(stripped);
    if marker(above, points).is_some()
        && ends_with_any(above, &SENTENCE_END[..5])
        && first_is_upper(stripped)
    {
        return None; // a numbered point that reads as complete: the next line is something new
    }
    if ended && !in_open_quote && !lower && !is_reference {
        return None; // (a reference on the next line belongs to the verse text above it, so that case still joins)
    }
    let words = pystr::split_ws(stripped).len() as i64;
    let wide = current.last_len;
    if is_caps(above) != is_caps(stripped) && words > i64::from(notes.orphan_max_words) {
        return None; // a heading in capitals is not continued by a sentence in lower case, or the other way round
    }
    if is_reference && wide < threshold {
        return None; // a reference line is never a "tail": it only joins the verse text above it when that line was full width
    }
    if wide >= threshold {
        return Some(format!("the line above looks full width ({wide} of {threshold}) and its sentence has not ended, so this line is the rest of it"));
    }
    if wide as f64 >= 0.55 * threshold as f64 && words <= i64::from(notes.orphan_max_words) {
        return Some(format!("this is a {words}-word tail and the line above (width {wide} of {threshold}) does not end its sentence, so it finishes that sentence"));
    }
    if wide as f64 >= 0.55 * threshold as f64 && lower {
        return Some(
            "this line starts in lower case, so it carries on the sentence above".to_string(),
        );
    }
    None
}

// ------------------------------------------------------------------------------------------------ the entry points

/// Classify a document into verse / point / skipped / review items, in document order.
///
/// Every choice (what a line break means, what to do with plain lines, cues, quotes...) comes from the options; the defaults are the
/// house style. `aliases` is `bible.version_aliases` (`None`: the parser's default).
pub fn classify_document(
    text: &str,
    aliases: Aliases,
    notes: Option<&NotesConfig>,
    points: Option<&PointsConfig>,
) -> Result<NoteDocument, ClassifyError> {
    classify_document_traced(
        text,
        aliases,
        notes,
        points,
        Shortcuts::default(),
        &mut NoTrace,
    )
}

/// [`classify_document`] with the older keyword shortcuts, writing the `parse_start`, `line_join`, `paragraph` and `item` records to `sink`.
pub fn classify_document_traced(
    text: &str,
    aliases: Aliases,
    notes: Option<&NotesConfig>,
    points: Option<&PointsConfig>,
    shortcuts: Shortcuts,
    sink: &mut dyn TraceSink,
) -> Result<NoteDocument, ClassifyError> {
    let mut notes = notes.cloned().unwrap_or_default();
    let default_points = PointsConfig::default();
    let points = points.unwrap_or(&default_points);
    if shortcuts.line_mode == Some(false) {
        notes.line_breaks = LineBreaks::Paragraph;
    }
    if shortcuts.unlabeled_bullets == Some(false) {
        notes.unlabeled_sentence = Handling::Review;
        notes.unlabeled_short = Handling::Review;
    }
    let rules = Rules::new(notes, points)?;
    let mut tracer = Tracer { sink };
    let normalized = text
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\u{a0}', " ");
    let lines: Vec<&str> = normalized.split('\n').collect();
    let (hard_wrap, threshold) = wrap_geometry(&lines);
    tracer.trace(
        "parse_start",
        [
            ("lines", json!(lines.len())),
            ("non_blank", json!(lines.iter().filter(|x| !pystr::strip(x).is_empty()).count())),
            ("hard_wrapped", json!(hard_wrap)),
            ("full_line_width", json!(threshold)),
            ("wrapped_lines", enum_value(&rules.notes.wrapped_lines)),
            ("orphan_max_words", json!(rules.notes.orphan_max_words)),
            ("note", json!("hard_wrapped means the text looks like a PDF/email with lines broken mid-sentence. Widths are measured as they look on the page \
                 (capitals are wider). A line is joined to the one above when that line has not ended its sentence and is full width, \
                 or when it is a short tail (orphan_max_words or fewer) after a line that is over half full")),
        ],
    );

    let mut paras: Vec<Para> = Vec::new();
    let mut current: Option<usize> = None; // index into paras
    for (index, raw) in lines.iter().enumerate() {
        let number = index + 1;
        let line = pystr::rstrip(raw);
        let stripped = pystr::strip(line);
        if stripped.is_empty() {
            current = None;
            continue;
        }
        let mut why = None;
        if let Some(at) = current {
            if hard_wrap
                && rules.notes.wrapped_lines == WrappedLines::Join
                && !starts_new(stripped, &paras[at], &rules, aliases)?
            {
                why = wrap_reason(
                    &paras[at],
                    stripped,
                    threshold,
                    &rules.notes,
                    rules.points,
                    is_reference_start(stripped, aliases)?,
                );
            }
        }
        if let (Some(why), Some(at)) = (why, current) {
            tracer.trace(
                "line_join",
                [
                    ("line", json!(number)),
                    ("into_paragraph", json!(paras[at].line)),
                    ("text", json!(stripped)),
                    ("why", json!(why)),
                ],
            );
            paras[at].append(stripped, width(line), number);
            continue;
        }
        paras.push(Para::new(number, stripped, width(line)));
        current = Some(paras.len() - 1);
    }

    if let Some(first) = paras.first_mut() {
        first.first = true;
    }
    for para in &mut paras {
        label(para, aliases, &rules)?;
        let raw_lines: Vec<&str> = (para.line - 1..para.end.min(lines.len()))
            .filter(|i| !pystr::strip(lines[*i]).is_empty())
            .map(|i| pystr::rstrip(lines[i]))
            .collect();
        let references: Vec<Value> = para
            .refs
            .iter()
            .map(|r| {
                json!({"as_written": pystr::strip(&r.source_text), "normalized": r.normalized_text,
                       "version_written": r.version, "warnings": r.warnings})
            })
            .collect();
        tracer.trace(
            "paragraph",
            [
                ("line", json!(para.line)),
                ("end_line", json!(para.end)),
                ("raw_lines", json!(raw_lines)),
                ("text", json!(para.text)),
                ("first_in_document", json!(para.first)),
                ("label", json!(para.kind.as_str())),
                ("rule", json!(para.extra.rule)),
                ("why", json!(para.extra.why.clone().unwrap_or_default())),
                ("marker", json!(para.extra.label)),
                ("references", Value::Array(references)),
            ],
        );
    }
    build(&paras, aliases, &rules, &mut tracer)
}

/// Writes records the way `runlog.trace` does: cleaned (long strings cut, long lists cut).
struct Tracer<'a> {
    sink: &'a mut dyn TraceSink,
}

impl Tracer<'_> {
    fn trace<const N: usize>(&mut self, stage: &str, fields: [(&str, Value); N]) {
        let mut map = Map::new();
        for (key, value) in fields {
            map.insert(key.to_string(), value);
        }
        let Value::Object(cleaned) = clean_value(Value::Object(map), 2000) else {
            return;
        };
        self.sink.trace(stage, cleaned);
    }
}

/// Does this line start a new paragraph whatever the wrapping says?
fn starts_new(
    stripped: &str,
    current: &Para,
    rules: &Rules,
    aliases: Aliases,
) -> Result<bool, ClassifyError> {
    let f = fixed();
    let points = rules.points;
    if f.slide_tag.is_match(stripped)
        || rules.stage.is_match(stripped)
        || f.url.is_match(stripped)
        || f.recap.is_match(stripped)
        || rules.skip.iter().any(|x| x.is_match(stripped))
    {
        return Ok(true);
    }
    if f.keyed.is_match(stripped) {
        return Ok(true);
    }
    let quote_still_open = starts_with_any(&current.text, &QUOTE_OPEN)
        && !ends_with_any(pystr::rstrip(&current.text), &CLOSERS);
    if starts_with_any(stripped, &QUOTE_OPEN) && !quote_still_open {
        return Ok(true); // a line that opens a quotation is a new item, unless the quotation above is still open
    }
    if f.note.is_match(stripped) {
        let inside_quote = quote_still_open;
        if inside_quote
            && !f.word_colon.is_match(stripped)
            && !(stripped.starts_with('(') || stripped.starts_with('['))
        {
            return Ok(false); // a wrapped line of a quote ("remember me when You come into Your kingdom")
        }
        return Ok(true);
    }
    if points.bullets && f.bullet.is_match(stripped) {
        return Ok(true);
    }
    if marker(stripped, points).is_some() && !is_reference_start(stripped, aliases)? {
        return Ok(true);
    }
    if starts_with_any(stripped, &QUOTE_OPEN) && ends_with_any(&current.text, &CLOSERS) {
        return Ok(true);
    }
    let first = f.first_word.search(stripped);
    if !trailing_references(&current.text, aliases)?.is_empty()
        && !first
            .as_ref()
            .and_then(|m| m.group(1))
            .is_some_and(is_real_version)
    {
        return Ok(true); // a reference at the end of a line closes that entry
    }
    if is_reference_start(stripped, aliases)? {
        // a reference line either finishes a quote above it, or starts a new verse entry
        return Ok(is_reference_start(&current.text, aliases)?
            || !trailing_references(&current.text, aliases)?.is_empty());
    }
    Ok(false)
}

// ------------------------------------------------------------------------------------------------ the rules

/// `re.match(r"^\s*[(\[]\s*(?P<inner>[^()\[\]]+?)\s*[)\]]\s*$", text)`: '(Romans 8:16)' or '(Romans 8:16 NKJV)': a whole line that is
/// just a reference in brackets.
fn paren_reference(text: &str, aliases: Aliases) -> Result<Vec<PassageReference>, ClassifyError> {
    let Some(caps) = fixed().paren.search(text) else {
        return Ok(Vec::new());
    };
    let inner = caps.name("inner").unwrap_or_default();
    let parsed = parse_references_with(inner, aliases)?;
    if parsed.references.len() != 1 || !parsed.issues.is_empty() {
        return Ok(Vec::new());
    }
    let reference = &parsed.references[0];
    if reference.source_start <= 1 && char_count(inner) as i64 - reference.source_end as i64 <= 1 {
        Ok(vec![reference.clone()])
    } else {
        Ok(Vec::new())
    }
}

/// Decide what one paragraph is (`_label`): sets `para.kind`, `para.refs`, `para.text` and the rule that fired.
fn label(para: &mut Para, aliases: Aliases, rules: &Rules) -> Result<(), ClassifyError> {
    let f = fixed();
    let text = para.text.clone();
    let points = rules.points;
    if let Some(tag) = f.slide_tag.search(&text) {
        para.kind = Kind::SlideTag;
        para.text = pystr::strip(tag.name("rest").unwrap_or_default()).to_string();
        para.extra.rule = "[slide] tag".to_string();
        return Ok(());
    }
    let paren = paren_reference(&text, aliases)?;
    if !paren.is_empty() {
        para.kind = Kind::Verse;
        para.refs = paren;
        para.extra.rule = "whole line is a reference in brackets".to_string();
        return Ok(());
    }
    let skip_hit = rules
        .skip
        .iter()
        .find(|x| x.is_match(&text))
        .map(|x| x.pattern.clone());
    // (an empty pattern matches every line but is falsy, so it does not count as a hit here)
    if let Some(pattern) = skip_hit.filter(|p| !p.is_empty()) {
        para.kind = Kind::Stage;
        para.extra.why = Some("matches a skip pattern".to_string());
        para.extra.rule = format!(
            "matches your skip pattern {}",
            super::pyre::py_repr(&pattern)
        );
        return Ok(());
    }
    if rules.stage.is_match(&text) || f.url.is_match(&text) || f.note.is_match(&text) {
        para.kind = Kind::Stage;
        para.extra.rule = if rules.stage.is_match(&text) {
            "starts with a stage/cue word"
        } else if f.url.is_match(&text) {
            "contains a web address"
        } else {
            "reads like a note to self (brackets, 'note:', 'remember'...)"
        }
        .to_string();
        return Ok(());
    }
    let bullet = if points.bullets {
        f.bullet.search(&text)
    } else {
        None
    };
    if let Some(bullet) = &bullet {
        let rest = bullet.name("rest").unwrap_or_default();
        if !is_reference_start(rest, aliases)? {
            para.kind = Kind::Marker;
            para.text = pystr::strip(rest).to_string();
            para.extra.label = Some("•".to_string());
            para.extra.number = None;
            para.extra.rule = "bullet character at the start of the line".to_string();
            para.refs = parse_references_with(&para.text, aliases)?.references;
            return Ok(());
        }
    }
    if f.recap.is_match(&text) {
        para.kind = Kind::Header;
        para.extra.why = Some("recap heading".to_string());
        para.extra.rule = "line starts with 'Recap'".to_string();
        return Ok(());
    }
    if f.date.is_match(&text) && !f.digit_colon_digit.is_match(&text) {
        para.kind = Kind::Header;
        para.extra.why = Some("title / date line".to_string());
        para.extra.rule = "contains a date (d/m/y) and no verse-like d:d".to_string();
        return Ok(());
    }
    let keyed = if points.numbered {
        f.keyed_point.search(&text)
    } else {
        None
    };
    if let Some(keyed) = keyed {
        para.kind = Kind::Marker;
        para.text = pystr::strip(keyed.name("rest").unwrap_or_default()).to_string();
        para.extra.label = Some(pystr::split_ws(keyed.name("label").unwrap_or_default()).join(" "));
        para.extra.number = Some(py_int(keyed.name("n").unwrap_or_default()));
        para.extra.rule = format!(
            "'{} #n)' is the point's own header",
            keyed.name("word").unwrap_or_default().to_uppercase()
        );
        para.refs = parse_references_with(&para.text, aliases)?.references;
        return Ok(());
    }
    let marker = marker(&text, points);
    if let Some(m) = &marker {
        let (inner, _) = lead_references(pystr::strip(&m.rest), aliases)?;
        if !inner.is_empty() && !is_reference_start(&text, aliases)? {
            para.kind = Kind::Verse;
            para.refs = inner;
            para.extra.numbered_verse = true;
            para.extra.rule = "numbered line whose text starts with a Bible reference".to_string();
            return Ok(());
        }
    }
    if let Some(m) = &marker {
        if !is_reference_start(&text, aliases)? {
            let display = if m.hash.is_some() {
                format!("#{}{}", m.label, m.punct)
            } else {
                format!("{}{}", m.label, m.punct)
            };
            para.kind = Kind::Marker;
            para.text = pystr::strip(&m.rest).to_string();
            let numeric = m.label.chars().all(|c| decimal_value(c).is_some());
            para.extra.number = if numeric {
                Some(py_int(&m.label))
            } else {
                None
            };
            para.extra.label = Some(display);
            para.extra.rule = "numbered/lettered marker at the start of the line".to_string();
            para.refs = parse_references_with(&para.text, aliases)?.references;
            return Ok(());
        }
    }
    let (leads, _) = lead_references(&text, aliases)?;
    if !leads.is_empty() {
        para.kind = Kind::Verse;
        para.refs = leads;
        para.extra.rule = "Bible reference at the start of the line".to_string();
        return Ok(());
    }
    let trailing = trailing_references(&text, aliases)?;
    if !trailing.is_empty() {
        para.kind = Kind::Verse;
        para.refs = trailing;
        para.extra.rule = "Bible reference at the end of a quoted block".to_string();
        return Ok(());
    }
    let found = parse_references_with(&text, aliases)?;
    if !found.references.is_empty() {
        para.kind = Kind::Midref;
        para.refs = found.references;
        para.extra.rule = "Bible reference found inside a sentence".to_string();
        return Ok(());
    }
    // it reads like a verse at the start of the line, but it is not a real one
    if let Some(leading) = found.issues.iter().find(|i| i.source_start <= 2) {
        para.kind = Kind::Badref;
        para.extra.why = Some(format!(
            "{}: {}",
            pystr::strip(&leading.source_text),
            leading.message
        ));
        para.extra.rule =
            "looks like a reference at the start of the line, but is not a real verse".to_string();
        return Ok(());
    }
    para.kind = if starts_with_any(&text, &QUOTE_OPEN) {
        Kind::Quote
    } else {
        Kind::Other
    };
    para.extra.rule = if para.kind == Kind::Quote {
        "starts with a quotation mark"
    } else {
        "no reference, marker or cue word: a plain line"
    }
    .to_string();
    Ok(())
}

// ------------------------------------------------------------------------------------------------ building the items

/// A title or heading line as its own slide, with no bullet (it is not a point, just words to have ready).
fn heading(para: &Para, text: &str) -> DocumentItem {
    DocumentItem::Point(PointItem {
        line: para.line,
        role: PointRole::SlideNote,
        label: String::new(),
        text: text.to_string(),
        warnings: Vec::new(),
        detail_end: para.end,
        label_fixed_from: String::new(),
    })
}

fn skipped(line: usize, category: SkipCategory, text: &str, reason: &str) -> DocumentItem {
    DocumentItem::Skipped(SkippedItem {
        line,
        category,
        text: text.to_string(),
        reason: reason.to_string(),
    })
}

fn review(line: usize, text: &str, reason: &str) -> DocumentItem {
    DocumentItem::Review(ReviewItem {
        line,
        text: text.to_string(),
        reason: reason.to_string(),
    })
}

fn point(
    line: usize,
    role: PointRole,
    label: &str,
    text: &str,
    warnings: Vec<String>,
    detail_end: usize,
) -> DocumentItem {
    DocumentItem::Point(PointItem {
        line,
        role,
        label: label.to_string(),
        text: text.to_string(),
        warnings,
        detail_end,
        label_fixed_from: String::new(),
    })
}

/// `_is_book_name`: a part of a reference ("2 Corinthians") is never a title.
fn is_book_name(text: &str, aliases: Aliases) -> Result<bool, ClassifyError> {
    let empty = IndexMap::new();
    let found =
        parse_references_with(&format!("{text} 1:1"), Some(aliases.unwrap_or(&empty)))?.references;
    Ok(found.first().is_some_and(|r| r.source_start <= 1))
}

/// A plain line: heading, preamble, or one of the three ways the options say to treat unlabeled text.
fn other(
    para: &Para,
    text: &str,
    rules: &Rules,
    aliases: Aliases,
) -> Result<DocumentItem, ClassifyError> {
    let f = fixed();
    let notes = &rules.notes;
    let line = para.line;
    let ends = ends_with_any(text, &[".", "?", "!", "”", "\""]);
    // a part of a reference ("2 Corinthians", "12:9 NKJV") is never a title
    let as_heading = notes.headings == Headings::Slide
        && !f.digits_colon_digits.is_match(text)
        && !is_book_name(text, aliases)?;
    if f.part_week.is_match(text) || text.ends_with(':') {
        return Ok(if as_heading {
            heading(para, text)
        } else {
            skipped(line, SkipCategory::Header, text, "title / heading line")
        });
    }
    let words = pystr::split_ws(text);
    if f.email_header.is_match(text) {
        return Ok(skipped(
            line,
            SkipCategory::Header,
            text,
            "email header line",
        ));
    }
    if para.first && !ends && notes.before_first_item == Handling::Skip {
        return Ok(if as_heading {
            heading(para, text)
        } else {
            skipped(
                line,
                SkipCategory::Header,
                text,
                "document title (first line)",
            )
        });
    }
    if words.len() <= notes.header_max_words as usize && header_like(&words) && !ends {
        if as_heading {
            return Ok(heading(para, text));
        }
        return Ok(skipped(
            line,
            SkipCategory::Header,
            text,
            &format!(
                "{} or fewer capitalized words: looks like a heading",
                notes.header_max_words
            ),
        ));
    }

    let handle = |mode: Handling, review_reason: &str, bullet_warning: Vec<String>| match mode {
        Handling::Bullet => point(
            line,
            PointRole::ListItem,
            "•",
            text,
            bullet_warning,
            para.end,
        ),
        Handling::Skip => skipped(
            line,
            SkipCategory::Header,
            text,
            "unlabeled line (your setting skips these)",
        ),
        Handling::Review => review(line, text, review_reason),
    };

    if !para.extra.seen_content {
        if char_count(text) <= 80 && !ends {
            if notes.before_first_item == Handling::Skip {
                return Ok(if as_heading {
                    heading(para, text)
                } else {
                    skipped(
                        line,
                        SkipCategory::Header,
                        text,
                        "title/preamble before the first verse or point",
                    )
                });
            }
            return Ok(handle(
                if notes.before_first_item == Handling::Bullet {
                    Handling::Bullet
                } else {
                    Handling::Review
                },
                "line before the first verse or point (your setting asks to check these)",
                vec![
                    "line before the first verse or point made a slide because of your setting"
                        .to_string(),
                ],
            ));
        }
        if notes.before_first_item == Handling::Skip && char_count(text) <= 80 {
            return Ok(skipped(
                line,
                SkipCategory::Header,
                text,
                "title/preamble before the first verse or point",
            ));
        }
        return Ok(if notes.before_first_item != Handling::Bullet {
            handle(
                Handling::Review,
                "unlabeled line; not a reference, point, or known cue",
                Vec::new(),
            )
        } else {
            handle(
                Handling::Bullet,
                "",
                vec![
                    "line before the first verse or point made a slide because of your setting"
                        .to_string(),
                ],
            )
        });
    }
    if as_heading
        && notes.caps_lines == CapsLines::Heading
        && !ends
        && is_caps(text)
        && words.len() >= 2
    {
        return Ok(heading(para, text)); // ALL CAPITALS, no full stop: a heading, so no bullet
    }
    if char_count(text) <= 80 && !ends {
        return Ok(handle(
            notes.unlabeled_short,
            "short unlabeled line: a heading, or a point missing its number? add a number or bullet to make it a slide",
            vec!["short line with no ending punctuation made a bullet point; it may be a heading instead".to_string()],
        ));
    }
    Ok(handle(
        notes.unlabeled_sentence,
        "unlabeled line; not a reference, point, or known cue",
        Vec::new(),
    )) // a full sentence: the line-per-slide rule, no guess
}

/// The verse text the notes quote with this reference: the quotation before it on the same line ("…" (John 3:3)), or the quotation
/// paragraph just above it.
fn quote_of(para: &Para, prev: Option<&Para>, reference: &PassageReference) -> String {
    let text = &para.text;
    let wanted = pystr::strip(&reference.source_text);
    let at = if wanted.is_empty() {
        None
    } else {
        text.rfind(wanted)
    };
    if let Some(at) = at.filter(|at| *at > 0) {
        let before = pystr::strip(
            pystr::strip(&text[..at]).trim_end_matches(['(', '—', '–', '-', ',', ';', ' ']),
        );
        if pystr::split_ws(before).len() >= 5 {
            return before.to_string();
        }
    }
    if let Some(prev) = prev {
        if prev.kind == Kind::Quote
            && para.line as i64 - prev.end as i64 <= 3
            && pystr::split_ws(&prev.text).len() >= 5
        {
            return prev.text.clone();
        }
    }
    String::new()
}

fn emit_verses(
    doc: &mut NoteDocument,
    para: &Para,
    refs: &[PassageReference],
    prev: Option<&Para>,
) {
    for reference in refs {
        if reference
            .warnings
            .iter()
            .any(|warning| warning.contains("whole chapter"))
        {
            doc.items.push(review(
                para.line,
                &reference.source_text,
                "whole-chapter reference; add verse numbers to include it",
            ));
            continue;
        }
        doc.items.push(DocumentItem::Verse(VerseItem {
            line: para.line,
            reference: reference.clone(),
            warnings: reference.warnings.clone(),
            quoted: quote_of(para, prev, reference),
        }));
    }
}

fn build(
    paras: &[Para],
    aliases: Aliases,
    rules: &Rules,
    tracer: &mut Tracer,
) -> Result<NoteDocument, ClassifyError> {
    let notes = &rules.notes;
    let points = rules.points;
    let duplicates = notes.duplicates;
    let mut doc = NoteDocument::default();
    let mut seen_refs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut last_point: u64 = 0;
    let mut seen_content = false;
    for (index, source_para) in paras.iter().enumerate() {
        let mut para = source_para.clone();
        let items_before = doc.items.len();
        let prior_text: Option<String> = match doc.items.last() {
            Some(DocumentItem::Point(p)) => Some(p.text.clone()),
            _ => None,
        };
        para.extra.seen_content = seen_content;
        if matches!(
            para.kind,
            Kind::Verse | Kind::Marker | Kind::SlideTag | Kind::Midref
        ) {
            seen_content = true;
        }
        let (kind, line) = (para.kind, para.line);
        let text = para.text.clone();
        match kind {
            Kind::SlideTag => {
                if text.is_empty() {
                    doc.items
                        .push(review(line, "[slide]", "[slide] tag with no text"));
                } else if fixed().media_words.is_match(&text) && char_count(&text) < 60 {
                    doc.items.push(skipped(
                        line,
                        SkipCategory::MediaMarker,
                        &text,
                        "[slide] tag that points at an existing slide or media item",
                    ));
                } else {
                    doc.items.push(point(
                        line,
                        PointRole::SlideNote,
                        "•",
                        &text,
                        Vec::new(),
                        line,
                    ));
                }
            }
            Kind::Stage => {
                let why = para
                    .extra
                    .why
                    .clone()
                    .unwrap_or_else(|| "stage/production cue".to_string());
                match notes.stage_cues {
                    Handling::Review => doc.items.push(review(
                        line,
                        &text,
                        &format!("{why}: your setting asks to check these"),
                    )),
                    Handling::Bullet => doc.items.push(point(
                        line,
                        PointRole::SlideNote,
                        "•",
                        &text,
                        vec![format!("{why} made a slide because of your setting")],
                        line,
                    )),
                    Handling::Skip => {
                        doc.items
                            .push(skipped(line, SkipCategory::StageNote, &text, &why))
                    }
                }
            }
            Kind::Header => {
                if para.extra.why.as_deref() == Some("recap heading")
                    && notes.headings == Headings::Slide
                {
                    doc.items.push(heading(&para, &text));
                } else {
                    doc.items.push(skipped(
                        line,
                        SkipCategory::Header,
                        &text,
                        para.extra.why.as_deref().unwrap_or("heading"),
                    ));
                }
            }
            Kind::Marker => {
                let number = para.extra.number;
                let label = para.extra.label.clone().unwrap_or_default();
                let role;
                match number {
                    None => role = PointRole::ListItem, // bullets and lettered items
                    Some(n) if n == 1 || n == last_point + 1 || !points.require_sequence => {
                        role = PointRole::Point;
                        last_point = n;
                    }
                    Some(_) => role = PointRole::ListItem,
                }
                let mut warnings = Vec::new();
                if role == PointRole::ListItem && number.is_some() {
                    warnings.push(format!(
                        "numbered out of sequence (after point {last_point}); treated as a list item"
                    ));
                }
                for reference in &para.refs {
                    warnings.push(format!(
                        "text mentions {}; not added as a verse slide",
                        reference.normalized_text
                    ));
                }
                doc.items
                    .push(point(line, role, &label, &text, warnings, line));
            }
            Kind::Verse => {
                doc.issues.extend(lead_references(&text, aliases)?.1);
                let before = doc.items.len();
                let prev = if index > 0 {
                    Some(&paras[index - 1])
                } else {
                    None
                };
                emit_verses(&mut doc, &para, &para.refs, prev);
                if duplicates == Duplicates::Drop {
                    let added: Vec<DocumentItem> = doc.items[before..].to_vec();
                    for item in added {
                        let key = match &item {
                            DocumentItem::Verse(v) => casefold(&v.reference.normalized_text),
                            _ => String::new(),
                        };
                        if !key.is_empty() && seen_refs.contains(&key) {
                            if let Some(at) = doc.items.iter().position(|i| *i == item) {
                                doc.items.remove(at);
                            }
                            let DocumentItem::Verse(verse) = &item else {
                                continue;
                            };
                            doc.items.push(skipped(
                                verse.line,
                                SkipCategory::StageNote,
                                &verse.reference.normalized_text,
                                "same verse already added above (your duplicates setting)",
                            ));
                        } else if !key.is_empty() {
                            seen_refs.insert(key);
                        }
                    }
                }
                if para.extra.numbered_verse {
                    match doc.items.last_mut() {
                        Some(DocumentItem::Verse(v)) => v.warnings.push(
                            "numbered line holds a reference; treated as a verse, not a sermon point"
                                .to_string(),
                        ),
                        Some(DocumentItem::Review(_)) => {
                            return Err(ClassifyError::NoWarnings { item: "ReviewItem" })
                        }
                        Some(DocumentItem::Skipped(_)) => {
                            return Err(ClassifyError::NoWarnings {
                                item: "SkippedItem",
                            })
                        }
                        Some(DocumentItem::Point(p)) => p.warnings.push(
                            "numbered line holds a reference; treated as a verse, not a sermon point"
                                .to_string(),
                        ),
                        None => {}
                    }
                }
            }
            Kind::Badref => {
                doc.items.push(review(
                    line,
                    &text,
                    &format!(
                        "looks like a verse but is not one ({})",
                        para.extra.why.clone().unwrap_or_default()
                    ),
                ));
            }
            Kind::Midref => {
                let names = para
                    .refs
                    .iter()
                    .take(3)
                    .map(|r| r.normalized_text.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                match notes.reference_in_text {
                    ReferenceInText::Skip => doc.items.push(skipped(
                        line,
                        SkipCategory::StageNote,
                        &text,
                        &format!("mentions {names} inside text (your setting skips these)"),
                    )),
                    ReferenceInText::Verse => {
                        for reference in &para.refs {
                            doc.items.push(DocumentItem::Verse(VerseItem {
                                line,
                                reference: reference.clone(),
                                warnings: vec![format!(
                                    "taken from the sentence: {}",
                                    py_repr_prefix(&text, 50)
                                )],
                                quoted: String::new(),
                            }));
                        }
                    }
                    ReferenceInText::Review => doc.items.push(review(
                        line,
                        &text,
                        &format!(
                            "mentions {names} inside text; put the reference on its own line to add it as a slide"
                        ),
                    )),
                }
            }
            Kind::Quote => {
                let next = paras.get(index + 1);
                let next_is_verse = next
                    .is_some_and(|n| n.kind == Kind::Verse && n.line as i64 - para.end as i64 <= 3);
                if next_is_verse && notes.quote_above_reference == QuoteAboveReference::Skip {
                    doc.items.push(skipped(
                        line,
                        SkipCategory::QuotedText,
                        &text,
                        "verse text quoted above its reference; text is fetched from the Bible provider",
                    ));
                } else if next_is_verse {
                    doc.items.push(review(
                        line,
                        &text,
                        "verse text quoted above its reference (your setting asks to check these)",
                    ));
                } else if notes.quote_alone == Handling::Skip {
                    doc.items.push(skipped(
                        line,
                        SkipCategory::QuotedText,
                        &text,
                        "quoted text with no reference (your setting skips these)",
                    ));
                } else if notes.quote_alone == Handling::Bullet {
                    doc.items.push(point(
                        line,
                        PointRole::ListItem,
                        "•",
                        &text,
                        vec!["quoted text with no reference made a bullet slide because of your setting".to_string()],
                        line,
                    ));
                } else {
                    doc.items
                        .push(review(line, &text, "quoted text with no reference"));
                }
            }
            Kind::Other => {
                let labelled = rules.labelled.as_ref().is_some_and(|l| l.is_match(&text));
                if labelled && !para.first && notes.label_own_slide {
                    // "Conclusion: Will You Let God Use Your Life?" is a different sentence from the point above it: its own slide
                    doc.items.push(point(
                        line,
                        PointRole::SlideNote,
                        "",
                        &text,
                        Vec::new(),
                        line,
                    ));
                } else {
                    let mut joined = false;
                    if notes.line_breaks == LineBreaks::Paragraph {
                        if let Some(DocumentItem::Point(prior)) = doc.items.last() {
                            if matches!(prior.role, PointRole::Point | PointRole::ListItem)
                                && line as i64 - prior.detail_end as i64 <= 2
                                && ends_with_any(pystr::rstrip(&text), &[".", "?", "!", "”", "\""])
                                && matches!(
                                    other(&para, &text, rules, aliases)?,
                                    DocumentItem::Review(_) | DocumentItem::Point(_)
                                )
                                && para.extra.seen_content
                            {
                                if let Some(DocumentItem::Point(prior)) = doc.items.last_mut() {
                                    prior.text.push('\n');
                                    prior.text.push_str(&text);
                                    prior.detail_end = line;
                                    let note = "joined unlabeled detail line(s) under this point";
                                    if !prior.warnings.iter().any(|w| w == note) {
                                        prior.warnings.push(note.to_string());
                                    }
                                }
                                joined = true;
                            }
                        }
                    }
                    if !joined {
                        doc.items.push(other(&para, &text, rules, aliases)?);
                    }
                }
            }
        }
        trace_outcome(&doc, &para, items_before, prior_text, rules, tracer);
    }
    Ok(doc)
}

/// Python's `repr(text[:50])` (the quotation marks and escapes included).
fn py_repr_prefix(text: &str, count: usize) -> String {
    super::pyre::py_repr(&text.chars().take(count).collect::<String>())
}

/// What this paragraph turned into, and which of your options decided it.
fn trace_outcome(
    doc: &NoteDocument,
    para: &Para,
    before: usize,
    prior_text: Option<String>,
    rules: &Rules,
    tracer: &mut Tracer,
) {
    let made = &doc.items[before..];
    let notes = &rules.notes;
    let mut settings = Map::new();
    let names: &[&str] = match para.kind {
        Kind::Stage => &["stage_cues"],
        Kind::Midref => &["reference_in_text"],
        Kind::Quote => &["quote_above_reference", "quote_alone"],
        Kind::Other => &[
            "line_breaks",
            "unlabeled_sentence",
            "unlabeled_short",
            "before_first_item",
            "header_max_words",
            "headings",
            "caps_lines",
            "wrapped_lines",
            "orphan_max_words",
        ],
        Kind::Verse => &["duplicates"],
        _ => &[],
    };
    for name in names {
        let value = match *name {
            "stage_cues" => enum_value(&notes.stage_cues),
            "reference_in_text" => enum_value(&notes.reference_in_text),
            "quote_above_reference" => enum_value(&notes.quote_above_reference),
            "quote_alone" => enum_value(&notes.quote_alone),
            "line_breaks" => enum_value(&notes.line_breaks),
            "unlabeled_sentence" => enum_value(&notes.unlabeled_sentence),
            "unlabeled_short" => enum_value(&notes.unlabeled_short),
            "before_first_item" => enum_value(&notes.before_first_item),
            "header_max_words" => json!(notes.header_max_words),
            "headings" => enum_value(&notes.headings),
            "caps_lines" => enum_value(&notes.caps_lines),
            "wrapped_lines" => enum_value(&notes.wrapped_lines),
            "orphan_max_words" => json!(notes.orphan_max_words),
            _ => enum_value(&notes.duplicates),
        };
        settings.insert((*name).to_string(), value);
    }
    if para.kind == Kind::Marker {
        settings = Map::new();
        settings.insert(
            "points.numbering".to_string(),
            enum_value(&rules.points.numbering),
        );
        settings.insert(
            "points.require_sequence".to_string(),
            json!(rules.points.require_sequence),
        );
    }
    let mut outcomes: Vec<Value> = made
        .iter()
        .map(|item| match item {
            DocumentItem::Point(p) => {
                json!({"kind": "point", "line": p.line, "role": enum_value(&p.role),
                "label": p.label, "text": p.text, "warnings": p.warnings})
            }
            DocumentItem::Verse(v) => {
                json!({"kind": "verse", "line": v.line, "reference": v.reference.normalized_text,
                "version_written": v.reference.version, "warnings": v.warnings})
            }
            DocumentItem::Skipped(s) => {
                json!({"kind": "skipped", "line": s.line, "category": enum_value(&s.category),
                "reason": s.reason, "text": s.text})
            }
            DocumentItem::Review(r) => {
                json!({"kind": "review", "line": r.line, "reason": r.reason, "text": r.text})
            }
        })
        .collect();
    let merged = made.is_empty() && prior_text.is_some();
    if merged {
        if let Some(last) = doc.items.last() {
            outcomes.push(json!({"kind": "joined", "into_line": last.line(),
                "note": "added to the sermon point above as a detail line (line_breaks=paragraph)"}));
        }
    }
    tracer.trace(
        "item",
        [
            ("line", json!(para.line)),
            ("label", json!(para.kind.as_str())),
            ("settings", Value::Object(settings)),
            ("outcome", Value::Array(outcomes)),
        ],
    );
}

/// Convenience for tests and the oracle runner: classify and keep the trace.
pub fn classify_with_trace(
    text: &str,
    aliases: Aliases,
    notes: Option<&NotesConfig>,
    points: Option<&PointsConfig>,
) -> Result<(NoteDocument, TraceLog), ClassifyError> {
    let mut log = TraceLog::default();
    let document =
        classify_document_traced(text, aliases, notes, points, Shortcuts::default(), &mut log)?;
    Ok((document, log))
}
