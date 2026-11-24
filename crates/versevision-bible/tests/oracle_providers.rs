//! The differential tests: what the Python providers did (recorded by `python/scripts/oracle_providers.py` into `oracle/providers/`) against
//! the Rust providers, on the same recorded-shape responses. No real network: the HTTP side is a local server (`tests/common`).
//!
//! `cargo test -p versevision-bible --test oracle_providers -- --nocapture` prints the counts.

mod common;

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use common::*;
use serde_json::{json, Value};
use versevision_bible::{
    api_bible_verse_text, html::soup_text, pyjson, AnyProvider, RegistryBuilder, ScriptureCache,
};
use versevision_core::bibles::BiblePassages;
use versevision_core::notes::TraceLog;
use versevision_core::references::{parse_references, PassageReference};

/// Cases the Rust port is known not to match yet (`file:line-number`); the test fails when one of them passes, so the list only shrinks.
const KNOWN_DIFFERENCES: &[&str] = &[];

struct Tally {
    name: &'static str,
    total: usize,
    failures: Vec<String>,
}

impl Tally {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            total: 0,
            failures: Vec::new(),
        }
    }

    fn check(&mut self, label: String, expected: &Value, actual: &Value) {
        self.total += 1;
        let known = KNOWN_DIFFERENCES.contains(&label.as_str());
        if expected == actual {
            if known {
                self.failures.push(format!(
                    "{label}: passes now, remove it from KNOWN_DIFFERENCES"
                ));
            }
        } else if !known {
            self.failures.push(format!(
                "{label}\n    python: {}\n    rust:   {}",
                short(expected),
                short(actual)
            ));
        }
    }

    fn report(self) {
        let bad = self.failures.len();
        println!(
            "oracle/providers/{}: {}/{} identical",
            self.name,
            self.total - bad.min(self.total),
            self.total
        );
        assert!(
            self.failures.is_empty(),
            "{} differs from Python in {bad} place(s):\n{}",
            self.name,
            self.failures.join("\n")
        );
    }
}

fn short(value: &Value) -> String {
    let text = value.to_string();
    if text.chars().count() > 700 {
        format!("{}...", text.chars().take(700).collect::<String>())
    } else {
        text
    }
}

#[test]
fn html_text_is_python_beautifulsoup() {
    let mut tally = Tally::new("html_text.jsonl");
    for (i, row) in jsonl("html_text.jsonl").iter().enumerate() {
        let html = row["html"].as_str().unwrap();
        tally.check(
            format!("html_text.jsonl:{} soup of {html:?}", i + 1),
            &row["soup"],
            &json!(soup_text(html)),
        );
        tally.check(
            format!("html_text.jsonl:{} text of {html:?}", i + 1),
            &row["text"],
            &json!(api_bible_verse_text(html)),
        );
    }
    tally.report();
}

#[test]
fn json_errors_are_python_json_errors() {
    let mut tally = Tally::new("json_errors.jsonl");
    for (i, row) in jsonl("json_errors.jsonl").iter().enumerate() {
        let body: Vec<u8> = match row.get("hex").and_then(Value::as_str) {
            Some(hex) => (0..hex.len())
                .step_by(2)
                .map(|k| u8::from_str_radix(&hex[k..k + 2], 16).unwrap())
                .collect(),
            None => row["body"].as_str().unwrap().as_bytes().to_vec(),
        };
        tally.check(
            format!(
                "json_errors.jsonl:{} {:?}",
                i + 1,
                String::from_utf8_lossy(&body)
            ),
            &row["message"],
            &json!(pyjson::loads_error(&body)),
        );
    }
    tally.report();
}

/// The folder of a scenario as the Rust side writes it in the config: absolute, and not in its canonical form (so the text of
/// `{path}` and the text of `{resolved}` can never be the same string).
fn usfm_path(relative: &str) -> String {
    let (head, tail) = relative.rsplit_once('/').unwrap();
    format!(
        "{}/{head}/../{}/{tail}",
        repo_root().display(),
        head.rsplit('/').next().unwrap()
    )
    .replace('\\', "/")
}

fn resolved_of(path: &str) -> String {
    let full = fs::canonicalize(path)
        .or_else(|_| std::path::absolute(path))
        .unwrap();
    let text = full.to_string_lossy().to_string();
    text.strip_prefix(r"\\?\")
        .map(str::to_string)
        .unwrap_or(text)
}

/// `{path}` for the folder as the config says it, `{resolved}` for `provider_version_id`.
fn scrub(value: &Value, path: &str, resolved: &str) -> Value {
    match value {
        Value::String(text) => Value::String(text.replace(path, "{path}")),
        Value::Array(items) => {
            Value::Array(items.iter().map(|v| scrub(v, path, resolved)).collect())
        }
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| {
                    let v = if k == "provider_version_id" && v.as_str() == Some(resolved) {
                        json!("{resolved}")
                    } else {
                        scrub(v, path, resolved)
                    };
                    (k.clone(), v)
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn usfm_provider_is_python_provider() {
    let mut tally = Tally::new("usfm.jsonl");
    for (i, row) in jsonl("usfm.jsonl").iter().enumerate() {
        let path = usfm_path(row["dir"].as_str().unwrap());
        let resolved = resolved_of(&path);
        let mut fields = row["config"].clone();
        fields["path"] = json!(path);
        let config = provider_config("usfm", &fields);
        let bible = versevision_core::config::BibleConfig {
            providers: [("oracle_usfm".to_string(), config)].into_iter().collect(),
            ..Default::default()
        };
        let mut registry = RegistryBuilder::new(keys_with(&[])).build(&bible);
        let provider = registry.providers.shift_remove("oracle_usfm").unwrap();
        for (k, call) in row["calls"].as_array().unwrap().iter().enumerate() {
            let actual = scrub(&run_call(&provider, call).await, &path, &resolved);
            tally.check(
                format!(
                    "usfm.jsonl:{} {} call {}",
                    i + 1,
                    row["name"].as_str().unwrap(),
                    k + 1
                ),
                &row["results"][k],
                &actual,
            );
        }
    }
    tally.report();
}

async fn http_provider_rows(kind: &'static str, file: &'static str) {
    let mut tally = Tally::new(file);
    for (i, row) in jsonl(file).iter().enumerate() {
        let name = row["name"].as_str().unwrap();
        let mut script = Script::new(&row["routes"]);
        // a timeout scenario sets a short timeout; the server answers later than that
        Arc::get_mut(&mut script).unwrap().timeout_for = std::time::Duration::from_secs(3);
        let addr = serve(script.clone()).await;
        let base_path = row
            .get("base_path")
            .and_then(Value::as_str)
            .unwrap_or("/v1");
        let base_url = format!("http://{addr}{base_path}");
        let keys = if row["key"].as_bool().unwrap_or(true) {
            keys_with(&[(env_name(kind), SYNTHETIC_KEY)])
        } else {
            keys_with(&[])
        };
        let provider = build_provider(kind, row, &base_url, keys);
        for (k, call) in row["calls"].as_array().unwrap().iter().enumerate() {
            let actual = run_call(&provider, call).await;
            tally.check(
                format!("{file}:{} {name} call {}", i + 1, k + 1),
                &row["results"][k],
                &actual,
            );
        }
        let requests = Value::Array(script.log.lock().unwrap().clone());
        tally.check(
            format!("{file}:{} {name} requests", i + 1),
            &row["requests"],
            &requests,
        );
    }
    tally.report();
}

#[tokio::test(flavor = "multi_thread")]
async fn api_bible_provider_is_python_provider() {
    http_provider_rows("api_bible", "api_bible.jsonl").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn youversion_provider_is_python_provider() {
    http_provider_rows("youversion", "youversion.jsonl").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn esv_provider_is_python_provider() {
    http_provider_rows("esv", "esv.jsonl").await;
}

fn reference_of(text: &str) -> PassageReference {
    let parsed = parse_references(text, None).unwrap();
    serde_json::from_value(parsed["references"][0].clone()).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn registry_is_python_registry() {
    let mut tally = Tally::new("registry.jsonl");
    for (i, row) in jsonl("registry.jsonl").iter().enumerate() {
        let name = row["name"].as_str().unwrap();
        let script = Script::new(&row["routes"]);
        let addr = serve(script.clone()).await;
        let path = usfm_path("oracle/providers/usfm/basic");
        let resolved = resolved_of(&path);
        let bible_json = row["bible"]
            .to_string()
            .replace("oracle/providers/usfm/basic", &path);
        let bible = bible_config(
            &serde_json::from_str(&bible_json).unwrap(),
            &format!("http://{addr}"),
        );
        let entries: Vec<(String, String)> = row["env"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
            .collect();
        let entries: Vec<(&str, &str)> = entries
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let directory = tempfile::tempdir().unwrap();
        let cache = Arc::new(ScriptureCache::new(directory.path()));
        let registry = RegistryBuilder::new(keys_with(&entries))
            .cache(cache.clone())
            .build(&bible);

        let mut actual = Vec::new();
        for step in row["steps"].as_array().unwrap() {
            let reference = reference_of(step.as_str().unwrap());
            let mut trace = TraceLog::default();
            let outcome = registry.fetch(&reference, &mut trace).await;
            let record = trace.records.last().expect("a provider record");
            assert_eq!(record.0, "provider");
            actual.push(match outcome {
                Ok(passage) => json!({
                    "provider": record.1["provider"],
                    "source": record.1["source"],
                    "resolved_version": passage.resolved_version,
                    "verses": passage.verses,
                }),
                Err(error) => json!({"error": error.message(), "class": error.python_exception()}),
            });
        }
        actual.push(json!({ "cache_count": cache.count() }));
        for (k, result) in actual.iter().enumerate() {
            tally.check(
                format!("registry.jsonl:{} {name} step {}", i + 1, k + 1),
                &row["results"][k],
                &scrub(result, &path, &resolved),
            );
        }
        // what was asked of the servers, in order
        let requests = Value::Array(script.log.lock().unwrap().clone());
        tally.check(
            format!("registry.jsonl:{} {name} requests", i + 1),
            &row["requests"],
            &requests,
        );
    }
    tally.report();
}

/// The providers' futures are `Send` (they run on StagePilot's tokio runtime) and the registry is `Send + Sync`.
#[test]
fn providers_and_registry_futures_are_send() {
    fn is_send<T: Send>(_: &T) {}
    fn is_send_sync<T: Send + Sync>() {}
    is_send_sync::<AnyProvider>();
    is_send_sync::<versevision_core::bibles::ProviderRegistry<AnyProvider>>();
    let registry = RegistryBuilder::new(keys_with(&[])).build(&Default::default());
    let reference = reference_of("John 3:16 NIV");
    let mut trace = TraceLog::default();
    let future = registry.fetch(&reference, &mut trace);
    is_send(&future);
    drop(future);
    let _: PathBuf = repo_root();
}
