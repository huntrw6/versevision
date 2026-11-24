//! A key never appears in any error, `Debug` output, trace record, panic message or file other than the keys file.
//!
//! The test uses a canary as the key of every provider and makes every kind of thing go wrong (a server that answers with the
//! canary in its body and headers, bad JSON, every status, a timeout, a refused and a reset connection, a missing key file), then
//! looks for the canary in everything the crate says.

mod common;

use std::fs;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use common::service::*;
use common::*;
use serde_json::json;
use versevision_bible::{
    AnyProvider, FileKeyStore, KeyError, KeyStore, MemoryKeyStore, RegistryBuilder, ScriptureCache,
    Secret, StagePilotProvider,
};
use versevision_core::bibles::{BibleError, BiblePassages, BibleProvider};
use versevision_core::config::AppConfig;
use versevision_core::notes::TraceLog;

const CANARY: &str = "CANARY-9f2b61c7-do-not-print-4d1e";

fn clean(label: &str, text: &str) {
    assert!(
        !text.contains(CANARY),
        "the key is in {label}: {}",
        text.replace(CANARY, "<<CANARY>>")
    );
}

fn formats(label: &str, error: &BibleError) {
    clean(&format!("{label} (Display)"), &error.to_string());
    clean(&format!("{label} (Debug)"), &format!("{error:?}"));
    clean(&format!("{label} (message)"), error.message());
}

/// A server that answers every request with the canary in the status text, headers and body.
fn echo_routes(status: u16) -> serde_json::Value {
    json!({
        "/v1/bibles": [{"status": status, "headers": {"x-echo": CANARY}, "text": format!("{{\"detail\": \"Token {CANARY}\"}}")}],
        "/v3/passage/text/": [{"status": status, "headers": {"x-echo": CANARY}, "text": format!("{{\"detail\": \"Token {CANARY}\"}}")}],
    })
}

async fn every_call(label: &str, provider: &AnyProvider) -> usize {
    let mut errors = 0;
    for call in [
        json!({"op": "versions"}),
        json!({"op": "health"}),
        json!({"op": "fetch", "version": "TST", "book": "JHN", "spans": [[3, 16]]}),
        json!({"op": "fetch", "version": "ESV", "book": "JHN", "spans": [[3, 16, 3, 17]]}),
        json!({"op": "fetch", "version": "ESV", "book": "JHN", "spans": [[3, 16, 4, 1]]}),
    ] {
        let result = match call["op"].as_str().unwrap() {
            "versions" => provider.list_versions().await.map(|_| ()),
            "health" => {
                let health = provider.health_check().await;
                clean(&format!("{label} health"), &format!("{health:?}"));
                Ok(())
            }
            _ => {
                let spans: Vec<_> = call["spans"]
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
                    .map(|_| ())
            }
        };
        if let Err(error) = result {
            errors += 1;
            formats(label, &error);
        }
    }
    errors
}

#[tokio::test(flavor = "multi_thread")]
async fn no_error_shows_the_key() {
    let mut errors = 0;
    for kind in ["api_bible", "youversion", "esv"] {
        let keys = keys_with(&[(env_name(kind), CANARY)]);
        // 1. the server answers badly, with the key in everything it says
        for status in [200u16, 401, 403, 404, 429, 500, 502, 301] {
            let script = Script::new(&echo_routes(status));
            let addr = serve(script).await;
            let base = format!("http://{addr}{}", if kind == "esv" { "/v3" } else { "/v1" });
            let provider = build_provider(kind, &json!({}), &base, keys.clone());
            errors += every_call(&format!("{kind} status {status}"), &provider).await;
            clean("the Debug of a provider", &format!("{provider:?}"));
        }
        // 2. nothing is listening
        let closed = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap()
        };
        let provider = build_provider(
            kind,
            &json!({"timeout_seconds": 1.0}),
            &format!("http://{closed}/v1"),
            keys.clone(),
        );
        errors += every_call(&format!("{kind} refused"), &provider).await;
        // 3. the connection is accepted and dropped
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let reset = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                if let Ok((socket, _)) = listener.accept().await {
                    drop(socket);
                }
            }
        });
        let provider = build_provider(
            kind,
            &json!({"timeout_seconds": 1.0}),
            &format!("http://{reset}/v1"),
            keys.clone(),
        );
        errors += every_call(&format!("{kind} reset"), &provider).await;
        // 4. the server never answers
        let mut script = Script::new(
            &json!({"/v1/bibles": [{"timeout": true}], "/v3/passage/text/": [{"timeout": true}]}),
        );
        Arc::get_mut(&mut script).unwrap().timeout_for = Duration::from_secs(2);
        let addr = serve(script).await;
        let provider = build_provider(
            kind,
            &json!({"timeout_seconds": 0.2}),
            &format!("http://{addr}/v1"),
            keys.clone(),
        );
        errors += every_call(&format!("{kind} timeout"), &provider).await;
        // 5. a key that cannot be sent in a header (the text has the key in it)
        let bad = keys_with(&[]);
        bad.set_key(env_name(kind), Secret::new(format!("{CANARY}\u{7f}x")))
            .unwrap();
        let provider = build_provider(kind, &json!({}), &format!("http://{addr}/v1"), bad);
        errors += every_call(&format!("{kind} bad header key"), &provider).await;
        // 6. no key at all
        let provider = build_provider(
            kind,
            &json!({}),
            &format!("http://{addr}/v1"),
            keys_with(&[]),
        );
        errors += every_call(&format!("{kind} no key"), &provider).await;
    }
    println!("canary: {errors} error texts checked, none holds the key");
    assert!(errors > 100, "the scenarios must produce errors ({errors})");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_registry_trace_and_cache_never_hold_the_key() {
    let directory = tempfile::tempdir().unwrap();
    let keys = keys_with(&[
        (env_name("api_bible"), CANARY),
        (env_name("youversion"), CANARY),
        (env_name("esv"), CANARY),
    ]);
    let script = Script::new(&echo_routes(401));
    let addr = serve(script).await;
    let bible = bible_config(
        &json!({
            "version_routes": {"NIV": "api", "ESV": "esv", "NASB2020": "yv"},
            "providers": {
                "api": {"type": "api_bible", "base_url": format!("{ORIGIN}/v1"), "api_key_env": env_name("api_bible")},
                "yv": {"type": "youversion", "base_url": format!("{ORIGIN}/v1"), "api_key_env": env_name("youversion")},
                "esv": {"type": "esv", "base_url": format!("{ORIGIN}/v3"), "api_key_env": env_name("esv")},
            },
        }),
        &format!("http://{addr}"),
    );
    let cache = Arc::new(ScriptureCache::new(directory.path().join("cache")));
    let registry = RegistryBuilder::new(keys.clone())
        .cache(cache.clone())
        .build(&bible);
    for text in ["John 3:16 NIV", "John 3:16 ESV", "John 3:16 NASB2020"] {
        let parsed = versevision_core::references::parse_references(text, None).unwrap();
        let reference = serde_json::from_value(parsed["references"][0].clone()).unwrap();
        let mut trace = TraceLog::default();
        let result = registry.fetch(&reference, &mut trace).await;
        let error = result.expect_err("401 from every server");
        formats(text, &error);
        clean("the trace", &format!("{:?}", trace.to_json(1)));
    }
    for health in registry.health_checks().await {
        clean("a health check", &format!("{health:?}"));
    }
    clean("the Debug of the registry", &format!("{registry:?}"));
    clean("the Debug of the cache", &format!("{cache:?}"));
    clean("the config", &format!("{bible:?}"));
    clean("the config as TOML", &{
        AppConfig {
            bible: bible.clone(),
            ..AppConfig::default()
        }
        .to_toml_string()
    });
    clean(
        "the Debug of the builder",
        &format!("{:?}", RegistryBuilder::new(keys.clone())),
    );
    clean("the Debug of the key store", &format!("{keys:?}"));
    assert!(
        !directory.path().join("cache").exists()
            || fs::read_dir(directory.path().join("cache"))
                .unwrap()
                .count()
                == 0
    );
}

#[test]
fn key_types_print_masked_in_every_format_and_in_panics() {
    let secret = Secret::new(CANARY);
    clean("Secret Debug", &format!("{secret:?}"));
    clean("Secret {:#?}", &format!("{secret:#?}"));
    assert_eq!(format!("{secret:?}"), "Secret(***)");
    assert_eq!(secret.redact(&format!("a {CANARY} b")), "a *** b");
    let some = Some(secret.clone());
    clean("Option<Secret>", &format!("{some:?}"));
    clean("Vec<Secret>", &format!("{:?}", vec![secret.clone()]));

    let directory = tempfile::tempdir().unwrap();
    let store = FileKeyStore::new(directory.path().join("keys.env"));
    store.set_key("VV_CANARY_KEY", secret.clone()).unwrap();
    clean("FileKeyStore Debug", &format!("{store:?}"));
    let memory = MemoryKeyStore::new();
    memory.set_key("VV_CANARY_KEY", secret.clone()).unwrap();
    clean("MemoryKeyStore Debug", &format!("{memory:?}"));
    // the errors of the key store
    let bad_value = store
        .set_key(
            "VV_CANARY_KEY",
            Secret::new(format!("{CANARY}\nsecond line")),
        )
        .unwrap_err();
    clean(
        "KeyError::InvalidValue",
        &format!("{bad_value} {bad_value:?}"),
    );
    let bad_name = store.set_key(CANARY, Secret::new("x")).unwrap_err();
    // a bad *name* is the caller's own text, not a key; it is the one place a caller-chosen string is echoed
    assert!(matches!(bad_name, KeyError::InvalidName(_)));
    let blocked = FileKeyStore::new(Path::new(directory.path()).join("keys.env").join("inside"));
    let io = blocked
        .set_key("VV_CANARY_KEY", secret.clone())
        .unwrap_err();
    clean("KeyError::Io", &format!("{io} {io:?}"));

    // a panic that prints the value of a result holding a key
    for panic in [
        catch_unwind(AssertUnwindSafe(|| {
            let ok = std::hint::black_box(Ok::<Secret, KeyError>(secret.clone()));
            ok.unwrap_err();
        })),
        catch_unwind(AssertUnwindSafe(|| {
            let err = std::hint::black_box(Err::<(), Secret>(secret.clone()));
            err.unwrap();
        })),
        catch_unwind(AssertUnwindSafe(|| {
            let err = std::hint::black_box(Err::<(), Secret>(secret.clone()));
            err.expect("storing the key");
        })),
        catch_unwind(AssertUnwindSafe(|| {
            assert_eq!(secret, Secret::new("something else"));
        })),
    ] {
        let payload = panic.unwrap_err();
        let text = payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default();
        clean("a panic message", &text);
    }
}

/// The crate writes nothing to stdout, stderr or a log: there is nowhere for a key (or ESV text) to go.
#[test]
fn the_library_never_prints_or_logs() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut checked = 0;
    for entry in fs::read_dir(src).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "rs") && !path.ends_with("entities.rs") {
            let text = fs::read_to_string(&path).unwrap();
            for banned in [
                "println!",
                "eprintln!",
                "print!(",
                "eprint!(",
                "dbg!",
                "tracing::",
                "log::",
                "std::io::stdout",
                "std::io::stderr",
            ] {
                assert!(!text.contains(banned), "{} uses {banned}", path.display());
            }
            checked += 1;
        }
    }
    assert!(checked >= 10);
}

// ------------------------------------------------------------------------------------------------ the StagePilot service

fn provider(origin: &str) -> StagePilotProvider {
    StagePilotProvider::new("stagepilot", Some(TestAuth::enrolled(origin)))
}

fn spans(raw: &[serde_json::Value]) -> Vec<versevision_core::references::VerseSpan> {
    raw.iter().map(span_of).collect()
}

fn clean_credential(label: &str, text: &str) {
    use common::service::CREDENTIAL;
    assert!(
        !text.contains(CREDENTIAL),
        "the credential is in {label}: {}",
        text.replace(CREDENTIAL, "<<CREDENTIAL>>")
    );
}

/// The StagePilot installation credential is in nothing the crate says: errors, health, `Debug`, whatever the service echoes back.
#[tokio::test]
async fn the_stagepilot_credential_is_never_printed() {
    let echo = |status: u16| {
        move |_: &Seen, _: usize| {
            Reply {
            status,
            headers: vec![
                ("x-echo".to_string(), CREDENTIAL.to_string()),
                ("retry-after".to_string(), "99".to_string()),
            ],
            body: json!({"error": "invalid_reference", "message": format!("echo {CREDENTIAL}"), "detail": CREDENTIAL})
                .to_string(),
            delay: None,
        }
        }
    };
    let mut errors = 0;
    for status in [200u16, 400, 401, 403, 404, 429, 500, 502, 503, 504, 301] {
        let (origin, _s) = serve_service(echo(status)).await;
        let p = provider(&origin);
        clean_credential("Debug of the provider", &format!("{p:?}"));
        for call in 0..3 {
            let result = match call {
                0 => p
                    .fetch_passage("NIV", "JHN", &spans(&[json!([3, 16])]))
                    .await
                    .map(|_| ()),
                1 => p.fetch_reference("NIV", "JHN 3:16").await.map(|_| ()),
                _ => {
                    let health = p.health_check().await;
                    clean_credential(&format!("health ({status})"), &format!("{health:?}"));
                    Ok(())
                }
            };
            if let Err(error) = result {
                errors += 1;
                clean_credential(&format!("Display ({status})"), &error.to_string());
                clean_credential(&format!("Debug ({status})"), &format!("{error:?}"));
                clean_credential(&format!("message ({status})"), error.message());
            }
        }
    }
    assert!(errors > 20, "{errors} errors were checked");
    // a body that holds the credential in a verse text is redacted too
    let (origin, _s) = serve_service(|seen, _| {
        let mut reply = contract_success(seen);
        let mut body: serde_json::Value = serde_json::from_str(&reply.body).unwrap();
        body["verses"][0]["text"] = json!(format!("text {CREDENTIAL}"));
        reply.body = body.to_string();
        reply
    })
    .await;
    let verses = provider(&origin)
        .fetch_passage("NIV", "JHN", &spans(&[json!([3, 16])]))
        .await
        .unwrap();
    clean_credential("a verse text", &verses[0].text);
    // a refused and a reset connection, and a credential that cannot be a header
    let dead = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };
    let error = provider(&dead)
        .fetch_passage("NIV", "JHN", &spans(&[json!([3, 16])]))
        .await
        .unwrap_err();
    clean_credential("connection error", error.message());
    let odd = Arc::new(TestAuth {
        origin: dead.clone(),
        id: Some(INSTALLATION.to_string()),
        credential: Some(format!("{CREDENTIAL}\nline")),
    });
    let error = StagePilotProvider::new("stagepilot", Some(odd))
        .fetch_passage("NIV", "JHN", &spans(&[json!([3, 16])]))
        .await
        .unwrap_err();
    clean_credential("header error", error.message());
}
