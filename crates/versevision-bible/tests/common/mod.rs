//! Shared by the integration tests: the oracle files, a local HTTP server that plays a scenario's canned responses, and a runner
//! that makes the Rust providers do what the Python exporter had its providers do.
#![allow(dead_code)]

pub mod service;

use std::collections::HashMap;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, Response, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use versevision_bible::{AnyProvider, KeyStore, MemoryKeyStore, RegistryBuilder, Secret};
use versevision_core::bibles::{BibleError, BibleProvider};
use versevision_core::config::{AppConfig, BibleConfig, ProviderConfig};
use versevision_core::references::{VersePoint, VerseSpan};

pub const SYNTHETIC_KEY: &str = "synthetic-key-0001-not-real";
pub const ORIGIN: &str = "https://oracle.test";

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn oracle_dir() -> PathBuf {
    repo_root().join("oracle/providers")
}

pub fn jsonl(name: &str) -> Vec<Value> {
    let path = oracle_dir().join(name);
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    text.lines()
        .enumerate()
        .map(|(i, line)| {
            serde_json::from_str(line).unwrap_or_else(|e| panic!("{name} line {}: {e}", i + 1))
        })
        .collect()
}

/// `env_name` of a kind (what the exporter used).
pub fn env_name(kind: &str) -> &'static str {
    match kind {
        "api_bible" => "VERSEVISION_ORACLE_API_BIBLE_KEY",
        "youversion" => "VERSEVISION_ORACLE_YOUVERSION_KEY",
        _ => "VERSEVISION_ORACLE_ESV_KEY",
    }
}

// ------------------------------------------------------------------------------------------------------------- server
/// A scenario's canned responses by path; the nth request to a path gets the nth response, the last one repeats.
pub struct Script {
    routes: HashMap<String, Vec<Value>>,
    counts: Mutex<HashMap<String, usize>>,
    pub log: Mutex<Vec<Value>>,
    pub timeout_for: Duration,
}

impl Script {
    pub fn new(routes: &Value) -> Arc<Self> {
        let routes = routes
            .as_object()
            .map(|map| {
                map.iter()
                    .map(|(path, responses)| {
                        (
                            path.clone(),
                            responses.as_array().cloned().unwrap_or_default(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        Arc::new(Self {
            routes,
            counts: Mutex::new(HashMap::new()),
            log: Mutex::new(Vec::new()),
            timeout_for: Duration::from_secs(3),
        })
    }
}

async fn handle(State(script): State<Arc<Script>>, request: Request<Body>) -> Response<Body> {
    let raw_path = request.uri().path().to_string();
    let path = percent_encoding::percent_decode_str(&raw_path)
        .decode_utf8_lossy()
        .to_string();
    let url = reqwest::Url::parse(&format!(
        "http://x/?{}",
        request.uri().query().unwrap_or("")
    ))
    .expect("query");
    let mut query: Vec<Value> = url.query_pairs().map(|(k, v)| json!([k, v])).collect();
    query.sort_by_key(|pair| pair.to_string());
    let mut query: Vec<(String, String)> = query
        .iter()
        .map(|p| {
            (
                p[0].as_str().unwrap().to_string(),
                p[1].as_str().unwrap().to_string(),
            )
        })
        .collect();
    query.sort();
    let mut headers = serde_json::Map::new();
    for name in ["api-key", "x-yvp-app-key", "authorization"] {
        if let Some(value) = request.headers().get(name) {
            headers.insert(name.to_string(), json!(value.to_str().unwrap_or("?")));
        }
    }
    script.log.lock().unwrap().push(json!({
        "method": request.method().as_str(),
        "path": path,
        "query": query.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
        "headers": headers,
    }));
    let Some(responses) = script.routes.get(&path).filter(|r| !r.is_empty()) else {
        return respond(404, &[], br#"{"message":"no such route"}"#.to_vec());
    };
    let index = {
        let mut counts = script.counts.lock().unwrap();
        let count = counts.entry(path.clone()).or_insert(0);
        let index = (*count).min(responses.len() - 1);
        *count += 1;
        index
    };
    let spec = &responses[index];
    if spec.get("timeout").and_then(Value::as_bool) == Some(true) {
        tokio::time::sleep(script.timeout_for).await;
        return respond(200, &[], b"{}".to_vec());
    }
    let status = spec.get("status").and_then(Value::as_u64).unwrap_or(200) as u16;
    let headers: Vec<(String, String)> = spec
        .get("headers")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                .collect()
        })
        .unwrap_or_default();
    let body = if let Some(json) = spec.get("json") {
        serde_json::to_vec(json).unwrap()
    } else if let Some(text) = spec.get("text").and_then(Value::as_str) {
        text.as_bytes().to_vec()
    } else if let Some(hex) = spec.get("hex").and_then(Value::as_str) {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    } else {
        Vec::new()
    };
    respond(status, &headers, body)
}

fn respond(status: u16, headers: &[(String, String)], body: Vec<u8>) -> Response<Body> {
    let mut builder = Response::builder().status(StatusCode::from_u16(status).unwrap());
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    builder.body(Body::from(body)).unwrap()
}

/// Serve `script` on a free local port; the server stops when the returned handle is dropped with the runtime.
pub async fn serve(script: Arc<Script>) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = Router::new().fallback(handle).with_state(script);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

// ------------------------------------------------------------------------------------------------------------ calls
pub fn span_of(raw: &Value) -> VerseSpan {
    let n: Vec<u64> = raw
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap())
        .collect();
    VerseSpan {
        start: VersePoint {
            chapter: n[0],
            verse: n[1],
        },
        end: (n.len() > 2).then(|| VersePoint {
            chapter: n[2],
            verse: n[3],
        }),
    }
}

/// One call of a scenario against a provider, as the exporter records it: `{"ok": ...}` or `{"error": text, "class": name}`.
pub async fn run_call<P: BibleProvider>(provider: &P, call: &Value) -> Value {
    let result: Result<Value, BibleError> = match call["op"].as_str().unwrap() {
        "versions" => provider
            .list_versions()
            .await
            .map(|v| serde_json::to_value(v).unwrap()),
        "health" => Ok(serde_json::to_value(provider.health_check().await).unwrap()),
        "fetch" => {
            let spans: Vec<VerseSpan> = call["spans"]
                .as_array()
                .unwrap()
                .iter()
                .map(span_of)
                .collect();
            provider
                .fetch_passage(
                    call["version"].as_str().unwrap(),
                    call["book"].as_str().unwrap(),
                    &spans,
                )
                .await
                .map(|v| serde_json::to_value(v).unwrap())
        }
        other => panic!("unknown op {other}"),
    };
    match result {
        Ok(value) => json!({ "ok": value }),
        Err(error) => json!({"error": error.message(), "class": error.python_exception()}),
    }
}

/// The provider config of a scenario, validated by the core config model (its defaults and ranges).
pub fn provider_config(kind: &str, fields: &Value) -> ProviderConfig {
    let mut table = fields.as_object().cloned().unwrap_or_default();
    table.insert("type".into(), json!(kind));
    let config =
        AppConfig::from_value(&json!({"bible": {"providers": {"p": Value::Object(table)}}}))
            .unwrap_or_else(|e| panic!("provider config: {e:?}"));
    config.bible.providers["p"].clone()
}

pub fn bible_config(bible: &Value, origin: &str) -> BibleConfig {
    let text = bible.to_string().replace(ORIGIN, origin);
    let bible: Value = serde_json::from_str(&text).unwrap();
    AppConfig::from_value(&json!({ "bible": bible }))
        .unwrap_or_else(|e| panic!("bible config: {e:?}"))
        .bible
}

pub fn keys_with(entries: &[(&str, &str)]) -> Arc<MemoryKeyStore> {
    let keys = MemoryKeyStore::new();
    for (name, value) in entries {
        keys.set_key(name, Secret::new(*value)).unwrap();
    }
    Arc::new(keys)
}

/// A one-provider registry-less provider for a scenario of `kind`, named like the exporter named it.
pub fn build_provider(
    kind: &str,
    scenario: &Value,
    base_url: &str,
    keys: Arc<dyn KeyStore>,
) -> AnyProvider {
    let mut fields = scenario.get("config").cloned().unwrap_or_else(|| json!({}));
    fields["api_key_env"] = json!(env_name(kind));
    fields["base_url"] = json!(base_url);
    let config = provider_config(kind, &fields);
    let name = format!("oracle_{kind}");
    let bible = BibleConfig {
        providers: [(name.clone(), config)].into_iter().collect(),
        ..AppConfig::default().bible
    };
    let mut registry = RegistryBuilder::new(keys).build(&bible);
    registry.providers.shift_remove(&name).unwrap()
}
