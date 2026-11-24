//! The document readers against the oracle (`oracle documents-extract`).
//!
//! * `oracle/documents/<case>/`: `input.<ext>` read by the Rust reader must give `expected/extracted.txt` and the `extract` record
//!   `expected/extract.json` (Python returned), or the exact error `expected/error.json` (Python raised).
//! * `oracle/cases/<case>/`: the same for every whole-document case (`expected/extracted.txt` and the first record of
//!   `expected/trace.jsonl`), for every input type including `.txt` and `.md`.
//!
//! `cargo test -p versevision-docs --test oracle_documents -- --nocapture` prints the counts per type. Nothing here is listed as an
//! expected failure: every case passes.
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use versevision_docs::{extract_text, DocumentError};

fn oracle_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../oracle")
}

fn input_of(folder: &Path) -> Option<(PathBuf, String)> {
    for entry in fs::read_dir(folder).ok()? {
        let path = entry.ok()?.path();
        let name = path.file_name()?.to_str()?.to_string();
        if let Some(ext) = name.strip_prefix("input.") {
            return Some((path.clone(), ext.to_string()));
        }
    }
    None
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(
        &fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
    )
    .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[derive(Default)]
struct Tally {
    passed: BTreeMap<String, usize>,
    failed: BTreeMap<String, usize>,
    failures: Vec<String>,
}

impl Tally {
    fn record(&mut self, key: String, result: Result<(), String>) {
        match result {
            Ok(()) => *self.passed.entry(key).or_default() += 1,
            Err(message) => {
                *self.failed.entry(key.clone()).or_default() += 1;
                self.failures.push(format!("{key}: {message}"));
            }
        }
    }

    fn report(&self, title: &str) {
        println!("{title}");
        let keys: std::collections::BTreeSet<&String> =
            self.passed.keys().chain(self.failed.keys()).collect();
        for key in keys {
            let ok = self.passed.get(key).copied().unwrap_or(0);
            let bad = self.failed.get(key).copied().unwrap_or(0);
            println!("   {key:<14} {ok}/{}", ok + bad);
        }
        let total_ok: usize = self.passed.values().sum();
        let total_bad: usize = self.failed.values().sum();
        println!("   total          {total_ok}/{}", total_ok + total_bad);
    }
}

fn show(text: &str) -> String {
    let shown: String = text.chars().take(160).collect();
    format!("{shown:?}")
}

fn check_text(expected: &str, actual: &str) -> Result<(), String> {
    if expected == actual {
        return Ok(());
    }
    let at = expected
        .chars()
        .zip(actual.chars())
        .position(|(a, b)| a != b)
        .unwrap_or(expected.chars().count().min(actual.chars().count()));
    let from = at.saturating_sub(30);
    let cut = |s: &str| s.chars().skip(from).take(80).collect::<String>();
    Err(format!(
        "text differs at character {at}: expected {:?} got {:?}",
        cut(expected),
        cut(actual)
    ))
}

fn run_documents_case(folder: &Path) -> Result<(), String> {
    let (input, ext) = input_of(folder).ok_or("no input file")?;
    let bytes = fs::read(&input).map_err(|e| e.to_string())?;
    let result = extract_text(&bytes, &format!("upload.{ext}"));
    let error_file = folder.join("expected/error.json");
    if error_file.exists() {
        let expected = read_json(&error_file);
        return match result {
            Ok((text, _)) => Err(format!(
                "Python raised {} but the reader returned {}",
                expected["message"],
                show(&text)
            )),
            Err(error) => {
                let kind = error.kind();
                let message = error.message();
                if expected["message"] == Value::String(message.clone())
                    && expected["kind"] == Value::String(kind.to_string())
                {
                    Ok(())
                } else {
                    Err(format!(
                        "expected {} {} got {kind} {message:?}",
                        expected["kind"], expected["message"]
                    ))
                }
            }
        };
    }
    let (text, info) = result.map_err(|e: DocumentError| {
        format!(
            "the reader failed ({}: {:?}) where Python returned text",
            e.kind(),
            e.message()
        )
    })?;
    let expected =
        fs::read_to_string(folder.join("expected/extracted.txt")).map_err(|e| e.to_string())?;
    check_text(&expected, &text)?;
    let expected_info = read_json(&folder.join("expected/extract.json"));
    if info.to_json() != expected_info {
        return Err(format!(
            "extract record differs: expected {expected_info} got {}",
            info.to_json()
        ));
    }
    Ok(())
}

fn run_whole_case(folder: &Path) -> Result<(), String> {
    let (input, ext) = input_of(folder).ok_or("no input file")?;
    let bytes = fs::read(&input).map_err(|e| e.to_string())?;
    let (text, info) = extract_text(&bytes, &format!("input.{ext}"))
        .map_err(|e| format!("the reader failed: {:?}", e.message()))?;
    let expected =
        fs::read_to_string(folder.join("expected/extracted.txt")).map_err(|e| e.to_string())?;
    check_text(&expected, &text)?;
    let trace =
        fs::read_to_string(folder.join("expected/trace.jsonl")).map_err(|e| e.to_string())?;
    let first = trace.split('\n').next().unwrap_or("");
    let mut record: Value = serde_json::from_str(first).map_err(|e| e.to_string())?;
    if record["stage"] != "extract" {
        return Err(format!(
            "the trace does not start with the extract record: {first}"
        ));
    }
    if let Value::Object(map) = &mut record {
        map.remove("seq");
        map.remove("stage");
    }
    if info.to_json() != record {
        return Err(format!(
            "extract record differs: expected {record} got {}",
            info.to_json()
        ));
    }
    Ok(())
}

fn sorted_folders(dir: &Path) -> Vec<PathBuf> {
    let mut folders: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    folders.sort();
    folders
}

fn key_of(folder: &Path) -> String {
    let ext = input_of(folder).map(|(_, e)| e).unwrap_or_default();
    let outcome = if folder.join("expected/error.json").exists() {
        "errors"
    } else {
        "text"
    };
    format!(".{ext} {outcome}")
}

#[test]
fn documents_oracle() {
    let mut tally = Tally::default();
    for folder in sorted_folders(&oracle_dir().join("documents")) {
        let name = folder.file_name().unwrap().to_string_lossy().to_string();
        let key = key_of(&folder);
        tally.record(
            key,
            run_documents_case(&folder).map_err(|m| format!("{name}: {m}")),
        );
    }
    tally.report("oracle/documents (the readers on their own)");
    assert!(
        tally.failures.is_empty(),
        "{} failures:\n{}",
        tally.failures.len(),
        tally.failures.join("\n")
    );
}

#[test]
fn whole_document_cases() {
    let mut tally = Tally::default();
    for folder in sorted_folders(&oracle_dir().join("cases")) {
        let name = folder.file_name().unwrap().to_string_lossy().to_string();
        let ext = input_of(&folder).map(|(_, e)| e).unwrap_or_default();
        tally.record(
            format!(".{ext}"),
            run_whole_case(&folder).map_err(|m| format!("{name}: {m}")),
        );
    }
    tally
        .report("oracle/cases (the reader's text and extract record of every whole-document case)");
    assert!(
        tally.failures.is_empty(),
        "{} failures:\n{}",
        tally.failures.len(),
        tally.failures.join("\n")
    );
}
