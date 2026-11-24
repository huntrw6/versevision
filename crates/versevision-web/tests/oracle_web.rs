//! The data routes answer what the Python server answers (`oracle/web/`, written by `python/scripts/oracle_web.py`): the same status codes
//! and the same JSON for status, config (and its schema), history, runs, themes, check, presentations and the refusals, starting from the same
//! files. Numbers compare by value (Python's `5` and Rust's `5.0` are the same limit), objects without regard to key order.

mod common;
use common::*;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::http::Method;
use serde_json::{json, Value};
use versevision_core::config::AppConfig;
use versevision_pp::dirs::Dirs;
use versevision_pp::errors::{VvError, VvResult};
use versevision_pp::transport::Transport;
use versevision_web::{configpage, WebState};

fn same(path: &str, a: &Value, b: &Value, out: &mut Vec<String>) {
    if out.len() > 8 {
        return;
    }
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            if x.as_f64() != y.as_f64() {
                out.push(format!("{path}: {x} != {y}"));
            }
        }
        (Value::Object(x), Value::Object(y)) => {
            for key in x.keys().chain(y.keys().filter(|k| !x.contains_key(*k))) {
                match (x.get(key), y.get(key)) {
                    (Some(p), Some(q)) => same(&format!("{path}.{key}"), p, q, out),
                    (p, q) => out.push(format!("{path}.{key}: python {p:?} / rust {q:?}")),
                }
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                out.push(format!("{path}: {} items / {} items", x.len(), y.len()));
            }
            for (i, (p, q)) in x.iter().zip(y).enumerate() {
                same(&format!("{path}[{i}]"), p, q, out);
            }
        }
        _ => {
            if a != b {
                out.push(format!("{path}: python {a} / rust {b}"));
            }
        }
    }
}

/// A path that starts with `<ROOT>` is compared with forward slashes (what Python wrote), whatever the system writes.
fn slash_paths(value: &mut Value) {
    match value {
        Value::String(s) if s.starts_with("<ROOT>") => *s = s.replace('\\', "/"),
        Value::Array(items) => items.iter_mut().for_each(slash_paths),
        Value::Object(map) => map.values_mut().for_each(slash_paths),
        _ => {}
    }
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// `FakeTransport` of the Python generator.
struct StaticTransport {
    themes: PathBuf,
}

fn refused(what: &str) -> VvError {
    VvError::general(format!("{what} is not part of this fake"))
}

impl Transport for StaticTransport {
    fn description(&self) -> String {
        "fake ProPresenter".into()
    }
    fn api_get(&self, route: &str) -> VvResult<Value> {
        Ok(match route {
            "/version" => json!({"host_description": "ProPresenter 19.0", "api_version": "v1"}),
            "/v1/libraries" => {
                json!([{"uuid": "L1", "name": "Verses"}, {"uuid": "L2", "name": "Songs"}])
            }
            "/v1/library/L1" => {
                json!({"items": [{"uuid": "P1", "name": "Sample"}, {"uuid": "P2", "name": "Weekly"}]})
            }
            "/v1/library/L2" => json!({"items": [{"uuid": "S1", "name": "Amazing Grace"}]}),
            other => return Err(refused(other)),
        })
    }
    fn api_put(&self, _: &str, _: &Value) -> VvResult<()> {
        Err(refused("api_put"))
    }
    fn is_running(&self) -> bool {
        true
    }
    fn quit_app(&self) -> VvResult<()> {
        Err(refused("quit"))
    }
    fn launch_app(&self) -> VvResult<()> {
        Err(refused("launch"))
    }
    fn fetch_file(&self, _: &str, _: &Path) -> VvResult<String> {
        Err(refused("fetch_file"))
    }
    fn install_file(&self, _: &Path, _: &str, _: &str) -> VvResult<String> {
        Err(refused("install_file"))
    }
    fn find_presentation(&self, _: &str, _: Option<&str>) -> Vec<String> {
        Vec::new()
    }
    fn library_folder(&self, _: &str) -> String {
        String::new()
    }
    fn fetch_themes(&self, dest: &Path) -> VvResult<()> {
        copy_dir(&self.themes, dest);
        Ok(())
    }
    fn create_file(&self, _: &Path, _: &str) -> VvResult<String> {
        Err(refused("create_file"))
    }
    fn delete_created_file(&self, _: &str, _: usize) -> VvResult<String> {
        Err(refused("delete_created_file"))
    }
    fn rename_file(&self, _: &str, _: &str) -> VvResult<()> {
        Err(refused("rename_file"))
    }
    fn delete_file(&self, _: &str, _: &str) -> VvResult<String> {
        Err(refused("delete_file"))
    }
    fn backup_file(&self, _: &str) -> VvResult<String> {
        Err(refused("backup_file"))
    }
    fn restore_backup(&self, _: &str, _: &str) -> VvResult<()> {
        Err(refused("restore_backup"))
    }
}

struct Corpus {
    _root: tempfile::TempDir,
    root_text: String,
    state: WebState,
}

fn corpus() -> Corpus {
    let root = tempfile::tempdir().unwrap();
    let work = root.path().to_path_buf();
    copy_dir(&repo().join("oracle/web/state"), &work);
    let config =
        AppConfig::from_toml_str(&fs::read_to_string(work.join("config.toml")).unwrap()).unwrap();
    let dirs = Dirs {
        config: work.clone(),
        data: work.clone(),
        logs: work.join("logs"),
        keys: work.clone(),
    };
    let themes = repo().join("python/tests/fixtures/themes");
    let state = WebState::builder(dirs, config)
        .app_version("0.38.1")
        .transport(Arc::new(move |_| {
            Box::new(StaticTransport {
                themes: themes.clone(),
            })
        }))
        .build();
    Corpus {
        root_text: work.to_string_lossy().into_owned(),
        _root: root,
        state,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_data_route_answers_what_python_answers() {
    let c = corpus();
    let router = versevision_web::router(c.state.clone());
    let rows: Vec<Value> =
        serde_json::from_str(&fs::read_to_string(repo().join("oracle/web/routes.json")).unwrap())
            .unwrap();
    assert!(rows.len() >= 20, "the corpus has {} routes", rows.len());
    let mut failures = Vec::new();
    for row in &rows {
        let name = row["name"].as_str().unwrap();
        let method = if row["method"] == "POST" {
            Method::POST
        } else {
            Method::GET
        };
        let path = row["path"].as_str().unwrap();
        let answer = if name == "plan with the wrong type" {
            send(
                &router,
                Method::POST,
                path,
                &[("host", "localhost"), ("x-file-name", "notes.exe")],
                b"x".to_vec(),
            )
            .await
        } else if method == Method::POST {
            post_json(&router, path, row.get("body").unwrap_or(&json!({}))).await
        } else {
            get(&router, path).await
        };
        if answer.status as u64 != row["status"].as_u64().unwrap() {
            failures.push(format!(
                "{name}: status {} instead of {}",
                answer.status, row["status"]
            ));
            continue;
        }
        // the folder the files were copied to is what Python called <ROOT> (on Windows its name has backslashes, which JSON writes doubled,
        // and the paths below it are written with them too)
        let escaped = c.root_text.replace('\\', "\\\\");
        let text = answer
            .text()
            .replace(&escaped, "<ROOT>")
            .replace(&c.root_text, "<ROOT>");
        let mut got: Value = serde_json::from_str(&text).unwrap();
        slash_paths(&mut got);
        let mut diffs = Vec::new();
        same(name, &row["json"], &got, &mut diffs);
        failures.extend(diffs);
    }
    assert!(
        failures.is_empty(),
        "differences from Python:\n{}",
        failures.join("\n")
    );
}

#[test]
fn applying_values_gives_python_s_errors_and_values() {
    let text = fs::read_to_string(repo().join("oracle/web/config_apply.jsonl")).unwrap();
    let base = AppConfig::from_toml_str(
        &fs::read_to_string(repo().join("oracle/web/state/config.toml")).unwrap(),
    )
    .unwrap();
    let mut failures = Vec::new();
    let mut count = 0;
    for line in text.lines() {
        let case: Value = serde_json::from_str(line).unwrap();
        count += 1;
        let name = case["name"].as_str().unwrap();
        let values = case["values"].as_object().unwrap();
        match configpage::apply(&base, values) {
            Err(message) => {
                if case["error"] != json!(message) {
                    failures.push(format!(
                        "{name}: error {message:?} instead of {}",
                        case["error"]
                    ));
                }
            }
            Ok(new) => {
                if !case["error"].is_null() {
                    failures.push(format!(
                        "{name}: accepted, Python refused with {}",
                        case["error"]
                    ));
                    continue;
                }
                let page = configpage::describe(&new, Path::new("x"));
                let mut shown = serde_json::Map::new();
                for section in page["sections"].as_array().unwrap() {
                    for field in section["fields"].as_array().unwrap() {
                        shown.insert(
                            field["key"].as_str().unwrap().into(),
                            field["value"].clone(),
                        );
                    }
                }
                same(name, &case["after"], &Value::Object(shown), &mut failures);
            }
        }
    }
    assert!(count >= 20);
    assert!(
        failures.is_empty(),
        "differences from Python:\n{}",
        failures.join("\n")
    );
}

#[test]
fn the_page_schema_has_every_option_with_its_help_label_and_bounds() {
    let page = configpage::describe(&AppConfig::default(), Path::new("x"));
    let mut seen = 0;
    for section in page["sections"].as_array().unwrap() {
        for f in section["fields"].as_array().unwrap() {
            seen += 1;
            let key = f["key"].as_str().unwrap();
            let help = f["help"].as_str().unwrap();
            let label = f["label"].as_str().unwrap();
            assert!(
                !help.is_empty() && help.chars().count() <= 70,
                "{key}: help {help:?}"
            );
            assert!(
                !label.is_empty() && label.chars().count() <= 52,
                "{key}: label {label:?}"
            );
            assert!(!help.contains('\n'));
        }
    }
    assert_eq!(seen, configpage::editable_keys().len());
    assert!(seen >= 50);
}
