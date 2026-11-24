//! Which Bible version a verse is read in: `resolve_note_versions` (its own, else the nearest explicit one above, else the
//! default), and `check_version_against_quote` (the notes quote the verse without a version: take the version whose wording
//! matches the quote).

use serde_json::{json, Map, Value};

use crate::bibles::{BiblePassages, ResolvedPassage};
use crate::config::{AppConfig, VersionFromQuote, VersionSource};
use crate::notes::models::VerseItem;
use crate::notes::trace::emit;
use crate::notes::{NoteDocument, TraceSink};
use crate::plan::ProblemEntry;
use crate::pyfmt::{fixed, percent0, round2};
use crate::pystr::strip;
use crate::references::pytext::casefold;
use crate::references::PassageReference;
use crate::trace_fields;
use crate::versions::{canonical_version, is_comparison_pair, quote_score};

/// What `resolve_note_versions` decided for one verse: the reference with its version, or why there is none.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedVerse {
    pub line: usize,
    pub reference: Option<PassageReference>,
    /// Why the verse has no reference (empty when it has one).
    pub why: String,
}

/// `resolve_note_versions`: give every verse a version: its own, else the nearest explicit one above (`verses.version_source =
/// above`), else the default. Writes the `version` trace record of every verse.
pub fn resolve_note_versions(
    document: &NoteDocument,
    default: Option<&str>,
    source: VersionSource,
    trace: &mut dyn TraceSink,
) -> (Vec<ResolvedVerse>, Vec<ProblemEntry>) {
    let mut notices = Vec::new();
    let mut resolved = Vec::new();
    let mut last_explicit: Option<String> = None;
    let default_name = default
        .filter(|d| !d.is_empty())
        .and_then(canonical_version);
    let verses = document.verses();
    let references: Vec<&PassageReference> = verses.iter().map(|v| &v.reference).collect();
    let source_name = match source {
        VersionSource::Above => "above",
        VersionSource::Default => "default",
        VersionSource::Required => "required",
    };
    let mut last_explicit_line = 0;
    for (index, item) in verses.iter().enumerate() {
        let reference = &item.reference;
        for warning in &item.warnings {
            if warning.contains("read as") || warning.contains("unrecognized") {
                let suffix = if warning.contains("unrecognized") {
                    "; ignored, so the verse uses the nearest version above or the default"
                } else {
                    ""
                };
                notices.push(ProblemEntry {
                    line: item.line,
                    text: strip(&reference.source_text).to_string(),
                    problem: format!("{warning}{suffix}"),
                });
            }
        }
        if let Some(version) = reference.version.as_deref().filter(|v| !v.is_empty()) {
            let pair = is_comparison_pair(&references, index);
            if !pair {
                // a comparison pair never sets the version for later verses
                last_explicit = Some(version.to_string());
                last_explicit_line = item.line;
            }
            emit(
                trace,
                "version",
                trace_fields! {
                    "line" => item.line,
                    "reference" => reference.normalized_text,
                    "version" => version,
                    "how" => "written on the verse",
                    "sets_version_for_later_verses" => !pair,
                    "note" => if pair { "same passage appears next to it in another version: a comparison pair, so it does not set the version for later verses" } else { "" },
                },
            );
            resolved.push(ResolvedVerse {
                line: item.line,
                reference: Some(reference.clone()),
                why: String::new(),
            });
            continue;
        }
        if source == VersionSource::Required {
            emit(
                trace,
                "version",
                trace_fields! {
                    "line" => item.line,
                    "reference" => reference.normalized_text,
                    "version" => Value::Null,
                    "how" => "none written and verses.version_source=required: not added",
                },
            );
            resolved.push(ResolvedVerse {
                line: item.line,
                reference: None,
                why: "no version on this verse, and your setting requires one (verses.version_source = required)".to_string(),
            });
            continue;
        }
        let above = source == VersionSource::Above;
        let chosen: Option<String> = if above {
            last_explicit
                .clone()
                .or_else(|| default_name.map(str::to_string))
        } else {
            default_name.map(str::to_string)
        };
        let from_above = above && last_explicit.is_some();
        emit(
            trace,
            "version",
            trace_fields! {
                "line" => item.line,
                "reference" => reference.normalized_text,
                "version" => chosen,
                "how" => if from_above { format!("none written; used the nearest version above (line {last_explicit_line})") } else { "none written; used the default version".to_string() },
                "version_source" => source_name,
                "default_version" => default_name,
                "last_explicit_version" => last_explicit,
            },
        );
        let Some(chosen) = chosen.filter(|c| !c.is_empty()) else {
            resolved.push(ResolvedVerse {
                line: item.line,
                reference: None,
                why: if above {
                    "no version on this verse, none above it, and no default version is set"
                } else {
                    "no version on this verse and no default version is set"
                }
                .to_string(),
            });
            continue;
        };
        let how = if from_above {
            "nearest version above"
        } else {
            "default version"
        };
        notices.push(ProblemEntry {
            line: item.line,
            text: reference.normalized_text.clone(),
            problem: format!("no version listed; used {chosen} ({how})"),
        });
        let mut with_version = reference.clone();
        with_version.version = Some(chosen);
        resolved.push(ResolvedVerse {
            line: item.line,
            reference: Some(with_version),
            why: String::new(),
        });
    }
    (resolved, notices)
}

/// `check_version_against_quote`: the notes quote this verse and did not name a version. If the quote does not match the version it
/// was given (the version above or the default) but does match another version's wording, use that version. Returns the reference
/// and passage to use; `reasons` gets the explanation.
pub async fn check_version_against_quote<B: BiblePassages + ?Sized>(
    item: &VerseItem,
    reference: PassageReference,
    passage: ResolvedPassage,
    registry: &B,
    config: &AppConfig,
    reasons: &mut Vec<String>,
    trace: &mut dyn TraceSink,
) -> (PassageReference, ResolvedPassage) {
    let cfg = &config.verses;
    if cfg.version_from_quote != VersionFromQuote::Use
        || item.quoted.is_empty()
        || item
            .reference
            .version
            .as_deref()
            .is_some_and(|v| !v.is_empty())
    {
        return (reference, passage);
    }
    let current = reference.version.clone().unwrap_or_default();
    let texts =
        |p: &ResolvedPassage| -> Vec<String> { p.verses.iter().map(|v| v.text.clone()).collect() };
    let score_of = |p: &ResolvedPassage| -> f64 {
        let texts = texts(p);
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        quote_score(&item.quoted, &refs)
    };
    let base = score_of(&passage);
    let mut tried = Map::new();
    tried.insert(casefold(&current), json!(round2(base)));
    let common = |fields: &mut Map<String, Value>| {
        fields.insert("line".into(), json!(item.line));
        fields.insert("reference".into(), json!(item.reference.normalized_text));
        fields.insert("quoted".into(), json!(item.quoted));
        fields.insert("version".into(), json!(current));
    };
    if base >= cfg.quote_match {
        let mut fields = Map::new();
        common(&mut fields);
        fields.insert("score".into(), json!(round2(base)));
        fields.insert("result".into(), json!("the quote matches this version"));
        emit(trace, "quote_check", fields);
        return (reference, passage);
    }
    let mut best: Option<(f64, PassageReference, ResolvedPassage)> = None;
    for name in &cfg.quote_versions {
        let Some(canon) = canonical_version(name).filter(|c| !c.is_empty()) else {
            continue;
        };
        if casefold(canon) == casefold(&current) {
            continue;
        }
        let mut trial_ref = reference.clone();
        trial_ref.version = Some(canon.to_string());
        let Ok(trial) = registry.fetch(&trial_ref, trace).await else {
            continue;
        };
        let score = score_of(&trial);
        tried.insert(casefold(canon), json!(round2(score)));
        if score > best.as_ref().map_or(0.0, |b| b.0) {
            best = Some((score, trial_ref, trial));
        }
        if score >= 0.95 {
            break;
        }
    }
    if let Some((score, best_ref, best_passage)) = best {
        if score >= cfg.quote_match && score >= base + 0.1 {
            let used = best_ref.version.clone().unwrap_or_default();
            let mut fields = Map::new();
            common(&mut fields);
            fields.insert("tried".into(), Value::Object(tried));
            fields.insert(
                "result".into(),
                json!(format!(
                    "the notes quote {used} wording (score {} vs {} for {current}): used {used}",
                    fixed(score, 2),
                    fixed(base, 2)
                )),
            );
            emit(trace, "quote_check", fields);
            // superseded: the version came from the quote
            reasons.retain(|r| !r.starts_with("no version listed"));
            reasons.push(format!("no version listed; the quoted text matches {used}, not {current} (used {used}; turn off with verses.version_from_quote=off)"));
            return (best_ref, best_passage);
        }
    }
    let mut fields = Map::new();
    common(&mut fields);
    fields.insert("tried".into(), Value::Object(tried));
    fields.insert(
        "result".into(),
        json!(format!(
            "the quote matches none of the versions tried; kept {current}"
        )),
    );
    emit(trace, "quote_check", fields);
    if base < 0.6 {
        reasons.push(format!("the quoted text in the notes does not match {current} wording (match {}); check the version", percent0(base)));
    }
    (reference, passage)
}
