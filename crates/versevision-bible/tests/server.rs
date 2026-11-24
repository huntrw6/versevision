//! The retry/timeout/error paths against a local server, and use from a multi-threaded tokio runtime.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use serde_json::{json, Value};
use versevision_bible::{AnyProvider, RegistryBuilder};
use versevision_core::bibles::{BiblePassages, BibleProvider};
use versevision_core::notes::TraceLog;

fn listing() -> Value {
    json!({"data": [{"id": "b1", "abbreviation": "NIV"}]})
}

async fn provider_for(kind: &str, routes: Value, config: Value) -> (AnyProvider, Arc<Script>) {
    let script = Script::new(&routes);
    let addr = serve(script.clone()).await;
    let base = format!("http://{addr}{}", if kind == "esv" { "/v3" } else { "/v1" });
    let provider = build_provider(
        kind,
        &json!({ "config": config }),
        &base,
        keys_with(&[(env_name(kind), SYNTHETIC_KEY)]),
    );
    (provider, script)
}

fn requests(script: &Script) -> usize {
    script.log.lock().unwrap().len()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_request_is_not_retried() {
    for (kind, path) in [("api_bible", "/v1/bibles"), ("youversion", "/v1/bibles")] {
        for status in [429u16, 500, 502, 503] {
            let (provider, script) = provider_for(
                kind,
                json!({path: [{"status": status}, {"json": listing()}]}),
                json!({}),
            )
            .await;
            let first = provider.list_versions().await.unwrap_err();
            assert_eq!(
                requests(&script),
                1,
                "{kind} {status}: one request, no retry"
            );
            let again = provider.list_versions().await;
            assert!(
                again.is_ok(),
                "{kind}: the next call asks again and succeeds ({first})"
            );
            assert_eq!(requests(&script), 2);
        }
    }
    let (provider, script) = provider_for(
        "esv",
        json!({"/v3/passage/text/": [{"status": 503}, {"json": {"passages": ["ok"]}}]}),
        json!({}),
    )
    .await;
    let spans = [span_of(&json!([3, 16]))];
    assert!(provider.fetch_passage("ESV", "JHN", &spans).await.is_err());
    assert_eq!(requests(&script), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn redirects_are_not_followed() {
    let (provider, script) = provider_for(
        "api_bible",
        json!({"/v1/bibles": [{"status": 302, "headers": {"location": "/v1/other"}}], "/v1/other": [{"json": listing()}]}),
        json!({}),
    )
    .await;
    let error = provider.list_versions().await.unwrap_err();
    assert_eq!(
        error.message(),
        "API.Bible request failed: HTTP 302; no presentation files changed"
    );
    assert_eq!(requests(&script), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_slow_server_times_out_after_the_configured_time_not_before() {
    let mut script = Script::new(&json!({"/v1/bibles": [{"timeout": true}]}));
    Arc::get_mut(&mut script).unwrap().timeout_for = Duration::from_secs(5);
    let addr = serve(script).await;
    let provider = build_provider(
        "api_bible",
        &json!({"config": {"timeout_seconds": 0.5}}),
        &format!("http://{addr}/v1"),
        keys_with(&[(env_name("api_bible"), SYNTHETIC_KEY)]),
    );
    let started = Instant::now();
    let error = provider.list_versions().await.unwrap_err();
    let took = started.elapsed();
    assert_eq!(
        error.message(),
        "API.Bible request timed out; no presentation files changed"
    );
    assert!(took >= Duration::from_millis(450), "{took:?}");
    assert!(took < Duration::from_secs(4), "{took:?}");
    assert_eq!(error.python_exception(), "BibleProviderError");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_stops_in_the_middle_of_the_body_is_a_timeout_or_an_error_text() {
    use tokio::io::AsyncWriteExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buffer = [0u8; 2048];
                let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut buffer).await;
                let _ = socket
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 1000\r\n\r\n{\"data\": [")
                    .await;
                tokio::time::sleep(Duration::from_secs(5)).await;
            });
        }
    });
    let provider = build_provider(
        "youversion",
        &json!({"config": {"timeout_seconds": 0.5}}),
        &format!("http://{addr}/v1"),
        keys_with(&[(env_name("youversion"), SYNTHETIC_KEY)]),
    );
    let started = Instant::now();
    let error = provider.list_versions().await.unwrap_err();
    assert_eq!(
        error.message(),
        "YouVersion request timed out; no presentation files changed"
    );
    assert!(started.elapsed() < Duration::from_secs(4));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_key_goes_only_in_its_header() {
    let (provider, script) = provider_for(
        "api_bible",
        json!({"/v1/bibles": [{"json": listing()}]}),
        json!({}),
    )
    .await;
    provider.list_versions().await.unwrap();
    let (yv, yv_script) = provider_for(
        "youversion",
        json!({"/v1/bibles": [{"json": {"data": []}}]}),
        json!({}),
    )
    .await;
    yv.list_versions().await.unwrap();
    let (esv, esv_script) = provider_for(
        "esv",
        json!({"/v3/passage/text/": [{"json": {"passages": ["x"]}}]}),
        json!({}),
    )
    .await;
    esv.fetch_passage("ESV", "JHN", &[span_of(&json!([3, 16]))])
        .await
        .unwrap();
    for (script, header, value) in [
        (script, "api-key", SYNTHETIC_KEY.to_string()),
        (yv_script, "x-yvp-app-key", SYNTHETIC_KEY.to_string()),
        (
            esv_script,
            "authorization",
            format!("Token {SYNTHETIC_KEY}"),
        ),
    ] {
        let log = script.log.lock().unwrap();
        assert_eq!(log[0]["headers"][header], json!(value));
        // not in the path or the query
        assert!(!log[0]["path"].to_string().contains(SYNTHETIC_KEY));
        assert!(!log[0]["query"].to_string().contains(SYNTHETIC_KEY));
    }
}

/// The registry is `Send + Sync` and its futures are `Send`: many tasks plan against one registry on a multi-threaded runtime.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_registry_serves_many_tasks_at_once() {
    let script = Script::new(&json!({
        "/v1/bibles": [{"json": listing()}],
        "/v1/bibles/b1/verses/JHN.3.16": [{"json": {"data": {"content": "[16] Synthetic."}}}],
        "/v1/bibles/b1/verses/JHN.3.17": [{"json": {"data": {"content": "[17] Synthetic two."}}}],
    }));
    let addr = serve(script.clone()).await;
    let bible = bible_config(
        &json!({"version_routes": {"NIV": "api"}, "providers": {"api": {"type": "api_bible", "base_url": format!("{ORIGIN}/v1"), "api_key_env": env_name("api_bible")}}}),
        &format!("http://{addr}"),
    );
    let directory = tempfile::tempdir().unwrap();
    let registry = Arc::new(
        RegistryBuilder::new(keys_with(&[(env_name("api_bible"), SYNTHETIC_KEY)]))
            .cache(Arc::new(versevision_bible::ScriptureCache::new(
                directory.path(),
            )))
            .build(&bible),
    );
    let mut tasks = Vec::new();
    for n in 0..24 {
        let registry = registry.clone();
        tasks.push(tokio::spawn(async move {
            let text = if n % 2 == 0 {
                "John 3:16 NIV"
            } else {
                "John 3:16-17 NIV"
            };
            let parsed = versevision_core::references::parse_references(text, None).unwrap();
            let reference = serde_json::from_value(parsed["references"][0].clone()).unwrap();
            let mut trace = TraceLog::default();
            let passage = registry.fetch(&reference, &mut trace).await.unwrap();
            passage
                .verses
                .iter()
                .map(|v| v.text.clone())
                .collect::<Vec<_>>()
        }));
    }
    for (n, task) in tasks.into_iter().enumerate() {
        let texts = task.await.unwrap();
        assert_eq!(texts.len(), if n % 2 == 0 { 1 } else { 2 });
        assert_eq!(texts[0], "Synthetic.");
    }
    // after the first answers, the cache serves: far fewer than 24 x 3 requests
    let count = requests(&script);
    println!("24 concurrent plans made {count} requests");
    assert!(count < 24 * 3);
    assert_eq!(registry.description(), "providers: api");
}

/// Python raises `AttributeError` (not `BibleProviderError`) for a response that is not the shape the provider reads; the Rust
/// providers say so in the usual words, and never panic.
#[tokio::test(flavor = "multi_thread")]
async fn a_response_of_the_wrong_shape_is_a_provider_error_not_a_panic() {
    let wrong = [
        json!([]),
        json!("text"),
        json!(5),
        json!(null),
        json!({"data": 5}),
        json!({"data": [1]}),
        json!({"data": [{"id": ["x"], "abbreviation": "A"}]}),
        json!({"data": [{"id": "x", "abbreviation": "A", "language": "eng"}]}),
        json!({"data": [{"id": "x", "abbreviation": "A", "copyright": 5}]}),
    ];
    for kind in ["api_bible", "youversion"] {
        for body in &wrong {
            let (provider, _) =
                provider_for(kind, json!({"/v1/bibles": [{"json": body}]}), json!({})).await;
            let label = if kind == "api_bible" {
                "API.Bible"
            } else {
                "YouVersion"
            };
            if kind == "youversion" && body.to_string().contains("\"language\"") {
                continue; // YouVersion does not read `language` (it reads `language_tag`)
            }
            let error = provider.list_versions().await.unwrap_err();
            assert_eq!(
                error.message(),
                format!("{label} response failed: unexpected response shape; no presentation files changed"),
                "{kind} {body}"
            );
        }
    }
    let (provider, _) = provider_for(
        "esv",
        json!({"/v3/passage/text/": [{"json": []}, {"json": {"passages": "text"}}, {"json": {"passages": [1]}}]}),
        json!({}),
    )
    .await;
    for _ in 0..3 {
        let error = provider
            .fetch_passage("ESV", "JHN", &[span_of(&json!([3, 16]))])
            .await
            .unwrap_err();
        assert_eq!(
            error.message(),
            "ESV response failed: unexpected response shape; no presentation files changed"
        );
    }
    let (provider, _) = provider_for(
        "api_bible",
        json!({"/v1/bibles": [{"json": listing()}], "/v1/bibles/b1/verses/JHN.3.16": [{"json": []}, {"json": {"data": 5}}, {"json": {"data": {"content": 5}}}]}),
        json!({}),
    )
    .await;
    for _ in 0..3 {
        let error = provider
            .fetch_passage("NIV", "JHN", &[span_of(&json!([3, 16]))])
            .await
            .unwrap_err();
        assert_eq!(
            error.message(),
            "API.Bible response failed: unexpected response shape; no presentation files changed"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_base_url_that_is_not_http_is_an_error_text_not_a_panic() {
    for (base, expected) in [
        (
            "not a url",
            "Request URL is missing an 'http://' or 'https://' protocol.",
        ),
        (
            "ftp://example.test/v1",
            "Request URL has an unsupported protocol 'ftp://'.",
        ),
    ] {
        let provider = build_provider(
            "api_bible",
            &json!({}),
            base,
            keys_with(&[(env_name("api_bible"), SYNTHETIC_KEY)]),
        );
        let error = provider.list_versions().await.unwrap_err();
        assert_eq!(
            error.message(),
            format!("API.Bible response failed: {expected}; no presentation files changed")
        );
    }
}
