//! The command line against `oracle/cli/` (what the Python app printed and returned for the same commands in the same kind of sandbox).
//!
//! Every case of `cases.jsonl` is run through the real binary: setup files are written, each step's arguments and standard input go in, and
//! the exit code, standard output, standard error and the files left behind must be the bytes Python produced. `{ROOT}` stands for the
//! sandbox folder. A case that is known to differ is listed in `tests/known_differences.txt` (`name | reason`); the list may only
//! shrink: a listed case that now passes fails the test, an unlisted case that fails fails it too.
//!
//! Set `ORACLE_ONLY=text` to run only the cases whose name contains `text`, and `ORACLE_SHOW=1` to print the first differences.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

fn oracle_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../oracle/cli")
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn setup(root: &Path, ops: &[Value]) {
    fs::create_dir_all(root.join("home")).unwrap();
    let text_root = root.to_string_lossy().replace('\\', "/");
    for op in ops {
        match op["op"].as_str().unwrap() {
            "write" => {
                let target = root.join(op["path"].as_str().unwrap());
                fs::create_dir_all(target.parent().unwrap()).unwrap();
                fs::write(
                    target,
                    op["text"].as_str().unwrap().replace("{ROOT}", &text_root),
                )
                .unwrap();
            }
            "copy" => {
                let source = repo().join(op["from"].as_str().unwrap());
                let target = root.join(op["to"].as_str().unwrap());
                copy_all(&source, &target);
            }
            "seed_runs" => {
                let base = repo()
                    .join("oracle/pipeline/scenarios")
                    .join(op["scenario"].as_str().unwrap())
                    .join("expected/runs");
                let mut runs: Vec<_> = fs::read_dir(base).unwrap().flatten().collect();
                runs.sort_by_key(|e| e.file_name());
                for run in runs {
                    let name = fs::read_to_string(run.path().join("name")).unwrap();
                    let dest = root.join("logs/runs").join(name.trim());
                    fs::create_dir_all(&dest).unwrap();
                    for item in fs::read_dir(run.path()).unwrap().flatten() {
                        if item.file_name() != "name" {
                            fs::copy(item.path(), dest.join(item.file_name())).unwrap();
                        }
                    }
                }
            }
            "seed_history" => {
                let source = repo()
                    .join("oracle/pipeline/scenarios")
                    .join(op["scenario"].as_str().unwrap())
                    .join("expected/history.json");
                let text = fs::read_to_string(source)
                    .unwrap()
                    .replace("<ROOT>", &text_root);
                fs::write(root.join("imports.json"), text).unwrap();
            }
            other => panic!("unknown setup op {other}"),
        }
    }
}

fn copy_all(source: &Path, target: &Path) {
    if source.is_dir() {
        fs::create_dir_all(target).unwrap();
        for entry in fs::read_dir(source).unwrap().flatten() {
            copy_all(&entry.path(), &target.join(entry.file_name()));
        }
    } else {
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::copy(source, target).unwrap();
    }
}

/// Windows prints paths with backslashes; the oracle has slashes.
fn normalise(text: &str, root: &Path) -> String {
    let mut out = text.to_string();
    for variant in [
        root.to_string_lossy().to_string(),
        fs::canonicalize(root)
            .map(|p| p.to_string_lossy().trim_start_matches(r"\\?\").to_string())
            .unwrap_or_default(),
    ] {
        if !variant.is_empty() {
            out = out.replace(&variant, "{ROOT}");
        }
    }
    if cfg!(windows) {
        // inside a `{ROOT}...` token turn the separators into slashes
        let mut fixed = String::new();
        let mut rest = out.as_str();
        while let Some(at) = rest.find("{ROOT}") {
            fixed.push_str(&rest[..at]);
            let tail = &rest[at..];
            let end = tail
                .find(|c: char| c.is_whitespace() || c == '\'' || c == '"')
                .unwrap_or(tail.len());
            fixed.push_str(&tail[..end].replace('\\', "/"));
            rest = &tail[end..];
        }
        fixed.push_str(rest);
        out = fixed;
    }
    out
}

struct Answer {
    exit: i64,
    stdout: String,
    stderr: String,
}

fn run_step(
    root: &Path,
    argv: &[String],
    stdin: &str,
    env: &BTreeMap<String, String>,
    columns: Option<u64>,
) -> Answer {
    let text_root = root.to_string_lossy().to_string();
    let mut command = Command::new(env!("CARGO_BIN_EXE_versevision"));
    for arg in argv {
        command.arg(arg.replace("{ROOT}", &text_root));
    }
    for var in [
        "GITHUB_ACTIONS",
        "FORCE_COLOR",
        "PY_COLORS",
        "NO_COLOR",
        "TERM",
        "COLUMNS",
        "LINES",
        "TERMINAL_WIDTH",
        "_TYPER_FORCE_DISABLE_TERMINAL",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_CACHE_HOME",
    ] {
        command.env_remove(var);
    }
    let home = root.join("home");
    command
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("LOCALAPPDATA", home.join("AppData/Local"))
        .env("VERSEVISION_CONFIG", root.join("config.toml"))
        .env("VERSEVISION_HISTORY", root.join("imports.json"))
        .env("VERSEVISION_LOGS", root.join("logs"))
        .env("VERSEVISION_KEYS", root.join("keys.env"))
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(columns) = columns {
        command.env("COLUMNS", columns.to_string());
    }
    for (k, v) in env {
        command.env(k, v.replace("{ROOT}", &text_root));
    }
    let mut child = command.spawn().expect("the binary starts");
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).ok();
    let out = child.wait_with_output().unwrap();
    Answer {
        exit: i64::from(out.status.code().unwrap_or(-1)),
        stdout: normalise(&String::from_utf8_lossy(&out.stdout), root),
        stderr: normalise(&String::from_utf8_lossy(&out.stderr), root),
    }
}

/// The ways the Rust binary is allowed to answer differently everywhere (each is listed in the PR): the version number is the port's own.
fn expected_text(name: &str, text: &str) -> String {
    if name != "version" {
        return text.to_string();
    }
    text.replace(
        "VerseVision 0.38.1",
        &format!("VerseVision {}", env!("CARGO_PKG_VERSION")),
    )
}

fn known_differences() -> BTreeMap<String, String> {
    let text = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/known_differences.txt"),
    )
    .unwrap_or_default();
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| {
            let (name, reason) = l.split_once('|').unwrap_or((l, ""));
            (name.trim().to_string(), reason.trim().to_string())
        })
        .collect()
}

/// Remove the line of the one command Python does not have (`keys`) from a command list, so the rest can be compared byte for byte.
fn without_keys_row(text: &str) -> String {
    text.lines()
        .filter(|l| !l.starts_with("│ keys "))
        .map(|l| format!("{l}\n"))
        .collect()
}

#[test]
fn every_case_matches_python() {
    let only = std::env::var("ORACLE_ONLY").unwrap_or_default();
    let show = std::env::var("ORACLE_SHOW").is_ok();
    let known = known_differences();
    let text =
        fs::read_to_string(oracle_dir().join("cases.jsonl")).expect("oracle/cli/cases.jsonl");
    let mut failed: Vec<String> = Vec::new();
    let mut passed = 0usize;
    let mut total = 0usize;
    let scratch = tempfile::tempdir().unwrap();
    for (index, line) in text.lines().enumerate() {
        let case: Value = serde_json::from_str(line).unwrap();
        let name = case["name"].as_str().unwrap().to_string();
        if !only.is_empty() && !name.contains(&only) {
            continue;
        }
        total += 1;
        let root = scratch.path().join(format!("c{index}"));
        setup(&root, case["setup"].as_array().unwrap());
        let env: BTreeMap<String, String> = case["env"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
            .collect();
        let columns = case["columns"].as_u64();
        let mut problems: Vec<String> = Vec::new();
        for (step, expected) in case["steps"]
            .as_array()
            .unwrap()
            .iter()
            .zip(case["expected"]["steps"].as_array().unwrap())
        {
            let argv: Vec<String> = step["argv"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| a.as_str().unwrap().to_string())
                .collect();
            let answer = run_step(
                &root,
                &argv,
                step["stdin"].as_str().unwrap_or(""),
                &env,
                columns,
            );
            let mut want_out = expected_text(&name, expected["stdout"].as_str().unwrap());
            let want_err = expected_text(&name, expected["stderr"].as_str().unwrap());
            let mut got_out = answer.stdout.clone();
            if name == "help-root" || name == "root-no-args" {
                got_out = without_keys_row(&got_out);
                want_out = without_keys_row(&want_out);
            }
            if answer.exit != expected["exit"].as_i64().unwrap() {
                problems.push(format!(
                    "{argv:?}: exit {} (Python {})",
                    answer.exit, expected["exit"]
                ));
            }
            if got_out != want_out {
                problems.push(format!(
                    "{argv:?}: stdout differs\n--- Python\n{want_out}\n--- Rust\n{got_out}"
                ));
            }
            if answer.stderr != want_err {
                problems.push(format!(
                    "{argv:?}: stderr differs\n--- Python\n{want_err}\n--- Rust\n{}",
                    answer.stderr
                ));
            }
        }
        for (rel, want) in case["expected"]["files"].as_object().unwrap() {
            let got = fs::read_to_string(root.join(rel))
                .ok()
                .map(|t| normalise(&t, &root));
            if got.as_deref() != want.as_str() {
                problems.push(format!("file {rel} differs"));
            }
        }
        if problems.is_empty() {
            passed += 1;
            if known.contains_key(&name) {
                failed.push(format!(
                    "{name}: now passes; remove it from tests/known_differences.txt"
                ));
            }
        } else if !known.contains_key(&name) {
            if show {
                eprintln!("=== {name}\n{}", problems.join("\n"));
            }
            failed.push(format!(
                "{name}: {}",
                problems[0].lines().next().unwrap_or("")
            ));
        }
    }
    eprintln!(
        "oracle/cli: {passed}/{total} cases identical to Python ({} known differences)",
        known.len()
    );
    assert!(
        failed.is_empty(),
        "cases that differ from Python:\n{}",
        failed.join("\n")
    );
}

#[test]
fn every_command_has_a_help_case() {
    use clap::CommandFactory;
    let text = fs::read_to_string(oracle_dir().join("cases.jsonl")).unwrap();
    let names: Vec<String> = text
        .lines()
        .map(|l| {
            serde_json::from_str::<Value>(l).unwrap()["name"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    fn walk(cmd: &clap::Command, path: Vec<String>, out: &mut Vec<Vec<String>>) {
        out.push(path.clone());
        for sub in cmd.get_subcommands() {
            let mut next = path.clone();
            next.push(sub.get_name().to_string());
            walk(sub, next, out);
        }
    }
    let mut paths = Vec::new();
    walk(
        &versevision_cli::def::Cli::command(),
        Vec::new(),
        &mut paths,
    );
    for path in paths {
        if path.first().is_some_and(|p| p == "keys") {
            continue; // the one command Python does not have
        }
        let wanted = format!(
            "help-{}",
            if path.is_empty() {
                "root".to_string()
            } else {
                path.join("-")
            }
        );
        assert!(names.contains(&wanted), "no help case {wanted}");
    }
}
