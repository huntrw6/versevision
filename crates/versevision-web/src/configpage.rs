//! The Config page: every customisation that matters, read from and written to the saved configuration (`web/configpage.py`).
//!
//! The options, their kinds, choices and limits come from the config model itself ([`versevision_core::config::schema`]: the field
//! types and ranges, written once as data), so a new option shows up on the page as soon as it is in the model, provided it is not
//! in [`HIDDEN`]. The one-line help under each option is [`SHORT`] (the same words as Python's `configpage.SHORT`); an option with no
//! entry there shows no help, and a test fails, so none can be forgotten.

use std::path::Path;

use serde_json::{json, Map, Value};
use versevision_core::config::schema::{Field, Model, Ty};
use versevision_core::config::{schema, AppConfig};
use versevision_core::pystr;

/// (section id, title, what it is, [(model of `AppConfig`, [only these options] or all)])
type Section = (
    &'static str,
    &'static str,
    &'static str,
    &'static [(&'static str, Option<&'static [&'static str]>)],
);

const SECTIONS: &[Section] = &[
    (
        "verse_slides",
        "Verse slides",
        "How a verse is cut and laid out on its slides.",
        &[("slides", None)],
    ),
    (
        "points",
        "Sermon points",
        "What counts as a point and how its slides are written.",
        &[("points", None)],
    ),
    (
        "verses",
        "Verse versions",
        "Which Bible version each verse gets.",
        &[("verses", None)],
    ),
    (
        "notes",
        "Reading the notes",
        "How lines of the document are read: titles, cues, wrapped lines, duplicates.",
        &[("notes", None)],
    ),
    (
        "general",
        "General",
        "Defaults for every import.",
        &[
            ("bible", Some(&["default_version"])),
            ("output", Some(&["library"])),
            ("presentation", Some(&["backup_count"])),
        ],
    ),
];

pub const SHORT: &[(&str, &str)] = &[
    (
        "slides.even_split_punctuation_tolerance",
        "Max size difference between pieces cut at punctuation (0.25 = 25%)",
    ),
    (
        "slides.even_split_tolerance",
        "Max size difference between pieces cut mid-phrase (0.15 = 15%)",
    ),
    (
        "slides.include_verse_numbers",
        "Put the verse number before the verse text",
    ),
    (
        "slides.reference_label_mode",
        "verse: Luke 13:5 · segment: range on the slide · source: as typed",
    ),
    (
        "slides.max_body_lines",
        "A longer verse is cut into near-equal slides",
    ),
    (
        "slides.line_width_units",
        "How wide a line of verse text is (about 48 letters)",
    ),
    (
        "slides.max_reference_lines",
        "A longer reference is flagged in the review",
    ),
    (
        "slides.reference_width_units",
        "How wide the reference line is (about 46 letters)",
    ),
    (
        "slides.prefer_punctuation_breaks",
        "Cut after a comma, semicolon or full stop when close enough",
    ),
    (
        "slides.repeat_reference_on_continuation",
        "Off: only the first slide of a long verse shows it",
    ),
    (
        "points.numbered",
        "Treat \"1)\", \"1.\" and \"#1:\" as points",
    ),
    ("points.lettered", "Treat \"a)\" and \"b.\" as points"),
    ("points.bullets", "Treat \"•\", \"-\" and \"*\" as points"),
    (
        "points.require_sequence",
        "Numbers out of order (1, 2, 5) become plain list items",
    ),
    (
        "points.numbering",
        "Keep the numbers, strip them, or renumber 1, 2, 3",
    ),
    (
        "points.number_style",
        "Used when renumbering: \"{n})\", \"{n}.\", \"#{n}:\"",
    ),
    (
        "points.bullet_char",
        "Mark before a plain point (empty = none)",
    ),
    ("points.case", "Letter case of the point text"),
    (
        "points.fix_label_typos",
        "\"KEY#1)\" is fixed to match the other headers and flagged",
    ),
    (
        "points.strip_trailing_period",
        "Remove the final \".\"; \"!\", \"?\" and \"etc.\" stay",
    ),
    (
        "points.balance_lines",
        "Break long points into evenly wide lines",
    ),
    (
        "points.line_width_units",
        "How wide a line of a point is (about 33 capitals)",
    ),
    (
        "points.line_tolerance",
        "Max difference from the average line width (0.15 = 15%)",
    ),
    (
        "points.line_tolerance_at_punctuation",
        "Same, for a line that starts and ends at punctuation",
    ),
    (
        "points.max_lines_per_slide",
        "A longer point continues on the next slide (0 = no limit)",
    ),
    (
        "points.max_lines",
        "Lines the point box holds before ProPresenter shrinks the text",
    ),
    (
        "points.shown_in_capitals",
        "Measure line widths as capitals (wider letters)",
    ),
    (
        "verses.version_source",
        "A verse with no version: use the one above, the default, or error",
    ),
    (
        "verses.version_from_quote",
        "Use the version whose wording matches the quoted verse text",
    ),
    (
        "verses.quote_versions",
        "Versions tried, in order, when matching a quote",
    ),
    (
        "verses.quote_match",
        "How closely a quote must match a version (0.85 = 85%)",
    ),
    (
        "notes.line_breaks",
        "slide: each line is a slide · paragraph: a sentence joins its point",
    ),
    (
        "notes.unlabeled_sentence",
        "Plain sentence after a point: bullet slide, review, or skip",
    ),
    (
        "notes.unlabeled_short",
        "Short line with no ending punctuation: bullet, review, or skip",
    ),
    (
        "notes.before_first_item",
        "Lines before the first verse or point (title, date, speaker)",
    ),
    (
        "notes.stage_cues",
        "\"Show slide of…\", \"(note)\", web links",
    ),
    (
        "notes.extra_cue_words",
        "More words that start a stage cue, e.g. \"pause\"",
    ),
    (
        "notes.skip_patterns",
        "Regular expressions: a matching line is always left out",
    ),
    (
        "notes.quote_above_reference",
        "Verse text typed above its reference",
    ),
    ("notes.quote_alone", "A quotation with no reference nearby"),
    (
        "notes.reference_in_text",
        "A reference inside a sentence (\"as Psalm 23:1 says…\")",
    ),
    (
        "notes.whole_chapter",
        "A chapter with no verses (\"Psalm 23\")",
    ),
    (
        "notes.duplicates",
        "The same verse and version appearing again later",
    ),
    (
        "notes.label_words",
        "Words that start a labelled line (\"Conclusion: …\")",
    ),
    (
        "notes.label_own_slide",
        "A labelled line gets its own slide",
    ),
    (
        "notes.header_max_words",
        "Capitalized lines this short are headings, not slides",
    ),
    (
        "notes.headings",
        "Titles and headings: a slide with no bullet, or left out",
    ),
    (
        "notes.caps_lines",
        "ALL-CAPS line with no ending punctuation: heading or plain line",
    ),
    (
        "notes.wrapped_lines",
        "join: a line the page cut mid-sentence rejoins the line above",
    ),
    (
        "notes.orphan_max_words",
        "A tail this short after a full line ends the sentence above",
    ),
    (
        "bible.default_version",
        "For a verse with no version written or above it",
    ),
    (
        "output.library",
        "ProPresenter library a new presentation goes in",
    ),
    (
        "presentation.backup_count",
        "Backups kept of a changed presentation",
    ),
];

pub const LABELS: &[(&str, &str)] = &[
    (
        "slides.max_body_lines",
        "Most lines of verse text on a slide",
    ),
    ("slides.max_reference_lines", "Most lines for the reference"),
    ("slides.line_width_units", "Verse line width"),
    ("slides.reference_width_units", "Reference line width"),
    (
        "slides.include_verse_numbers",
        "Show verse numbers in the text",
    ),
    ("slides.reference_label_mode", "Reference shows"),
    (
        "slides.repeat_reference_on_continuation",
        "Repeat the reference on every slide of a long verse",
    ),
    (
        "slides.prefer_punctuation_breaks",
        "Prefer to cut a long verse at punctuation",
    ),
    (
        "slides.even_split_tolerance",
        "Uneven split allowed (mid-phrase)",
    ),
    (
        "slides.even_split_punctuation_tolerance",
        "Uneven split allowed (at punctuation)",
    ),
    (
        "points.max_lines_per_slide",
        "Most lines on a sermon point slide",
    ),
    ("points.max_lines", "Lines a point box holds at full size"),
    ("points.line_width_units", "Point line width"),
    ("points.line_tolerance", "Uneven lines allowed"),
    (
        "points.line_tolerance_at_punctuation",
        "Uneven lines allowed (at punctuation)",
    ),
    (
        "points.strip_trailing_period",
        "Remove the period at the end of a point",
    ),
    (
        "points.fix_label_typos",
        "Fix a header that differs from the others",
    ),
    (
        "points.balance_lines",
        "Break long points into evenly wide lines",
    ),
    ("points.bullet_char", "Bullet mark"),
    ("points.shown_in_capitals", "Points are shown in capitals"),
    ("notes.line_breaks", "Each line break is"),
    ("bible.default_version", "Default Bible version"),
    ("output.library", "Library for new presentations"),
    ("presentation.backup_count", "Backups to keep"),
];

pub const HIDDEN: &[&str] = &[
    "output.swap_keeps_id",
    "points.theme",
    "points.theme_slide",
    "slides.break_on_new_verse",
    "slides.max_characters",
    "slides.max_verses_per_slide",
    "slides.preserve_poetry_lines",
    "slides.reference_format",
    "verses.theme",
    "verses.theme_slide",
];
fn model_of(attr: &str) -> Option<&'static Model> {
    Some(match attr {
        "slides" => &schema::SLIDES,
        "points" => &schema::POINTS,
        "verses" => &schema::VERSES,
        "notes" => &schema::NOTES,
        "bible" => &schema::BIBLE,
        "output" => &schema::OUTPUT,
        "presentation" => &schema::PRESENTATION,
        _ => None?,
    })
}

fn lookup(table: &'static [(&'static str, &'static str)], key: &str) -> Option<&'static str> {
    table.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

/// What kind of control an option gets (`""`: the page cannot edit it).
fn kind_of(ty: &Ty) -> (&'static str, Vec<&'static str>) {
    match ty {
        Ty::Bool => ("bool", Vec::new()),
        Ty::Int { .. } => ("int", Vec::new()),
        Ty::Float { .. } => ("float", Vec::new()),
        Ty::Str | Ty::OptStr => ("text", Vec::new()),
        Ty::Literal(values) => ("choice", values.to_vec()),
        Ty::StrList => ("list", Vec::new()),
        _ => ("", Vec::new()),
    }
}

/// `"max_body_lines"` -> `"Max body lines"` (`str.capitalize()` of the name with spaces).
fn humanize(name: &str) -> String {
    let spaced = name.replace('_', " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
        None => String::new(),
    }
}

/// One option the page can change.
struct Editable {
    key: String,
    section: &'static str,
    attr: &'static str,
    name: &'static str,
    field: &'static Field,
    kind: &'static str,
    choices: Vec<&'static str>,
}

/// Every option the page can change, in the order of the sections and of the model.
fn editable() -> Vec<Editable> {
    let mut out = Vec::new();
    for (section, _title, _help, parts) in SECTIONS {
        for (attr, only) in *parts {
            let Some(model) = model_of(attr) else {
                continue;
            };
            for field in model.fields {
                let key = format!("{attr}.{}", field.name);
                let (kind, choices) = kind_of(&field.ty);
                if kind.is_empty() || HIDDEN.contains(&key.as_str()) {
                    continue;
                }
                if only.is_some_and(|names| !names.contains(&field.name)) {
                    continue;
                }
                out.push(Editable {
                    key,
                    section,
                    attr,
                    name: field.name,
                    field,
                    kind,
                    choices,
                });
            }
        }
    }
    out
}

/// The keys of every option the page can change (for tests and the host).
pub fn editable_keys() -> Vec<String> {
    editable().into_iter().map(|e| e.key).collect()
}

fn bounds(ty: &Ty) -> Vec<(&'static str, Value)> {
    let number = |x: f64| json!(x);
    match ty {
        Ty::Int { ge, le } => {
            let mut out = Vec::new();
            if let Some(v) = ge {
                out.push(("min", json!(v)));
            }
            if let Some(v) = le {
                out.push(("max", json!(v)));
            }
            out
        }
        Ty::Float { gt, ge, le } => {
            let mut out = Vec::new();
            if let Some(v) = ge.or(*gt) {
                out.push(("min", number(v)));
            }
            if let Some(v) = le {
                out.push(("max", number(*v)));
            }
            out
        }
        _ => Vec::new(),
    }
}

fn section_value(config: &Value, attr: &str, name: &str) -> Value {
    config
        .get(attr)
        .and_then(|m| m.get(name))
        .cloned()
        .unwrap_or(Value::Null)
}

/// `describe(config)`: the page's schema with the current values. `path` is where the config file is saved.
pub fn describe(config: &AppConfig, path: &Path) -> Value {
    let current = config.to_value();
    let defaults = AppConfig::default().to_value();
    let all = editable();
    let mut sections = Vec::new();
    for (id, title, help, _) in SECTIONS {
        let mut fields = Vec::new();
        for e in all.iter().filter(|e| e.section == *id) {
            let mut row = Map::new();
            row.insert("key".into(), json!(e.key));
            row.insert(
                "label".into(),
                json!(lookup(LABELS, &e.key)
                    .map(str::to_string)
                    .unwrap_or_else(|| humanize(e.name))),
            );
            row.insert("kind".into(), json!(e.kind));
            row.insert("choices".into(), json!(e.choices));
            row.insert("value".into(), section_value(&current, e.attr, e.name));
            row.insert("default".into(), section_value(&defaults, e.attr, e.name));
            row.insert("help".into(), json!(lookup(SHORT, &e.key).unwrap_or("")));
            for (k, v) in bounds(&e.field.ty) {
                row.insert(k.into(), v);
            }
            fields.push(Value::Object(row));
        }
        sections.push(json!({"id": id, "title": title, "help": help, "fields": fields}));
    }
    json!({"sections": sections, "path": path.to_string_lossy()})
}

/// `str(value)` of a JSON value, as Python prints it.
pub fn py_str(value: &Value) -> String {
    match value {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn py_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|x| x != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

pub fn py_type(value: &Value) -> &'static str {
    py_type_name(value)
}

fn py_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// `int(value)` (digits with an optional sign and underscores between digits, whitespace around; a float is cut towards zero).
pub fn py_int(value: &Value) -> Result<i64, String> {
    match value {
        Value::Bool(b) => Ok(i64::from(*b)),
        Value::Number(n) => match n.as_i64() {
            Some(i) => Ok(i),
            None => {
                let x = n.as_f64().unwrap_or(f64::NAN);
                if x.is_finite() && x.abs() < 9.0e18 {
                    Ok(x.trunc() as i64)
                } else {
                    Err("Bad request: cannot convert float to integer".into())
                }
            }
        },
        Value::String(s) => {
            let t = pystr::strip(s);
            let digits = t.strip_prefix(['+', '-']).unwrap_or(t);
            let ok = !digits.is_empty()
                && !digits.starts_with('_')
                && !digits.ends_with('_')
                && !digits.contains("__")
                && digits.chars().all(|c| c.is_ascii_digit() || c == '_');
            let parsed = ok.then(|| t.replace('_', "").parse::<i64>().ok()).flatten();
            parsed.ok_or_else(|| {
                format!("Bad request: invalid literal for int() with base 10: {}", py_repr(s))
            })
        }
        other => Err(format!(
            "Bad request: int() argument must be a string, a bytes-like object or a real number, not '{}'",
            py_type_name(other)
        )),
    }
}

/// `int(text)` of a key of the page's JSON.
pub fn py_int_text(text: &str) -> Result<i64, String> {
    py_int(&Value::String(text.to_string()))
}

fn py_float(value: &Value) -> Result<f64, String> {
    match value {
        Value::Bool(b) => Ok(f64::from(u8::from(*b))),
        Value::Number(n) => Ok(n.as_f64().unwrap_or(0.0)),
        Value::String(s) => pystr::strip(s)
            .replace('_', "")
            .parse::<f64>()
            .ok()
            .filter(|x| x.is_finite())
            .ok_or_else(|| {
                format!(
                    "Bad request: could not convert string to float: {}",
                    py_repr(s)
                )
            }),
        other => Err(format!(
            "Bad request: float() argument must be a string or a real number, not '{}'",
            py_type_name(other)
        )),
    }
}

/// `repr(text)` for the short texts of an error message.
fn py_repr(text: &str) -> String {
    if text.contains('\'') && !text.contains('"') {
        format!("\"{text}\"")
    } else {
        format!("'{}'", text.replace('\\', "\\\\").replace('\'', "\\'"))
    }
}

/// `apply(config, values)`: the configuration with these option values changed (not saved yet). Unknown or non-editable options are
/// refused; the first validation error is the answer, worded `section.option: message`.
pub fn apply(config: &AppConfig, values: &Map<String, Value>) -> Result<AppConfig, String> {
    let all = editable();
    let mut data = config.to_value();
    for (key, value) in values {
        let Some(e) = all.iter().find(|e| &e.key == key) else {
            return Err(format!("{key} cannot be changed here"));
        };
        let new = match e.kind {
            "list" => {
                let items: Vec<String> = match value {
                    Value::Array(items) => items.iter().map(py_str).collect(),
                    other => pystr::splitlines(&py_str(other))
                        .into_iter()
                        .map(str::to_string)
                        .collect(),
                };
                json!(items
                    .iter()
                    .map(|v| pystr::strip(v))
                    .filter(|v| !v.is_empty())
                    .collect::<Vec<_>>())
            }
            "text" | "choice" => {
                let empty = value.is_null() || pystr::strip(&py_str(value)).is_empty();
                if empty && key == "bible.default_version" {
                    Value::Null
                } else {
                    json!(py_str(value))
                }
            }
            "bool" => json!(py_truthy(value)),
            "int" => json!(py_int(value)?),
            _ => json!(py_float(value)?),
        };
        if let Some(Value::Object(section)) = data.get_mut(e.attr) {
            section.insert(e.name.to_string(), new);
        }
    }
    AppConfig::from_value(&data).map_err(|e| format!("{}: {}", e.location(), e.msg))
}
