//! The oracle runner: what the Python app produced (`oracle/`) against what this crate produces.
//!
//! * `oracle/cases/*`: each case is read (input, options, bible.json), planned and compared field by field with
//!   `expected/plan.json`, `review.json` and `trace.jsonl`. The planner is not written yet (S3), so every case fails
//!   with `NotImplemented`; the comparison itself is tested on its own below.
//! * `oracle/fitting.jsonl`, `references.jsonl`, `versions.jsonl`: one function call per line.
//! * `oracle/config_defaults.json` / `.toml` and `config_options.jsonl`: the config model.
//!
//! `oracle/expected_failures.txt` is a ratchet: a listed unit that passes, or an unlisted one that fails, fails the test.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};
use versevision_core::config::{changed_fields, AppConfig};
use versevision_core::fitting;
use versevision_core::plan::{
    plan_notes, NotesImportResult, PlanOptions, PlanRun, ReviewReport, TraceRecord, VerseSource,
};
use versevision_core::references::parse_references;
use versevision_core::versions::version_line;

fn oracle_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../oracle")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

fn json_file(path: &Path) -> Value {
    serde_json::from_str(&read(path))
        .unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()))
}

fn jsonl_file(path: &Path) -> Vec<Value> {
    read(path)
        .lines()
        .enumerate()
        .map(|(i, line)| {
            serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("{} line {}: {e}", path.display(), i + 1))
        })
        .collect()
}

// ------------------------------------------------------------------------------------------------ the comparison

fn show(value: &Value) -> String {
    let text = value.to_string();
    if text.chars().count() > 300 {
        format!(
            "{}... ({} characters)",
            text.chars().take(300).collect::<String>(),
            text.chars().count()
        )
    } else {
        text
    }
}

/// Where `actual` first differs from `expected`, as a JSON path with both values; `None` when they are the same.
/// Key order does not matter, numbers compare as numbers (`48` equals `48.0`), everything else exactly.
fn first_difference(path: &str, expected: &Value, actual: &Value) -> Option<String> {
    let here = if path.is_empty() {
        "(whole file)"
    } else {
        path
    };
    match (expected, actual) {
        (Value::Object(want), Value::Object(got)) => {
            for (key, value) in want {
                let child = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                match got.get(key) {
                    None => {
                        return Some(format!(
                            "{child}: missing in the Rust output (expected {})",
                            show(value)
                        ))
                    }
                    Some(other) => {
                        if let Some(difference) = first_difference(&child, value, other) {
                            return Some(difference);
                        }
                    }
                }
            }
            got.iter()
                .find(|(key, _)| !want.contains_key(*key))
                .map(|(key, value)| {
                    format!(
                        "{here}.{key}: extra in the Rust output ({}); the contract is closed",
                        show(value)
                    )
                })
        }
        (Value::Array(want), Value::Array(got)) => {
            for (i, (a, b)) in want.iter().zip(got).enumerate() {
                if let Some(difference) = first_difference(&format!("{path}[{i}]"), a, b) {
                    return Some(difference);
                }
            }
            (want.len() != got.len()).then(|| {
                format!(
                    "{here}: {} items expected, {} in the Rust output",
                    want.len(),
                    got.len()
                )
            })
        }
        (Value::Number(a), Value::Number(b)) => {
            (a.as_f64() != b.as_f64()).then(|| format!("{here}: expected {a}, got {b}"))
        }
        (a, b) => (a != b).then(|| format!("{here}: expected {}, got {}", show(a), show(b))),
    }
}

/// The first difference of two JSON-lines files, by record: `trace[41].settings.max_body_lines`.
fn first_trace_difference(expected: &[Value], actual: &[Value]) -> Option<String> {
    first_difference(
        "trace",
        &Value::Array(expected.to_vec()),
        &Value::Array(actual.to_vec()),
    )
}

// ------------------------------------------------------------------------------------------------ the ratchet

#[derive(Debug, Default)]
struct Listed {
    groups: BTreeSet<String>,
    cases: BTreeSet<String>,
    files: BTreeSet<String>,
}

fn parse_listed(text: &str) -> Result<Listed, String> {
    let mut listed = Listed::default();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        match line.split_once(':') {
            Some(("group", name)) => listed.groups.insert(name.trim().to_string()),
            Some(("case", name)) => listed.cases.insert(name.trim().to_string()),
            Some(("file", name)) => listed.files.insert(name.trim().to_string()),
            _ => {
                return Err(format!(
                    "expected_failures.txt line {}: `{raw}` is not group:/case:/file:",
                    i + 1
                ))
            }
        };
    }
    Ok(listed)
}

/// One thing the oracle checks, and whether the Rust side passed it.
#[derive(Debug, Clone)]
struct Unit {
    id: String,
    group: Option<String>,
    passed: bool,
    detail: String,
}

fn case_group(name: &str) -> String {
    name.split('-').next().unwrap_or(name).to_string()
}

/// The ratchet: every complaint (empty = all in order).
fn ratchet(units: &[Unit], listed: &Listed) -> Vec<String> {
    let mut complaints = Vec::new();
    let mut known: BTreeSet<&str> = BTreeSet::new();
    let mut group_results: BTreeMap<&str, Vec<&Unit>> = BTreeMap::new();
    for unit in units {
        known.insert(&unit.id);
        if let Some(group) = &unit.group {
            group_results.entry(group).or_default().push(unit);
        }
    }
    for unit in units {
        let by_group = unit
            .group
            .as_ref()
            .is_some_and(|g| listed.groups.contains(g));
        let by_name = listed.cases.contains(unit.id.trim_start_matches("case:"))
            && unit.id.starts_with("case:")
            || listed.files.contains(unit.id.trim_start_matches("file:"))
                && unit.id.starts_with("file:");
        let expected_to_fail = by_group || by_name;
        match (expected_to_fail, unit.passed) {
            (true, true) => complaints.push(format!(
                "{} now passes: remove it from expected_failures.txt (the list only shrinks)",
                unit.id
            )),
            (false, false) => complaints.push(format!(
                "{} fails and is not in expected_failures.txt: {}",
                unit.id, unit.detail
            )),
            _ => {}
        }
    }
    for group in &listed.groups {
        if !group_results.contains_key(group.as_str()) {
            complaints.push(format!(
                "expected_failures.txt lists group:{group}, but no case starts with `{group}-`"
            ));
        }
    }
    for name in &listed.cases {
        if !known.contains(format!("case:{name}").as_str()) {
            complaints.push(format!(
                "expected_failures.txt lists case:{name}, which does not exist"
            ));
        }
    }
    for name in &listed.files {
        if !known.contains(format!("file:{name}").as_str()) {
            complaints.push(format!(
                "expected_failures.txt lists file:{name}, which the runner does not check"
            ));
        }
    }
    complaints
}

// ------------------------------------------------------------------------------------------------ cases

/// The offline provider: verses come from the case's `bible.json` (`"NKJV Psalms 23:1"` -> text).
struct OfflineBible(Map<String, Value>);

impl VerseSource for OfflineBible {
    fn verse_text(&self, version: &str, book: &str, chapter: u32, verse: u32) -> Option<String> {
        self.0
            .get(&format!("{version} {book} {chapter}:{verse}"))
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

struct LoadedCase {
    text: Option<String>,
    config: AppConfig,
    options: PlanOptions,
    bible: OfflineBible,
}

fn input_file(dir: &Path) -> PathBuf {
    fs::read_dir(dir)
        .expect("case folder")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.file_stem().is_some_and(|s| s == "input"))
        .unwrap_or_else(|| panic!("{} has no input file", dir.display()))
}

/// Read a case: the document text (txt and md only: the other readers are S4), the options, the offline Bible.
fn load_case(dir: &Path) -> Result<LoadedCase, String> {
    let input = input_file(dir);
    let extension = input.extension().and_then(|e| e.to_str()).unwrap_or("");
    let text = match extension {
        // read in text mode: `\r\n` and `\r` become `\n`
        "txt" | "md" => Some(read(&input).replace("\r\n", "\n").replace('\r', "\n")),
        _ => None,
    };
    let options_text = read(&dir.join("options.toml"));
    let config =
        AppConfig::from_toml_str(&options_text).map_err(|e| format!("options.toml: {e}"))?;
    let raw: toml::Table = toml::from_str(&options_text).map_err(|e| e.to_string())?;
    let run = raw.get("run").and_then(toml::Value::as_table);
    let default_version = match run.and_then(|r| r.get("default_version")) {
        None => Some("NKJV".to_string()),
        Some(toml::Value::String(s)) if s.is_empty() => None,
        Some(toml::Value::String(s)) => Some(s.clone()),
        Some(other) => return Err(format!("[run] default_version: {other}")),
    };
    let no_fetch = run
        .and_then(|r| r.get("no_fetch"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(false);
    let bible = match json_file(&dir.join("bible.json")) {
        Value::Object(map) => OfflineBible(map),
        other => return Err(format!("bible.json is not an object: {other}")),
    };
    Ok(LoadedCase {
        text,
        config,
        options: PlanOptions {
            default_version,
            no_fetch,
        },
        bible,
    })
}

/// Compare a run with the three expected files; `None` when they match.
fn compare_run(dir: &Path, run: &PlanRun) -> Option<String> {
    let expected = dir.join("expected");
    let plan = serde_json::to_value(&run.result).expect("plan serialises");
    if let Some(d) = first_difference("plan", &json_file(&expected.join("plan.json")), &plan) {
        return Some(d);
    }
    let review = serde_json::to_value(&run.report).expect("report serialises");
    if let Some(d) = first_difference("review", &json_file(&expected.join("review.json")), &review)
    {
        return Some(d);
    }
    let trace: Vec<Value> = run
        .trace
        .iter()
        .map(|r| serde_json::to_value(r).expect("record serialises"))
        .collect();
    first_trace_difference(&jsonl_file(&expected.join("trace.jsonl")), &trace)
}

fn run_case(dir: &Path) -> Result<(), String> {
    let loaded = load_case(dir)?;
    let Some(text) = loaded.text.as_deref() else {
        return Err("not implemented yet: reading this kind of document arrives in S4".to_string());
    };
    let run = plan_notes(text, &loaded.config, &loaded.options, &loaded.bible)
        .map_err(|e| e.to_string())?;
    match compare_run(dir, &run) {
        None => Ok(()),
        Some(difference) => Err(difference),
    }
}

fn case_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(oracle_dir().join("cases"))
        .expect("oracle/cases")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs
}

// ------------------------------------------------------------------------------------------------ line-level files

fn run_fitting_line(row: &Value) -> Result<(), String> {
    let args = &row["args"];
    let float = |key: &str| {
        args[key]
            .as_f64()
            .ok_or_else(|| format!("args.{key} is not a number"))
    };
    let count = |key: &str| {
        args[key]
            .as_u64()
            .map(|n| n as usize)
            .ok_or_else(|| format!("args.{key} is not a count"))
    };
    let flag = |key: &str| {
        args[key]
            .as_bool()
            .ok_or_else(|| format!("args.{key} is not a bool"))
    };
    let text = |key: &str| {
        args[key]
            .as_str()
            .ok_or_else(|| format!("args.{key} is not text"))
    };
    let name = row["fn"].as_str().unwrap_or("?");
    let actual = match name {
        "balance_lines" => Value::String(fitting::balance_lines(
            text("text")?,
            float("line_width_units")?,
            flag("upper")?,
            count("glue_first")?,
            count("max_lines")?,
            float("tolerance")?,
            float("punctuation_tolerance")?,
        )),
        "split_verse_text" => json!(fitting::split_verse_text(
            text("text")?,
            count("max_lines")?,
            float("line_width_units")?,
            flag("prefer_punctuation")?,
            float("tolerance")?,
            float("punctuation_tolerance")?,
        )),
        "split_point_text" => json!(fitting::split_point_text(
            text("text")?,
            float("line_width_units")?,
            count("max_lines")?,
            flag("upper")?,
            count("glue_first")?,
            float("tolerance")?,
            float("punctuation_tolerance")?,
        )),
        "text_width_units" => json!(fitting::text_width_units(text("value")?)),
        "wrap_body_lines" => json!(fitting::wrap_body_lines(
            text("text")?,
            float("line_width_units")?
        )),
        other => return Err(format!("unknown function {other}")),
    };
    match first_difference("out", &row["out"], &actual) {
        None => Ok(()),
        Some(difference) => Err(format!("{name}({}): {difference}", show(args))),
    }
}

fn run_reference_line(row: &Value) -> Result<(), String> {
    let aliases = row["aliases"].as_object();
    let actual = parse_references(row["input"].as_str().unwrap_or(""), aliases);
    if let Some(exception) = row["raises"].as_str() {
        // Python raised (the oracle records the exception, not a result): Rust has to fail too, the same way.
        return match actual {
            Err(e) if e.python_exception() == exception => Ok(()),
            Err(e) => Err(format!("Python raises {exception}; Rust gave {e}")),
            Ok(value) => Err(format!(
                "Python raises {exception}; Rust returned {}",
                show(&value)
            )),
        };
    }
    let actual = actual.map_err(|e| e.to_string())?;
    let expected = json!({"references": row["references"], "issues": row["issues"]});
    first_difference("out", &expected, &actual).map_or(Ok(()), Err)
}

fn run_version_line(row: &Value) -> Result<(), String> {
    let actual = version_line(row).map_err(|e| e.to_string())?;
    first_difference("out", &row["out"], &actual).map_or(Ok(()), Err)
}

/// Run every line of a jsonl oracle: the unit passes when all lines do. Returns the unit and `(passed, total)`.
fn jsonl_unit(file: &str, run_line: fn(&Value) -> Result<(), String>) -> (Unit, usize, usize) {
    let rows = jsonl_file(&oracle_dir().join(file));
    let mut passed = 0;
    let mut first_failure = String::new();
    for (i, row) in rows.iter().enumerate() {
        match run_line(row) {
            Ok(()) => passed += 1,
            Err(why) if first_failure.is_empty() => {
                first_failure = format!("line {}: {why}", i + 1)
            }
            Err(_) => {}
        }
    }
    let unit = Unit {
        id: format!("file:{file}"),
        group: None,
        passed: passed == rows.len(),
        detail: first_failure,
    };
    (unit, passed, rows.len())
}

// ------------------------------------------------------------------------------------------------ the config files

fn run_config_defaults_json() -> Result<(), String> {
    let expected = json_file(&oracle_dir().join("config_defaults.json"));
    first_difference("config", &expected, &AppConfig::default().to_value()).map_or(Ok(()), Err)
}

fn run_config_defaults_toml() -> Result<(), String> {
    let expected = read(&oracle_dir().join("config_defaults.toml"));
    let written = AppConfig::default().to_toml_string();
    if written != expected {
        let at = written
            .lines()
            .zip(expected.lines())
            .position(|(a, b)| a != b)
            .map_or(0, |i| i + 1);
        return Err(format!(
            "the saved default config differs from Python's from line {at}"
        ));
    }
    match AppConfig::from_toml_str(&expected) {
        Ok(read_back) if read_back == AppConfig::default() => Ok(()),
        Ok(_) => Err(
            "the Python default config.toml reads back as something other than the defaults"
                .to_string(),
        ),
        Err(e) => Err(e.to_string()),
    }
}

/// A config that uses every kind of value, as Python saved it: reading it and saving it again gives the same bytes.
fn run_config_rich_toml() -> Result<(), String> {
    let expected = read(&oracle_dir().join("config_rich.toml"));
    let config = AppConfig::from_toml_str(&expected).map_err(|e| e.to_string())?;
    let written = config.to_toml_string();
    if written != expected {
        let at = written
            .lines()
            .zip(expected.lines())
            .position(|(a, b)| a != b)
            .map_or(0, |i| i + 1);
        return Err(format!(
            "saving what Python saved changes it, from line {at}"
        ));
    }
    // the values themselves: spot checks of what is easy to get wrong
    if config.bible.providers.len() != 3
        || config.profiles.len() != 2
        || config.presentation_template.is_none()
    {
        return Err("the providers, profiles or template were not read".to_string());
    }
    Ok(())
}

fn run_config_option_line(row: &Value) -> Result<(), String> {
    let base = AppConfig::from_value(&row["base"])
        .map_err(|e| format!("the base config is invalid: {} at {}", e.msg, e.location()))?;
    let profile = row["profile"].as_str();
    let sets: Vec<&str> = row["sets"]
        .as_array()
        .map_or_else(Vec::new, |a| a.iter().filter_map(Value::as_str).collect());
    let outcome = base.with_options(profile, &sets);
    let want = &row["out"];
    match (outcome, want.get("error"), want.get("changed")) {
        (Err(got), Some(Value::String(text)), _) => {
            if &got.to_string() == text {
                Ok(())
            } else {
                Err(format!(
                    "error text: expected {text:?}, got {:?}",
                    got.to_string()
                ))
            }
        }
        (Err(got), _, _) => Err(format!(
            "expected a valid config, got the error {:?}",
            got.to_string()
        )),
        (Ok(_), Some(Value::String(text)), _) => {
            Err(format!("expected the error {text:?}, got a valid config"))
        }
        (Ok(result), _, Some(changed)) => {
            let (got_changed, got_removed) = changed_fields(&base.to_value(), &result.to_value());
            if let Some(d) = first_difference("changed", changed, &Value::Object(got_changed)) {
                return Err(d);
            }
            let removed = want.get("removed").cloned().unwrap_or_else(|| json!([]));
            if let Some(d) = first_difference("removed", &removed, &json!(got_removed)) {
                return Err(d);
            }
            // what the Config page saves must read back as the same config
            match AppConfig::from_toml_str(&result.to_toml_string()) {
                Ok(read_back) if read_back == result => Ok(()),
                Ok(_) => Err("saving and reading the result back changes it".to_string()),
                Err(e) => Err(format!("the saved result does not read back: {e}")),
            }
        }
        (Ok(_), _, None) => Err("the oracle row has neither error nor changed".to_string()),
    }
}

// ------------------------------------------------------------------------------------------------ the tests

fn unit_from(id: String, group: Option<String>, result: Result<(), String>) -> Unit {
    match result {
        Ok(()) => Unit {
            id,
            group,
            passed: true,
            detail: String::new(),
        },
        Err(detail) => Unit {
            id,
            group,
            passed: false,
            detail,
        },
    }
}

#[test]
fn oracle_against_expected_failures() {
    let listed = parse_listed(&read(&oracle_dir().join("expected_failures.txt")))
        .expect("expected_failures.txt");
    let mut units: Vec<Unit> = Vec::new();
    let mut report = String::new();

    let dirs = case_dirs();
    let mut by_group: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for dir in &dirs {
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();
        let group = case_group(&name);
        let unit = unit_from(format!("case:{name}"), Some(group.clone()), run_case(dir));
        let entry = by_group.entry(group).or_default();
        entry.1 += 1;
        entry.0 += usize::from(unit.passed);
        units.push(unit);
    }
    let passing: usize = by_group.values().map(|g| g.0).sum();
    let _ = writeln!(report, "oracle cases: {passing}/{} pass", dirs.len());
    for (group, (ok, total)) in &by_group {
        let _ = writeln!(
            report,
            "    {group:<10} {ok}/{total}{}",
            if listed.groups.contains(group) {
                "   (in expected_failures)"
            } else {
                ""
            }
        );
    }

    for (file, runner) in [
        (
            "fitting.jsonl",
            run_fitting_line as fn(&Value) -> Result<(), String>,
        ),
        ("references.jsonl", run_reference_line),
        ("versions.jsonl", run_version_line),
        ("config_options.jsonl", run_config_option_line),
    ] {
        let (unit, passed, total) = jsonl_unit(file, runner);
        let _ = writeln!(
            report,
            "oracle {file}: {passed}/{total} lines pass{}",
            if listed.files.contains(file) {
                "   (in expected_failures)"
            } else {
                ""
            }
        );
        units.push(unit);
    }
    for (file, result) in [
        ("config_defaults.json", run_config_defaults_json()),
        ("config_defaults.toml", run_config_defaults_toml()),
        ("config_rich.toml", run_config_rich_toml()),
    ] {
        let _ = writeln!(
            report,
            "oracle {file}: {}",
            if result.is_ok() { "pass" } else { "FAIL" }
        );
        units.push(unit_from(format!("file:{file}"), None, result));
    }

    println!("{report}");
    let complaints = ratchet(&units, &listed);
    assert!(
        complaints.is_empty(),
        "oracle ratchet:\n  {}\n\n{report}",
        complaints.join("\n  ")
    );
}

/// Every case's options load as a config, with the run parameters in place (they are the input of S3, so a case
/// that cannot even be read must never hide behind "not implemented").
#[test]
fn every_case_loads() {
    for dir in case_dirs() {
        let loaded = load_case(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
        let _ = loaded;
    }
}

/// The plan types are the shape of the expected files: every `plan.json`, `review.json` and `trace.jsonl` of the corpus
/// reads into them and writes back unchanged (so the planner has only values left to get right).
#[test]
fn expected_files_round_trip_through_the_plan_types() {
    let mut files = 0;
    for dir in case_dirs() {
        let expected = dir.join("expected");
        let plan: NotesImportResult =
            serde_json::from_value(json_file(&expected.join("plan.json")))
                .unwrap_or_else(|e| panic!("{}: plan.json: {e}", dir.display()));
        if let Some(d) = first_difference(
            "plan",
            &json_file(&expected.join("plan.json")),
            &serde_json::to_value(&plan).unwrap(),
        ) {
            panic!("{}: {d}", dir.display());
        }
        let report: ReviewReport = serde_json::from_value(json_file(&expected.join("review.json")))
            .unwrap_or_else(|e| panic!("{}: review.json: {e}", dir.display()));
        if let Some(d) = first_difference(
            "review",
            &json_file(&expected.join("review.json")),
            &serde_json::to_value(&report).unwrap(),
        ) {
            panic!("{}: {d}", dir.display());
        }
        let trace = jsonl_file(&expected.join("trace.jsonl"));
        let records: Vec<TraceRecord> = trace
            .iter()
            .map(|r| {
                serde_json::from_value(r.clone())
                    .unwrap_or_else(|e| panic!("{}: trace: {e}", dir.display()))
            })
            .collect();
        for (i, record) in records.iter().enumerate() {
            assert_eq!(
                record.seq as usize,
                i + 1,
                "{}: trace seq counts from 1 with no gaps",
                dir.display()
            );
        }
        let back: Vec<Value> = records
            .iter()
            .map(|r| serde_json::to_value(r).unwrap())
            .collect();
        if let Some(d) = first_trace_difference(&trace, &back) {
            panic!("{}: {d}", dir.display());
        }
        files += 3;
    }
    println!("{files} expected files read into the plan types and written back unchanged");
}

#[test]
fn plan_notes_is_not_written_yet() {
    let error = plan_notes(
        "x",
        &AppConfig::default(),
        &PlanOptions::default(),
        &OfflineBible(Map::new()),
    )
    .unwrap_err();
    assert!(
        error.to_string().starts_with("not implemented yet"),
        "{error}"
    );
}

// -- the comparison and the ratchet are code too: test them

#[test]
fn difference_names_the_path_and_both_values() {
    let want = json!({"decisions": [{"reasons": ["a", "b"]}, {"reasons": ["c"]}], "n": 48});
    assert_eq!(
        first_difference(
            "plan",
            &want,
            &json!({"n": 48.0, "decisions": [{"reasons": ["a", "b"]}, {"reasons": ["c"]}]})
        ),
        None,
        "key order and 48 vs 48.0 do not matter"
    );
    let got = json!({"decisions": [{"reasons": ["a", "b"]}, {"reasons": ["c "]}], "n": 48});
    assert_eq!(
        first_difference("plan", &want, &got).as_deref(),
        Some("plan.decisions[1].reasons[0]: expected \"c\", got \"c \"")
    );
    let got = json!({"decisions": [{"reasons": ["a"]}, {"reasons": ["c"]}], "n": 48});
    assert_eq!(
        first_difference("plan", &want, &got).as_deref(),
        Some("plan.decisions[0].reasons: 2 items expected, 1 in the Rust output")
    );
    let got = json!({"decisions": [], "n": 48});
    assert_eq!(
        first_difference("plan", &want, &got).as_deref(),
        Some("plan.decisions: 2 items expected, 0 in the Rust output")
    );
    let got = json!({"decisions": [{"reasons": ["a", "b"]}, {"reasons": ["c"]}]});
    assert!(first_difference("plan", &want, &got)
        .unwrap()
        .starts_with("plan.n: missing in the Rust output"));
    let got =
        json!({"decisions": [{"reasons": ["a", "b"]}, {"reasons": ["c"]}], "n": 48, "extra": 1});
    assert!(first_difference("plan", &want, &got)
        .unwrap()
        .contains("extra in the Rust output"));
    let trace_want = vec![json!({"seq": 1, "settings": {"max_body_lines": 5}})];
    let trace_got = vec![json!({"seq": 1, "settings": {"max_body_lines": 4}})];
    assert_eq!(
        first_trace_difference(&trace_want, &trace_got).as_deref(),
        Some("trace[0].settings.max_body_lines: expected 5, got 4")
    );
    assert_eq!(
        first_difference("x", &json!("a\nb"), &json!("a b")).as_deref(),
        Some("x: expected \"a\\nb\", got \"a b\"")
    );
}

#[test]
fn ratchet_fails_both_ways() {
    let unit = |id: &str, group: Option<&str>, passed: bool| Unit {
        id: id.to_string(),
        group: group.map(str::to_string),
        passed,
        detail: "why".to_string(),
    };
    let listed =
        parse_listed("# c\ngroup:tests\ncase:whitefield-a  # one\nfile:references.jsonl\n")
            .unwrap();
    let units = vec![
        unit("case:tests-1", Some("tests"), false),
        unit("case:whitefield-a", Some("whitefield"), false),
        unit("case:whitefield-b", Some("whitefield"), true),
        unit("file:references.jsonl", None, false),
        unit("file:fitting.jsonl", None, true),
    ];
    assert!(ratchet(&units, &listed).is_empty());
    let mut now_passing = units.clone();
    now_passing[0].passed = true;
    assert_eq!(
        ratchet(&now_passing, &listed),
        vec![
            "case:tests-1 now passes: remove it from expected_failures.txt (the list only shrinks)"
        ]
    );
    let mut regressed = units.clone();
    regressed[2].passed = false;
    assert_eq!(
        ratchet(&regressed, &listed),
        vec!["case:whitefield-b fails and is not in expected_failures.txt: why"]
    );
    let mut file_now_passing = units.clone();
    file_now_passing[3].passed = true;
    assert_eq!(ratchet(&file_now_passing, &listed).len(), 1);
    let stale = parse_listed("group:nothing\ncase:nope\nfile:nope.jsonl\n").unwrap();
    assert_eq!(
        ratchet(&units, &stale).len(),
        3 + 3,
        "three stale entries and the three failures that are not listed"
    );
    assert!(parse_listed("whatever").is_err());
}
