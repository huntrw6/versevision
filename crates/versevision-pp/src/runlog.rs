//! `runlog.py`: one folder per run, so any import can be audited afterwards.
//!
//! ```text
//! <logs>/runs/20261018-091502-3fa9c1/
//!     summary.txt   plain-language account of what happened (read this first)
//!     events.jsonl  every step with a timestamp: document read, each ProPresenter action, errors
//!     trace.jsonl   HOW the document became slides (read it with `explain`)
//!     options.json  the parsing / formatting options in force for the run (so `replay` can re-run the parse)
//!     source.txt    the exact text that was read from the document
//!     report.json   the full plan: every slide, every flagged / adjusted / skipped line
//! ```
//!
//! The files are written the way Python writes them, so the Python app's `explain`, `replay` and `log` read a run this wrote and
//! the other way round. Nothing secret is written: keys, tokens and request URLs are never logged (`clean`).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Map, Value};
use sha1::{Digest as _, Sha1};
use sha2::Sha256;
use versevision_core::notes::trace::{clean_value, TraceSink};
use versevision_core::pystr::splitlines;

use crate::clock::{iso_millis_utc, iso_seconds_utc, stamp, Clock};
use crate::dirs::Dirs;
use crate::errors::VvError;
use crate::pyjson::{dumps, float_repr};
use crate::pyrepr::{pad_left, pad_right, py_repr, py_str, slice_chars};

/// The newest runs kept (`KEEP_RUNS`).
pub const KEEP_RUNS: usize = 200;

pub struct RunLog {
    pub folder: PathBuf,
    pub id: String,
    pub kind: String,
    pub meta: Map<String, Value>,
    runs_dir: PathBuf,
    started: f64,
    seq: u64,
    clock: Arc<dyn Clock>,
}

impl RunLog {
    fn new(
        folder: PathBuf,
        runs_dir: PathBuf,
        run_id: &str,
        kind: &str,
        clock: Arc<dyn Clock>,
    ) -> RunLog {
        RunLog {
            started: clock.monotonic(),
            folder,
            runs_dir,
            id: run_id.to_string(),
            kind: kind.to_string(),
            meta: Map::new(),
            seq: 0,
            clock,
        }
    }

    /// `RunLog.start(kind, **meta)`: a new folder (mode 0700 where the system has modes) and the `run_start` event.
    pub fn start(
        dirs: &Dirs,
        clock: Arc<dyn Clock>,
        app_version: &str,
        kind: &str,
        meta: Map<String, Value>,
    ) -> std::io::Result<RunLog> {
        let now = clock.now_micros();
        let digest = Sha1::digest(clock.random_bytes(8));
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        let run_id = format!("{}-{}", stamp(now, clock.local_offset_seconds()), &hex[..6]);
        let runs_dir = dirs.runs_dir();
        let folder = runs_dir.join(&run_id);
        fs::create_dir_all(&folder)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&folder, fs::Permissions::from_mode(0o700));
        }
        let mut run = RunLog::new(folder, runs_dir, &run_id, kind, clock);
        let mut all = Map::new();
        all.insert("run_id".into(), json!(run_id));
        all.insert("kind".into(), json!(kind));
        all.insert("application_version".into(), json!(app_version));
        all.insert("started".into(), json!(iso_seconds_utc(now)));
        if let Value::Object(cleaned) = clean_value(Value::Object(meta), 400) {
            all.extend(cleaned);
        }
        run.meta = all.clone();
        run.event("run_start", "info", all);
        run.prune();
        Ok(run)
    }

    /// `RunLog.reopen(run_id)`: carry on writing into an earlier run's folder (the trace numbers start again, as in Python).
    pub fn reopen(dirs: &Dirs, clock: Arc<dyn Clock>, run_id: &str) -> Option<RunLog> {
        let runs_dir = dirs.runs_dir();
        let folder = runs_dir.join(run_id);
        if !folder.is_dir() {
            return None;
        }
        let mut run = RunLog::new(folder, runs_dir, run_id, "run", clock);
        run.event("run_resumed", "info", Map::new());
        Some(run)
    }

    /// One line of `events.jsonl`: `{"t", "elapsed", "step", "level", ...data}`. Logging never breaks an import.
    pub fn event(&mut self, step: &str, level: &str, data: Map<String, Value>) {
        let mut line = Map::new();
        line.insert("t".into(), json!(iso_millis_utc(self.clock.now_micros())));
        line.insert(
            "elapsed".into(),
            json!(versevision_core::pyfmt::round2(
                self.clock.monotonic() - self.started
            )),
        );
        line.insert("step".into(), json!(step));
        line.insert("level".into(), json!(level));
        if let Value::Object(cleaned) = clean_value(Value::Object(data), 400) {
            line.extend(cleaned);
        }
        self.append("events.jsonl", &dumps(&Value::Object(line), None, false));
    }

    fn append(&self, file: &str, text: &str) {
        if let Ok(mut stream) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.folder.join(file))
        {
            let _ = stream.write_all(format!("{text}\n").as_bytes());
        }
    }

    fn write(&self, file: &str, text: &str) -> bool {
        fs::write(self.folder.join(file), text).is_ok()
    }

    /// `options.json`: the options in force (cleaned, 2000 characters).
    pub fn save_options(&self, options: Value) {
        let cleaned = clean_value(options, 2000);
        self.write("options.json", &dumps(&cleaned, Some(2), false));
    }

    /// `source.txt` and the `source_saved` event.
    pub fn save_source(&mut self, text: &str, name: &str) {
        if !self.write("source.txt", text) {
            return;
        }
        let sha: String = Sha256::digest(text.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let mut data = Map::new();
        data.insert("file".into(), json!(name));
        data.insert("characters".into(), json!(text.chars().count()));
        data.insert("lines".into(), json!(text.matches('\n').count() + 1));
        data.insert("sha256".into(), json!(sha));
        self.event("source_saved", "info", data);
    }

    pub fn save_report(&self, report: &Value) {
        self.write("report.json", &dumps(report, Some(2), false));
    }

    /// `failed(exc)`.
    pub fn failed(&mut self, error: &VvError) {
        let mut data = Map::new();
        data.insert("error".into(), json!(error.message));
        data.insert("error_type".into(), json!(error.python_name()));
        data.insert(
            "traceback".into(),
            json!(format!("{}: {}\n", error.python_name(), error.message)),
        );
        self.event("run_failed", "error", data);
    }

    /// `finish(outcome, report, applied, error)`: the `output` trace record, `run_end` and `summary.txt`.
    pub fn finish(
        &mut self,
        outcome: &str,
        report: Option<&Value>,
        applied: Option<&Value>,
        error: Option<&str>,
    ) {
        let bad = outcome == "failed" || error.is_some_and(|e| !e.is_empty());
        if let Some(applied) = applied.and_then(Value::as_object) {
            let mut fields = Map::new();
            fields.insert("outcome".into(), json!(outcome));
            for (k, v) in applied {
                if [
                    "mode",
                    "presentation_name",
                    "target_name",
                    "target_uuid",
                    "slide_count",
                    "cue_count_before",
                    "cue_count_after",
                    "removed_slides",
                    "verified",
                    "verification",
                    "steps",
                ]
                .contains(&k.as_str())
                {
                    fields.insert(k.clone(), v.clone());
                }
            }
            versevision_core::notes::trace::emit(self, "output", fields);
        }
        let mut end = Map::new();
        end.insert("outcome".into(), json!(outcome));
        end.insert(
            "error".into(),
            error.map(|e| json!(e)).unwrap_or(Value::Null),
        );
        let level = if bad {
            "error"
        } else if outcome.starts_with("stopped") {
            "warn"
        } else {
            "info"
        };
        self.event("run_end", level, end);
        let text = render_summary(self, outcome, report, applied, error);
        if !self.write("summary.txt", &text) {
            let mut data = Map::new();
            data.insert("error".into(), json!("could not write summary.txt"));
            self.event("summary_failed", "warn", data);
        }
    }

    /// Keep the newest [`KEEP_RUNS`] folders (names sort by time).
    fn prune(&self) {
        let Ok(entries) = fs::read_dir(&self.runs_dir) else {
            return;
        };
        let mut folders: Vec<_> = entries
            .filter_map(Result::ok)
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        folders.sort();
        let excess = folders.len().saturating_sub(KEEP_RUNS);
        for old in &folders[..excess] {
            let _ = fs::remove_dir_all(self.runs_dir.join(old));
        }
    }
}

impl TraceSink for RunLog {
    /// One decision record in `trace.jsonl` (the caller has cleaned the fields: `notes::trace::emit`).
    fn trace(&mut self, stage: &str, fields: Map<String, Value>) {
        self.seq += 1;
        let mut line = Map::new();
        line.insert("seq".into(), json!(self.seq));
        line.insert("stage".into(), json!(stage));
        line.extend(fields);
        self.append("trace.jsonl", &dumps(&Value::Object(line), None, false));
    }
}

// ---------------------------------------------------------------------- reading

fn read_lines(path: &Path) -> Vec<Value> {
    let Ok(bytes) = fs::read(path) else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    splitlines(&text)
        .into_iter()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect()
}

/// `read_events(run_id)`.
pub fn read_events(runs_dir: &Path, run_id: &str) -> Vec<Value> {
    read_lines(&runs_dir.join(run_id).join("events.jsonl"))
}

/// `read_trace(run_id)`.
pub fn read_trace(runs_dir: &Path, run_id: &str) -> Vec<Value> {
    read_lines(&runs_dir.join(run_id).join("trace.jsonl"))
}

fn run_folders(runs_dir: &Path) -> Option<Vec<String>> {
    let entries = fs::read_dir(runs_dir).ok()?;
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    Some(names)
}

/// `list_runs(limit)`: newest first, `{"id", "path", "outcome", "source", "mode"[, "started"]}`.
pub fn list_runs(runs_dir: &Path, limit: usize) -> Vec<Value> {
    let Some(mut names) = run_folders(runs_dir) else {
        return Vec::new();
    };
    names.reverse();
    names.truncate(limit);
    names
        .into_iter()
        .map(|name| {
            let mut info = Map::new();
            info.insert("id".into(), json!(name));
            info.insert("path".into(), json!(runs_dir.join(&name).to_string_lossy()));
            info.insert("outcome".into(), json!("unfinished"));
            info.insert("source".into(), json!(""));
            info.insert("mode".into(), json!(""));
            for e in read_events(runs_dir, &name) {
                match e.get("step").and_then(Value::as_str) {
                    Some("run_start") => {
                        info.insert(
                            "source".into(),
                            e.get("source").cloned().unwrap_or(json!("")),
                        );
                        let mode = e
                            .get("mode")
                            .or_else(|| e.get("kind"))
                            .cloned()
                            .unwrap_or(json!(""));
                        info.insert("mode".into(), mode);
                        info.insert(
                            "started".into(),
                            e.get("started").cloned().unwrap_or(json!("")),
                        );
                    }
                    Some("run_end") => {
                        info.insert(
                            "outcome".into(),
                            e.get("outcome").cloned().unwrap_or(json!("?")),
                        );
                    }
                    _ => {}
                }
            }
            Value::Object(info)
        })
        .collect()
}

/// `resolve(run_id)`: `latest`, an exact id, or the end of an id that fits exactly one run.
pub fn resolve(runs_dir: &Path, run_id: &str) -> Option<String> {
    if run_id.is_empty() || run_id == "latest" {
        let runs = list_runs(runs_dir, 1);
        return runs
            .first()
            .and_then(|r| r["id"].as_str().map(str::to_string));
    }
    if runs_dir.join(run_id).is_dir() {
        return Some(run_id.to_string());
    }
    let matches: Vec<String> = run_folders(runs_dir)
        .unwrap_or_default()
        .into_iter()
        .filter(|n| n.ends_with(run_id))
        .collect();
    (matches.len() == 1).then(|| matches[0].clone())
}

// ---------------------------------------------------------------------- the readable summary

fn get_str(value: &Value, key: &str) -> String {
    value.get(key).map(py_str).unwrap_or_default()
}

fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

fn elapsed_of(event: &Value) -> String {
    match event.get("elapsed") {
        Some(Value::Number(n)) if n.is_f64() => float_repr(n.as_f64().unwrap_or(0.0)),
        Some(other) => py_str(other),
        None => String::new(),
    }
}

/// `render_summary`.
pub fn render_summary(
    run: &RunLog,
    outcome: &str,
    report: Option<&Value>,
    applied: Option<&Value>,
    error: Option<&str>,
) -> String {
    let events = read_events(&run.runs_dir, &run.id);
    let start = events
        .iter()
        .find(|e| e["step"] == "run_start")
        .cloned()
        .unwrap_or(json!({}));
    let mut lines: Vec<String> = Vec::new();
    let mut add = |s: String| lines.push(s);
    add(format!("VerseVision run {}", run.id));
    add("=".repeat(60));
    let result = match outcome {
        "ok" => "SUCCESS".to_string(),
        "failed" => "FAILED".to_string(),
        other => other.to_uppercase(),
    };
    add(format!("Result:        {result}"));
    if let Some(error) = error.filter(|e| !e.is_empty()) {
        add(format!("Error:         {error}"));
    }
    add(format!("Started:       {}", get_str(&start, "started")));
    add(format!(
        "Version:       VerseVision {}",
        get_str(&start, "application_version")
    ));
    add(format!("Document:      {}", get_str(&start, "source")));
    let action = match start.get("mode") {
        Some(v) if !v.is_null() => py_str(v),
        _ => run.kind.clone(),
    };
    add(format!("Action:        {action}"));
    for (key, label) in [
        ("title", "Name used"),
        ("library", "Library"),
        ("target", "Chosen presentation"),
        ("node", "ProPresenter on"),
        ("default_version", "Default version"),
        ("flags", "Options"),
    ] {
        if truthy(start.get(key)) {
            add(format!(
                "{} {}",
                pad_right(&format!("{label}:"), 14),
                get_str(&start, key)
            ));
        }
    }
    if let Some(src) = events.iter().find(|e| e["step"] == "source_saved") {
        let sha = get_str(src, "sha256");
        add(format!(
            "Source text:   {} characters, {} lines (sha256 {}), kept in source.txt",
            get_str(src, "characters"),
            get_str(src, "lines"),
            slice_chars(&sha, 0, 12)
        ));
    }
    let applied_obj = applied.filter(|a| truthy(Some(a)));
    if let (Some(applied), true) = (
        applied_obj,
        start.get("mode").and_then(Value::as_str) == Some("undo"),
    ) {
        add(String::new());
        add("What was undone".to_string());
        add("-".repeat(60));
        let name = if truthy(applied.get("presentation_name")) {
            get_str(applied, "presentation_name")
        } else {
            get_str(applied, "target_name")
        };
        add(format!(
            "Removed:       {name} ({} slides)",
            get_str(applied, "removed_slides")
        ));
        add(format!(
            "Result:        {}",
            get_str(applied, "verification")
        ));
    } else if let Some(applied) = applied_obj {
        add(String::new());
        add("What was written".to_string());
        add("-".repeat(60));
        let name = if truthy(applied.get("presentation_name")) {
            get_str(applied, "presentation_name")
        } else {
            get_str(applied, "target_name")
        };
        add(format!("New file:      {name}"));
        let mode = applied.get("mode").and_then(Value::as_str).unwrap_or("");
        let extra = if mode == "replace" || mode == "append" {
            format!(
                " (copied from one with {}, {} removed)",
                get_str(applied, "cue_count_before"),
                get_str(applied, "removed_slides")
            )
        } else {
            String::new()
        };
        add(format!(
            "Slides:        {} added; the file has {} slides{extra}",
            get_str(applied, "slide_count"),
            get_str(applied, "cue_count_after")
        ));
        add(format!(
            "Verified:      {} - {}",
            if truthy(applied.get("verified")) {
                "yes"
            } else {
                "NO"
            },
            get_str(applied, "verification")
        ));
        if let Some(steps) = applied.get("steps").and_then(Value::as_array) {
            for step in steps {
                add(format!("  - {}", py_str(step)));
            }
        }
    }
    if let Some(report) = report {
        let s = &report["summary"];
        let n = |k: &str| py_str(&s[k]);
        add(String::new());
        add("What was understood".to_string());
        add("-".repeat(60));
        add(format!(
            "{} slides: {} verse, {} sermon point. {} flagged ({} not added, {} need review). {} adjustments. {} lines left out on purpose.",
            n("slides"), n("verse_slides"), n("point_slides"), n("flagged"), n("not_added"), n("needs_review"), n("adjustments"), n("left_out")
        ));
        add(String::new());
        for slide in report["slides"].as_array().cloned().unwrap_or_default() {
            let title = get_str(&slide, "title");
            let text = get_str(&slide, "text").replace('\n', " ");
            add(format!(
                "  {}. {} {} {}",
                pad_left(&get_str(&slide, "n"), 3),
                pad_right(&get_str(&slide, "kind"), 5),
                pad_right(&slice_chars(&title, 0, 46), 46),
                slice_chars(&text, 0, 60)
            ));
        }
        for (title, key) in [
            ("FLAGGED (not imported or needs a look)", "flagged"),
            ("ADJUSTMENTS made automatically", "adjustments"),
            ("LEFT OUT on purpose", "left_out"),
        ] {
            let rows = report
                .get(key)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if !rows.is_empty() {
                add(String::new());
                add(format!("{title}: {}", rows.len()));
                for r in rows {
                    add(format!(
                        "  line {}  {}  -> {}",
                        pad_left(&get_str(&r, "line"), 4),
                        py_repr(&slice_chars(&get_str(&r, "text"), 0, 70)),
                        get_str(&r, "detail")
                    ));
                }
            }
        }
    }
    let fetches: Vec<&Value> = events
        .iter()
        .filter(|e| e["step"] == "bible_fetch")
        .collect();
    if !fetches.is_empty() {
        add(String::new());
        add("Bible lookups".to_string());
        add("-".repeat(60));
        for e in fetches {
            let tail = if truthy(e.get("error")) {
                format!("ERROR {}", get_str(e, "error"))
            } else {
                format!("{} verses", get_str(e, "verses"))
            };
            let provider = e.get("provider").map(py_str).unwrap_or_else(|| "-".into());
            let ms = e.get("ms").map(py_str).unwrap_or_else(|| "0".into());
            add(format!(
                "  {} {} via {} {} {} ms  {tail}",
                pad_right(&get_str(e, "reference"), 28),
                pad_right(&get_str(e, "version"), 8),
                pad_right(&provider, 10),
                pad_right(&get_str(e, "source"), 8),
                pad_left(&ms, 5)
            ));
        }
    }
    let problems: Vec<&Value> = events
        .iter()
        .filter(|e| matches!(e["level"].as_str(), Some("warn" | "error")) && e["step"] != "run_end")
        .collect();
    if !problems.is_empty() {
        add(String::new());
        add("Warnings and errors".to_string());
        add("-".repeat(60));
        for e in problems {
            let detail = ["message", "error"]
                .iter()
                .find(|k| truthy(e.get(**k)))
                .map(|k| get_str(e, k))
                .unwrap_or_default();
            add(format!(
                "  [{}s] {}: {detail}",
                pad_left(&elapsed_of(e), 6),
                get_str(e, "step")
            ));
        }
    }
    add(String::new());
    add("Timeline".to_string());
    add("-".repeat(60));
    for e in &events {
        let step = e["step"].as_str().unwrap_or("");
        if [
            "progress",
            "node_action",
            "playlist",
            "verify",
            "file_placed",
            "run_failed",
            "plan_ready",
        ]
        .contains(&step)
        {
            let detail = ["message", "action", "error"]
                .iter()
                .find(|k| truthy(e.get(**k)))
                .map(|k| get_str(e, k))
                .unwrap_or_default();
            add(format!(
                "  [{}s] {} {detail}",
                pad_left(&elapsed_of(e), 6),
                pad_right(step, 12)
            ));
        }
    }
    add(String::new());
    add("Files in this folder: summary.txt, events.jsonl (every step), trace.jsonl (every decision), options.json, source.txt, report.json".to_string());
    add(format!(
        "To follow any line of the document to its slide:  versevision explain {}",
        run.id
    ));
    lines.join("\n") + "\n"
}
