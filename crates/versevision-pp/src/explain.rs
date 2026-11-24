//! `explain.py`: read a run's trace back (how did the document become these slides?) and re-run an old document through today's rules.
//!
//! `explain_data` follows every source line to the slides it produced, naming the rule that decided it. `replay_data` parses the saved
//! source again with the options saved beside it (or today's), without fetching Scripture, and lists every line whose decision changed.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};
use versevision_core::bibles::InMemoryBible;
use versevision_core::config::AppConfig;
use versevision_core::notes::trace::NoTrace;
use versevision_core::plan::PlanOptions;
use versevision_core::pystr::{split_ws, splitlines};
use versevision_core::workflow::plan_notes;

use crate::errors::{Kind, VvError, VvResult};
use crate::pyrepr::{pad_left, pad_right, py_eq, py_repr, py_repr_value, py_str, slice_chars};
use crate::runlog;

fn not_found(message: &str) -> VvError {
    VvError::new(Kind::Other("FileNotFoundError"), message)
}

/// `_clip(text, width)`.
pub fn clip(text: &str, width: usize) -> String {
    let flat = split_ws(text).join(" ");
    if flat.chars().count() <= width {
        flat
    } else {
        format!("{}…", slice_chars(&flat, 0, width - 1))
    }
}

fn clip_v(value: &Value, width: usize) -> String {
    clip(&py_str(value), width)
}

/// `load(run_id)`: the run's id, folder and trace.
pub fn load(runs_dir: &Path, run_id: &str) -> VvResult<(String, PathBuf, Vec<Value>)> {
    let wanted = if run_id.is_empty() { "latest" } else { run_id };
    let Some(found) = runlog::resolve(runs_dir, wanted) else {
        return Err(not_found("No such run. Try: versevision log list"));
    };
    let folder = runs_dir.join(&found);
    let rows = runlog::read_trace(runs_dir, &found);
    Ok((found, folder, rows))
}

fn section_defaults(section: &str) -> Value {
    AppConfig::default().to_value()[section].clone()
}

/// Options that differ from the defaults: those are the ones that explain surprising behaviour.
fn non_default_options(folder: &Path) -> VvResult<Vec<String>> {
    let path = folder.join("options.json");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(&path)?;
    let saved: Value = serde_json::from_str(&text)
        .map_err(|e| VvError::new(Kind::Other("JSONDecodeError"), e.to_string()))?;
    let mut out = Vec::new();
    for section in ["notes", "points", "slides", "verses"] {
        let defaults = section_defaults(section);
        let Some(values) = saved.get(section).and_then(Value::as_object) else {
            continue;
        };
        for (key, value) in values {
            let default = defaults.get(key).cloned().unwrap_or(Value::Null);
            if !py_eq(&default, value) {
                out.push(format!(
                    "{section}.{key} = {}  (default {})",
                    py_repr_value(value),
                    py_repr_value(&default)
                ));
            }
        }
    }
    Ok(out)
}

fn row_str<'a>(row: &'a Value, key: &str) -> &'a str {
    row.get(key).and_then(Value::as_str).unwrap_or("")
}

fn line_of(row: &Value, key: &str) -> i64 {
    row.get(key).and_then(Value::as_i64).unwrap_or(0)
}

/// Everything the trace knows, organised by source line (the structure `--json` prints and the text view is drawn from).
pub fn explain_data(
    runs_dir: &Path,
    run_id: &str,
    line: Option<i64>,
    tier: Option<&str>,
) -> VvResult<Value> {
    let (found, folder, rows) = load(runs_dir, run_id)?;
    let fresh = || -> Map<String, Value> {
        let mut m = Map::new();
        for key in [
            "joins",
            "versions",
            "fetches",
            "splits",
            "points",
            "decisions",
        ] {
            m.insert(key.into(), json!([]));
        }
        m
    };
    let mut by_line: BTreeMap<i64, Map<String, Value>> = BTreeMap::new();
    let mut paragraphs: Vec<Value> = Vec::new();
    let mut singles: Vec<(String, Vec<Value>)> = Vec::new();
    let push = |entry: &mut Map<String, Value>, key: &str, row: &Value| {
        if let Some(Value::Array(list)) = entry.get_mut(key) {
            list.push(row.clone());
        }
    };
    for row in &rows {
        let stage = row_str(row, "stage");
        match stage {
            "paragraph" => {
                paragraphs.push(row.clone());
                by_line
                    .entry(line_of(row, "line"))
                    .or_insert_with(fresh)
                    .insert("paragraph".into(), row.clone());
            }
            "line_join" => push(
                by_line
                    .entry(line_of(row, "into_paragraph"))
                    .or_insert_with(fresh),
                "joins",
                row,
            ),
            "item" => {
                by_line
                    .entry(line_of(row, "line"))
                    .or_insert_with(fresh)
                    .insert("item".into(), row.clone());
            }
            "version" => push(
                by_line.entry(line_of(row, "line")).or_insert_with(fresh),
                "versions",
                row,
            ),
            "fetch_result" => push(
                by_line.entry(line_of(row, "line")).or_insert_with(fresh),
                "fetches",
                row,
            ),
            "split" => push(
                by_line.entry(line_of(row, "line")).or_insert_with(fresh),
                "splits",
                row,
            ),
            "point_format" => push(
                by_line.entry(line_of(row, "line")).or_insert_with(fresh),
                "points",
                row,
            ),
            "decision" => push(
                by_line.entry(line_of(row, "line")).or_insert_with(fresh),
                "decisions",
                row,
            ),
            other => match singles.iter_mut().find(|(k, _)| k == other) {
                Some((_, list)) => list.push(row.clone()),
                None => singles.push((other.to_string(), vec![row.clone()])),
            },
        }
    }
    let providers: Vec<&Value> = singles
        .iter()
        .find(|(k, _)| k == "provider")
        .map(|(_, v)| v.iter().collect())
        .unwrap_or_default();
    let provider_for = |reference: &Value, version: &Value| -> Value {
        // a dict keyed by (reference, version): the last row wins
        providers
            .iter()
            .rev()
            .find(|r| {
                r.get("reference") == Some(reference)
                    && r.get("version").unwrap_or(&Value::Null) == version
            })
            .map(|r| (*r).clone())
            .unwrap_or(Value::Null)
    };
    for entry in by_line.values_mut() {
        if let Some(Value::Array(fetches)) = entry.get_mut("fetches") {
            for f in fetches.iter_mut() {
                let prov = provider_for(
                    f.get("reference").unwrap_or(&Value::Null),
                    f.get("version_asked").unwrap_or(&Value::Null),
                );
                if let Value::Object(map) = f {
                    map.insert("provider".into(), prov);
                }
            }
        }
    }
    let source_path = folder.join("source.txt");
    let source: Vec<String> = match fs::read(&source_path) {
        Ok(bytes) => splitlines(&String::from_utf8_lossy(&bytes))
            .into_iter()
            .map(str::to_string)
            .collect(),
        Err(_) => Vec::new(),
    };
    let mut covered = std::collections::BTreeSet::new();
    for p in &paragraphs {
        for n in line_of(p, "line")..=line_of(p, "end_line") {
            covered.insert(n);
        }
    }
    let uncovered: Vec<i64> = source
        .iter()
        .enumerate()
        .filter(|(i, text)| !text.trim().is_empty() && !covered.contains(&(*i as i64 + 1)))
        .map(|(i, _)| i as i64 + 1)
        .collect();
    let mut lines_out = Vec::new();
    for p in &paragraphs {
        let entry = by_line
            .get(&line_of(p, "line"))
            .cloned()
            .unwrap_or_else(fresh);
        let decisions = entry["decisions"].as_array().cloned().unwrap_or_default();
        let tiers: Vec<&str> = decisions.iter().map(|d| row_str(d, "tier")).collect();
        let worst = if tiers.contains(&"error") {
            "error"
        } else if tiers.contains(&"warning") {
            "warning"
        } else {
            "good"
        };
        if let Some(n) = line {
            if !(line_of(p, "line") <= n && n <= line_of(p, "end_line")) {
                continue;
            }
        }
        if let Some(t) = tier.filter(|t| !t.is_empty()) {
            if worst != t {
                continue;
            }
        }
        let mut out = Map::new();
        out.insert("line".into(), p["line"].clone());
        out.insert("end_line".into(), p["end_line"].clone());
        out.insert("worst_tier".into(), json!(worst));
        out.extend(entry);
        lines_out.push(Value::Object(out));
    }
    let events = runlog::read_events(runs_dir, &found);
    let start = events
        .iter()
        .find(|e| e["step"] == "run_start")
        .cloned()
        .unwrap_or(json!({}));
    let mut data = Map::new();
    data.insert("run".into(), json!(found));
    data.insert("start".into(), start);
    data.insert("folder".into(), json!(folder.to_string_lossy()));
    data.insert(
        "options_changed".into(),
        json!(non_default_options(&folder)?),
    );
    data.insert("lines".into(), Value::Array(lines_out));
    data.insert("uncovered_source_lines".into(), json!(uncovered));
    for (k, v) in singles {
        data.insert(k, Value::Array(v));
    }
    Ok(Value::Object(data))
}

fn tier_mark(tier: &str) -> &'static str {
    match tier {
        "good" => "ok",
        "warning" => "CHECK",
        _ => "ERROR",
    }
}

fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

fn g(v: &Value, key: &str) -> String {
    py_str(v.get(key).unwrap_or(&Value::Null))
}

fn arr(v: &Value, key: &str) -> Vec<Value> {
    v.get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// `render_explain`.
pub fn render_explain(
    runs_dir: &Path,
    run_id: &str,
    line: Option<i64>,
    tier: Option<&str>,
    width: usize,
) -> VvResult<String> {
    let d = explain_data(runs_dir, run_id, line, tier)?;
    let mut out: Vec<String> = Vec::new();
    let start = d["start"].clone();
    out.push(format!(
        "HOW THE DOCUMENT BECAME SLIDES  -  run {}",
        g(&d, "run")
    ));
    out.push("=".repeat(78));
    out.push(format!(
        "Document: {}    Action: {}    VerseVision {}",
        g(&start, "source"),
        g(&start, "mode"),
        g(&start, "application_version")
    ));
    let start_of = |key: &str| arr(&d, key);
    for row in start_of("extract") {
        out.push(format!(
            "Read as:  {}  ->  {} characters, {} lines ({} blank)",
            g(&row, "method"),
            g(&row, "characters"),
            g(&row, "lines"),
            g(&row, "blank_lines")
        ));
    }
    for row in start_of("parse_start") {
        let layout = if truthy(row.get("hard_wrapped")) {
            format!(
                "HARD-WRAPPED text (PDF/email style): a line that looks {}+ wide (capitals count extra) and has not ended its sentence is joined to the next one",
                g(&row, "full_line_width")
            )
        } else {
            "lines are treated as written (no hard-wrap detected)".to_string()
        };
        out.push(format!(
            "Layout:   {} text lines; {layout}",
            g(&row, "non_blank")
        ));
    }
    for row in start_of("look") {
        let or_template = |k: &str| {
            if truthy(row.get(k)) {
                g(&row, k)
            } else {
                "VerseVision template".to_string()
            }
        };
        out.push(format!(
            "Look:     verses = {};  points = {}",
            or_template("verse"),
            or_template("point")
        ));
    }
    for row in start_of("review_edit") {
        let changes: Vec<String> = arr(&row, "changes")
            .iter()
            .map(|c| {
                format!(
                    "line {}: {} -> {}",
                    g(c, "line"),
                    py_repr(&clip_v(&c["before"], 40)),
                    py_repr(&clip_v(&c["after"], 40))
                )
            })
            .collect();
        out.push(format!(
            "Review:   edited after run {}: {}",
            g(&row, "parent_run"),
            changes.join("; ")
        ));
    }
    let changed: Vec<String> = arr(&d, "options_changed").iter().map(py_str).collect();
    out.push(format!(
        "Options that are not the defaults: {}",
        if changed.is_empty() {
            "none (house style)"
        } else {
            ""
        }
    ));
    for c in &changed {
        out.push(format!("   {c}"));
    }
    out.push(String::new());
    for entry in arr(&d, "lines") {
        let (a, b) = (line_of(&entry, "line"), line_of(&entry, "end_line"));
        let p = entry["paragraph"].clone();
        let where_ = if a == b {
            format!("line {a}")
        } else {
            format!("lines {a}-{b}")
        };
        out.push(format!(
            "[{}] {where_}",
            pad_right(tier_mark(row_str(&entry, "worst_tier")), 5)
        ));
        for raw in arr(&p, "raw_lines") {
            out.push(format!("        | {}", clip_v(&raw, width)));
        }
        let joins = arr(&entry, "joins");
        if !joins.is_empty() {
            let text = joins
                .iter()
                .map(|j| format!("line {} ({})", g(j, "line"), g(j, "why")))
                .collect::<Vec<_>>()
                .join("; ");
            out.push(format!("        joined: {}", clip(&text, width)));
        }
        let rule = if truthy(p.get("rule")) {
            g(&p, "rule")
        } else {
            g(&p, "label")
        };
        out.push(format!(
            "        rule: {rule}  ->  read as '{}'{}",
            g(&p, "label"),
            if truthy(p.get("why")) {
                format!("  [{}]", g(&p, "why"))
            } else {
                String::new()
            }
        ));
        for r in arr(&p, "references") {
            let version = if truthy(r.get("version_written")) {
                format!(" (version written: {})", g(&r, "version_written"))
            } else {
                String::new()
            };
            let warnings = arr(&r, "warnings");
            let warn = if warnings.is_empty() {
                String::new()
            } else {
                format!(
                    "  warnings: {}",
                    warnings.iter().map(py_str).collect::<Vec<_>>().join("; ")
                )
            };
            out.push(format!(
                "        reference: {} -> {}{version}{warn}",
                py_repr(&g(&r, "as_written")),
                g(&r, "normalized")
            ));
        }
        if let Some(item) = entry.get("item") {
            if let Some(settings) = item
                .get("settings")
                .and_then(Value::as_object)
                .filter(|s| !s.is_empty())
            {
                let text = settings
                    .iter()
                    .map(|(k, v)| format!("{k}={}", py_str(v)))
                    .collect::<Vec<_>>()
                    .join(", ");
                out.push(format!("        your options that applied: {text}"));
            }
            for o in arr(item, "outcome") {
                let detail = match row_str(&o, "kind") {
                    "point" => format!(
                        "POINT ({}) label {}",
                        g(&o, "role"),
                        py_repr_value(o.get("label").unwrap_or(&Value::Null))
                    ),
                    "verse" => format!("VERSE {}", g(&o, "reference")),
                    "skipped" => format!("LEFT OUT ({}): {}", g(&o, "category"), g(&o, "reason")),
                    "review" => format!("NEEDS A DECISION: {}", g(&o, "reason")),
                    "joined" => format!(
                        "JOINED into the item at line {}: {}",
                        g(&o, "into_line"),
                        g(&o, "note")
                    ),
                    other => other.to_string(),
                };
                out.push(format!("        became: {detail}"));
                for w in arr(&o, "warnings") {
                    out.push(format!("                note: {}", clip_v(&w, width)));
                }
            }
        }
        for v in arr(&entry, "versions") {
            let note = if truthy(v.get("note")) {
                format!("  ({})", g(&v, "note"))
            } else {
                String::new()
            };
            out.push(format!(
                "        version: {}  - {}{note}",
                g(&v, "version"),
                g(&v, "how")
            ));
        }
        for f in arr(&entry, "fetches") {
            let prov = f
                .get("provider")
                .filter(|p| truthy(Some(p)))
                .cloned()
                .unwrap_or(json!({}));
            let via = if truthy(prov.get("provider")) {
                format!(
                    " via {} ({}, {} ms)",
                    g(&prov, "provider"),
                    g(&prov, "source"),
                    g(&prov, "ms")
                )
            } else if truthy(prov.get("error")) {
                format!(" FAILED: {}", g(&prov, "error"))
            } else {
                String::new()
            };
            let first = if truthy(f.get("first_verse")) {
                format!("; first: {}", py_repr(&clip_v(&f["first_verse"], 60)))
            } else {
                String::new()
            };
            out.push(format!(
                "        Scripture: asked {}, got {}{via}: {} verse(s), {} characters{first}",
                g(&f, "version_asked"),
                g(&f, "version_returned"),
                g(&f, "verses"),
                g(&f, "characters")
            ));
        }
        for s in arr(&entry, "splits") {
            let verses = arr(&s, "verses");
            let multi: Vec<&Value> = verses
                .iter()
                .filter(|v| v.get("slides").and_then(Value::as_i64).unwrap_or(0) > 1)
                .collect();
            let tail = if multi.is_empty() {
                "; each fits on one slide".to_string()
            } else {
                format!(
                    "; {} verse(s) needed cutting (max {} lines per slide)",
                    multi.len(),
                    py_str(&s["settings"]["max_body_lines"])
                )
            };
            out.push(format!(
                "        slides: {} for {} verse(s){tail}",
                g(&s, "slides"),
                verses.len()
            ));
            for v in multi {
                let pieces = arr(v, "pieces")
                    .iter()
                    .map(|x| clip_v(x, 50))
                    .collect::<Vec<_>>()
                    .join(" | ");
                out.push(format!(
                    "                verse {}: {} wrapped lines -> {} slides: {pieces}",
                    g(v, "verse"),
                    g(v, "wrapped_lines"),
                    g(v, "slides")
                ));
            }
        }
        for pf in arr(&entry, "points") {
            out.push(format!(
                "        slide text: {}   (numbering={}, case={})",
                py_repr(&clip_v(&pf["slide_text"], width)),
                py_str(&pf["settings"]["numbering"]),
                py_str(&pf["settings"]["case"])
            ));
        }
        for dec in arr(&entry, "decisions") {
            let n = arr(&dec, "slides");
            let slides = if n.len() > 1 {
                format!(" slides {}-{}", py_str(&n[0]), py_str(&n[n.len() - 1]))
            } else if n.len() == 1 {
                format!(" slide {}", py_str(&n[0]))
            } else {
                " (no slide)".to_string()
            };
            let reasons = arr(&dec, "reasons");
            let because = if reasons.is_empty() {
                String::new()
            } else {
                format!(
                    "  because: {}",
                    clip(
                        &reasons.iter().map(py_str).collect::<Vec<_>>().join("; "),
                        width
                    )
                )
            };
            out.push(format!(
                "        => {} [{}]{slides}{because}",
                g(&dec, "kind"),
                g(&dec, "tier")
            ));
        }
        out.push(String::new());
    }
    let uncovered = arr(&d, "uncovered_source_lines");
    if !uncovered.is_empty() {
        let shown: Vec<String> = uncovered.iter().take(30).map(py_str).collect();
        out.push(format!(
            "!! Source lines the parser never looked at: [{}]  (this should be empty; tell me)",
            shown.join(", ")
        ));
    }
    for row in start_of("review_excluded") {
        let list = arr(&row, "decisions")
            .iter()
            .map(|x| {
                format!(
                    "line {} {}",
                    g(x, "line"),
                    py_repr(&clip_v(&x["title"], 40))
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        out.push(format!("You switched these off in the review: {list}"));
    }
    for row in start_of("output") {
        let name = if truthy(row.get("presentation_name")) {
            g(&row, "presentation_name")
        } else {
            g(&row, "target_name")
        };
        out.push(format!(
            "Written to ProPresenter: {name}  ({} slides added; the file has {}; verified: {})",
            g(&row, "slide_count"),
            g(&row, "cue_count_after"),
            g(&row, "verified")
        ));
    }
    let plan = arr(&d, "plan").last().cloned();
    if let Some(plan) = plan.filter(|p| truthy(Some(p))) {
        if line.is_none() && tier.is_none() {
            out.push(String::new());
            out.push(format!(
                "FINAL SLIDES ({}: {} verse, {} point)",
                g(&plan, "slide_count"),
                g(&plan, "verse_slides"),
                g(&plan, "point_slides")
            ));
            out.push("-".repeat(78));
            for s in arr(&plan, "slides") {
                let who = if truthy(s.get("reference")) {
                    g(&s, "reference")
                } else {
                    "point".to_string()
                };
                out.push(format!(
                    "{}. {} {} {}",
                    pad_left(&g(&s, "n"), 4),
                    pad_right(&g(&s, "kind"), 5),
                    pad_right(&clip(&who, 28), 28),
                    py_repr(&clip_v(&s["text"], 90))
                ));
            }
        }
    }
    Ok(out.join("\n") + "\n")
}

// ---------------------------------------------------------------------------------------------- replay

fn decisions_from_trace(rows: &[Value]) -> Vec<Value> {
    rows.iter()
        .filter(|r| r["stage"] == "decision")
        .cloned()
        .collect()
}

fn decision_key(x: &Value) -> (i64, String, String) {
    (
        line_of(x, "line"),
        row_str(x, "kind").to_string(),
        row_str(x, "title").to_string(),
    )
}

fn network_only(x: &Value) -> bool {
    row_str(x, "tier") == "error"
        && arr(x, "reasons")
            .iter()
            .any(|r| r.as_str().is_some_and(|s| s.starts_with("not added:")))
}

/// Parse the saved source again (no Scripture fetched) and compare every decision with what that run decided.
pub fn replay_data(
    runs_dir: &Path,
    config: &AppConfig,
    run_id: &str,
    use_saved_options: bool,
) -> VvResult<Value> {
    let (found, folder, rows) = load(runs_dir, run_id)?;
    let source_file = folder.join("source.txt");
    if !source_file.exists() {
        return Err(not_found(&format!("run {found} has no saved source.txt")));
    }
    let saved: Option<Value> = if folder.join("options.json").exists() {
        let text = fs::read_to_string(folder.join("options.json"))?;
        Some(
            serde_json::from_str(&text)
                .map_err(|e| VvError::new(Kind::Other("JSONDecodeError"), e.to_string()))?,
        )
    } else {
        None
    };
    let saved_used = saved.as_ref().is_some_and(|s| truthy(Some(s))) && use_saved_options;
    let mut config = config.clone();
    if let (Some(saved), true) = (saved.as_ref(), saved_used) {
        let mut value = config.to_value();
        for section in ["notes", "points", "slides", "verses"] {
            value[section] = saved
                .get(section)
                .cloned()
                .ok_or_else(|| VvError::new(Kind::Other("KeyError"), format!("'{section}'")))?;
        }
        config = AppConfig::from_value(&value).map_err(|e| {
            VvError::new(
                Kind::Other("ValidationError"),
                format!("{}: {}", e.location(), e.msg),
            )
        })?;
    }
    let default_version = saved
        .as_ref()
        .and_then(|s| s.get("default_version_used"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let options = PlanOptions {
        default_version,
        no_fetch: true,
    };
    let text = String::from_utf8_lossy(&fs::read(&source_file)?).into_owned();
    let result = pollster::block_on(plan_notes::<InMemoryBible>(
        &text,
        &config,
        &options,
        None,
        &mut NoTrace,
    ))?;
    let mut old = decisions_from_trace(&rows);
    if old.is_empty() {
        // a run from before traces existed: use its report
        if let Ok(bytes) = fs::read(folder.join("report.json")) {
            if let Ok(report) = serde_json::from_slice::<Value>(&bytes) {
                old = arr(&report, "decisions")
                    .iter()
                    .map(|x| {
                        json!({"line": x["line"], "kind": x["kind"], "tier": x.get("tier").cloned().unwrap_or(json!("good")),
                               "title": x["title"], "reasons": x.get("reasons").cloned().unwrap_or(json!([]))})
                    })
                    .collect();
            }
        }
    }
    let new: Vec<Value> = result
        .decisions
        .iter()
        .map(|x| {
            json!({"line": x.line, "kind": serde_json::to_value(x.kind).unwrap_or(Value::Null), "tier": serde_json::to_value(x.tier).unwrap_or(Value::Null),
                   "title": x.title, "reasons": x.reasons})
        })
        .collect();
    let mut old_map: BTreeMap<(i64, String, String), &Value> = BTreeMap::new();
    for x in &old {
        old_map.insert(decision_key(x), x);
    }
    let mut new_map: BTreeMap<(i64, String, String), &Value> = BTreeMap::new();
    for x in &new {
        new_map.insert(decision_key(x), x);
    }
    let mut keys: Vec<&(i64, String, String)> = old_map.keys().chain(new_map.keys()).collect();
    keys.sort_by(|a, b| (a.0, &a.1, &a.2).cmp(&(b.0, &b.1, &b.2)));
    keys.dedup();
    let mut diffs = Vec::new();
    for k in keys {
        match (old_map.get(k), new_map.get(k)) {
            (Some(o), Some(n)) => {
                if row_str(o, "tier") != row_str(n, "tier") && !network_only(o) {
                    diffs.push(json!({"line": k.0, "title": k.2,
                        "change": format!("tier {} -> {}", row_str(o, "tier"), row_str(n, "tier")),
                        "before": o["reasons"], "after": n["reasons"]}));
                }
            }
            (Some(o), None) => {
                if !network_only(o) {
                    diffs.push(json!({"line": k.0, "title": k.2,
                        "change": format!("was {} [{}]; no longer produced", row_str(o, "kind"), row_str(o, "tier")),
                        "before": o["reasons"], "after": []}));
                }
            }
            (None, Some(n)) => {
                diffs.push(json!({"line": k.0, "title": k.2,
                    "change": format!("now {} [{}]; was not there", row_str(n, "kind"), row_str(n, "tier")),
                    "before": [], "after": n["reasons"]}));
            }
            (None, None) => {}
        }
    }
    Ok(json!({
        "run": found,
        "options": if saved_used { "saved with the run" } else { "today's" },
        "decisions_then": old.len(),
        "decisions_now": new.len(),
        "differences": diffs,
    }))
}

/// `render_replay`.
pub fn render_replay(data: &Value) -> String {
    let diffs = arr(data, "differences");
    let mut out = vec![
        format!(
            "REPLAY of run {} with {} options (Scripture is not fetched, so only the parsing is compared)",
            g(data, "run"),
            g(data, "options")
        ),
        format!(
            "decisions then: {}   now: {}   changed: {}",
            g(data, "decisions_then"),
            g(data, "decisions_now"),
            diffs.len()
        ),
    ];
    for d in &diffs {
        out.push(format!(
            "  line {}  {}: {}",
            pad_left(&g(d, "line"), 4),
            py_repr(&clip_v(&d["title"], 50)),
            g(d, "change")
        ));
        for r in arr(d, "after").iter().take(3) {
            out.push(format!("            now: {}", clip_v(r, 120)));
        }
    }
    if diffs.is_empty() {
        out.push("  Nothing changed: today's rules decide every line the same way.".to_string());
    }
    out.join("\n") + "\n"
}
