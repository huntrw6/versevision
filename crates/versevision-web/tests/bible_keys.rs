//! The "Bible keys" section: write-only keys, the Test button, `bible.source`, the StagePilot service line, and the canary that proves a key
//! sent to the API is never in a response, a header, the config, the history or a run folder.

mod common;
use common::*;

use std::sync::{Arc, Mutex};

use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::Router;
use serde_json::{json, Value};
use versevision_bible::{FileKeyStore, InstallationAuth, KeyStore, Secret};
use versevision_core::config::AppConfig;

const GOOD: &str = "good-key-CANARY-7f3a91";
const VERSE: &str = "SECRET-VERSE-TEXT-in-the-passage";

/// A fake of the three providers: it answers when the key in the header is `GOOD`, 401 otherwise (the 401 body repeats the key it was given,
/// to show that nothing of the answer reaches the page), 500 for the key `down`.
async fn provider(request: Request) -> axum::response::Response {
    let headers = request.headers();
    let key = headers
        .get("api-key")
        .or_else(|| headers.get("x-yvp-app-key"))
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .or_else(|| {
            headers
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Token "))
                .map(str::to_string)
        })
        .unwrap_or_default();
    let path = request.uri().path().to_string();
    if key == "down" {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    if key != GOOD {
        return (
            StatusCode::UNAUTHORIZED,
            format!("{{\"message\": \"bad key {key}\"}}"),
        )
            .into_response();
    }
    let body = if path.ends_with("/passage/text/") {
        json!({"passages": [format!("[16] {VERSE}")]})
    } else {
        json!({"data": [{"id": "b1", "abbreviation": "WEB", "name": "World English Bible"}]})
    };
    axum::Json(body).into_response()
}

async fn start_provider() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = Router::new().fallback(provider);
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    port
}

fn with_providers(port: u16) -> impl FnOnce(&mut AppConfig) {
    move |config: &mut AppConfig| {
        let base = format!("http://127.0.0.1:{port}");
        let input = json!({"bible": {"providers": {
            "api": {"type": "api_bible", "base_url": format!("{base}/v1"), "timeout_seconds": 3},
            "yv": {"type": "youversion", "base_url": format!("{base}/yv"), "timeout_seconds": 3},
            "esv": {"type": "esv", "base_url": format!("{base}/v3"), "timeout_seconds": 3},
        }}});
        let parsed = AppConfig::from_value(&input).unwrap();
        config.bible.providers = parsed.bible.providers;
    }
}

fn key_row(section: &Value, id: &str) -> Value {
    section["keys"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["id"] == id)
        .unwrap()
        .clone()
}

#[tokio::test]
async fn keys_show_only_set_or_not_set_and_are_never_read_back() {
    let h = Harness::new("new");
    let router = h.router();
    let section = get(&router, "/api/bible/keys").await.json();
    let ids: Vec<&str> = section["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["api_bible", "youversion", "esv"]);
    assert!(section["keys"]
        .as_array()
        .unwrap()
        .iter()
        .all(|k| k["set"] == false));
    assert_eq!(key_row(&section, "esv")["env"], "ESV_API_KEY");
    // save one
    let saved = post_json(
        &router,
        "/api/bible/keys",
        &json!({"id": "esv", "key": format!("  {GOOD}\n")}),
    )
    .await;
    assert_eq!(saved.status, 200, "{}", saved.text());
    assert!(!saved.text().contains(GOOD));
    assert_eq!(key_row(&saved.json(), "esv")["set"], true);
    assert_eq!(key_row(&saved.json(), "api_bible")["set"], false);
    assert_eq!(
        h.keys.resolve("ESV_API_KEY").unwrap().expose(),
        GOOD,
        "stored trimmed, under the provider's own name"
    );
    // a field of a row is one of these and no more
    let row = key_row(&get(&router, "/api/bible/keys").await.json(), "esv");
    let mut fields: Vec<&str> = row
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    fields.sort();
    assert_eq!(fields, ["env", "help", "id", "label", "set"]);
    // remove it
    let removed = post_json(&router, "/api/bible/keys/remove", &json!({"id": "esv"})).await;
    assert_eq!(key_row(&removed.json(), "esv")["set"], false);
    assert!(h.keys.resolve("ESV_API_KEY").is_none());
    // refusals
    let unknown = post_json(
        &router,
        "/api/bible/keys",
        &json!({"id": "nope", "key": "x"}),
    )
    .await;
    assert_eq!(
        (unknown.status, unknown.json()["error"].as_str().unwrap()),
        (400, "Unknown Bible provider")
    );
    let empty = post_json(
        &router,
        "/api/bible/keys",
        &json!({"id": "esv", "key": "   "}),
    )
    .await;
    assert_eq!(empty.status, 400);
    let quoted = post_json(
        &router,
        "/api/bible/keys",
        &json!({"id": "esv", "key": "\"abc\""}),
    )
    .await;
    assert_eq!(quoted.status, 400);
    assert!(!quoted.text().contains("abc"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_test_button_says_ok_rejected_or_unreachable_and_never_the_key_or_the_verse() {
    let port = start_provider().await;
    let h = Harness::with_config("new", with_providers(port));
    let router = h.router();
    for id in ["api_bible", "youversion", "esv"] {
        // a good key typed in the field, nothing stored
        let ok = post_json(
            &router,
            "/api/bible/keys/test",
            &json!({"id": id, "key": GOOD}),
        )
        .await;
        assert_eq!(ok.status, 200, "{id}: {}", ok.text());
        assert_eq!(
            (
                ok.json()["ok"].clone(),
                ok.json()["status"].as_str().unwrap().to_string()
            ),
            (json!(true), "ok".to_string()),
            "{id}: {}",
            ok.text()
        );
        assert!(
            !ok.text().contains(GOOD) && !ok.text().contains(VERSE),
            "{id}"
        );
        // a wrong key
        let rejected = post_json(
            &router,
            "/api/bible/keys/test",
            &json!({"id": id, "key": "wrong-key-CANARY-2"}),
        )
        .await;
        assert_eq!(
            rejected.json()["status"],
            "rejected",
            "{id}: {}",
            rejected.text()
        );
        assert_eq!(rejected.json()["ok"], false);
        assert!(
            !rejected.text().contains("wrong-key-CANARY-2"),
            "the 401 body repeated the key; the answer must not"
        );
        // the provider is down
        let down = post_json(
            &router,
            "/api/bible/keys/test",
            &json!({"id": id, "key": "down"}),
        )
        .await;
        assert_eq!(
            down.json()["status"],
            "unreachable",
            "{id}: {}",
            down.text()
        );
        // no key typed and none saved
        let none = post_json(&router, "/api/bible/keys/test", &json!({"id": id})).await;
        assert_eq!(none.json()["status"], "not_set", "{id}");
        // the test of a typed key stored nothing
        assert!(!h.keys.has_key(match id {
            "api_bible" => "API_BIBLE_API_KEY",
            "youversion" => "YOUVERSION_API_KEY",
            _ => "ESV_API_KEY",
        }));
    }
    // the stored key is what a test with an empty field uses
    post_json(
        &router,
        "/api/bible/keys",
        &json!({"id": "esv", "key": GOOD}),
    )
    .await;
    let stored = post_json(
        &router,
        "/api/bible/keys/test",
        &json!({"id": "esv", "key": ""}),
    )
    .await;
    assert_eq!(stored.json()["status"], "ok", "{}", stored.text());
    assert!(!stored.text().contains(GOOD) && !stored.text().contains(VERSE));
    // a key with a quote mark at its end is refused before any request
    let shaped = post_json(
        &router,
        "/api/bible/keys/test",
        &json!({"id": "esv", "key": "abc\""}),
    )
    .await;
    assert_eq!(shaped.json()["status"], "rejected");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_provider_that_is_not_there_is_unreachable() {
    let port = {
        // a port nobody listens on
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let h = Harness::with_config("new", with_providers(port));
    let router = h.router();
    let answer = post_json(
        &router,
        "/api/bible/keys/test",
        &json!({"id": "api_bible", "key": GOOD}),
    )
    .await;
    assert_eq!(answer.json()["status"], "unreachable", "{}", answer.text());
    assert!(!answer.text().contains(GOOD));
}

#[tokio::test]
async fn bible_source_is_saved_to_the_config_and_shown() {
    let h = Harness::new("new");
    let router = h.router();
    let section = get(&router, "/api/bible/keys").await.json();
    assert_eq!(section["source"], "auto");
    let sources: Vec<&str> = section["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert_eq!(sources, ["auto", "stagepilot", "own_keys"]);
    assert!(section["sources"]
        .as_array()
        .unwrap()
        .iter()
        .all(|s| !s["help"].as_str().unwrap().is_empty()
            && !s["help"].as_str().unwrap().contains('\n')));
    for value in ["own_keys", "stagepilot", "auto"] {
        let answer = post_json(&router, "/api/bible/source", &json!({"source": value})).await;
        assert_eq!(answer.status, 200, "{}", answer.text());
        assert_eq!(answer.json()["source"], value);
        let saved =
            AppConfig::from_toml_str(&std::fs::read_to_string(h.dirs.config_file()).unwrap())
                .unwrap();
        assert_eq!(serde_json::to_value(saved.bible.source).unwrap(), value);
        assert_eq!(
            serde_json::to_value(h.state.config().bible.source).unwrap(),
            value
        );
    }
    let bad = post_json(
        &router,
        "/api/bible/source",
        &json!({"source": "everywhere"}),
    )
    .await;
    assert_eq!(bad.status, 400);
    assert!(bad.json()["error"]
        .as_str()
        .unwrap()
        .starts_with("bible.source: "));
    assert_eq!(
        h.state.config().bible.source,
        versevision_core::config::BibleSource::Auto
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_source_cannot_change_while_an_import_runs() {
    let h = Harness::new("new");
    let router = h.router();
    let answer = upload(&router, "?version=NIV", "n.txt", NOTES.as_bytes()).await;
    let job = finished(&router, answer.json()["job"].as_str().unwrap()).await;
    h.gate.set(false);
    let apply = post_json(
        &router,
        "/api/apply",
        &json!({"job": answer.json()["job"], "accept_flags": true}),
    )
    .await;
    assert_eq!(apply.status, 202, "{}", apply.text());
    let _ = job;
    for _ in 0..100 {
        if h.state.busy() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let refused = post_json(&router, "/api/bible/source", &json!({"source": "own_keys"})).await;
    assert_eq!(
        (refused.status, refused.json()["error"].as_str().unwrap()),
        (
            400,
            "An import is running; change settings when it finishes"
        )
    );
    h.gate.set(true);
    finished(&router, apply.json()["job"].as_str().unwrap()).await;
}

struct Auth {
    id: Option<String>,
    credential: Option<String>,
}

impl InstallationAuth for Auth {
    fn origin(&self) -> String {
        "https://stagepilot.example".into()
    }
    fn installation_id(&self) -> Option<String> {
        self.id.clone()
    }
    fn credential(&self) -> Option<Secret> {
        self.credential.clone().map(Secret::new)
    }
}

#[tokio::test]
async fn the_page_says_whether_the_stagepilot_service_is_available() {
    let none = Harness::new("new");
    let section = get(&none.router(), "/api/bible/keys").await.json();
    assert_eq!(section["service"]["available"], false);
    assert!(!section["service"]["help"].as_str().unwrap().is_empty());
    for (id, credential, expected) in [
        (Some("inst-1"), Some("cred-CANARY"), true),
        (Some("inst-1"), None, false),
        (None, Some("cred-CANARY"), false),
        (Some(""), Some("cred-CANARY"), false),
    ] {
        let auth = Arc::new(Auth {
            id: id.map(str::to_string),
            credential: credential.map(str::to_string),
        });
        let h = Harness::with("new", move |b| b.installation_auth(auth));
        let answer = get(&h.router(), "/api/bible/keys").await;
        assert_eq!(
            answer.json()["service"]["available"],
            expected,
            "{id:?} {credential:?}"
        );
        assert!(
            !answer.text().contains("cred-CANARY") && !answer.text().contains("inst-1"),
            "the installation's id and credential are not the page's business"
        );
    }
}

fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// The canary: a key sent to the API appears nowhere but the keys file.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_key_sent_to_the_api_appears_in_no_response_header_log_or_run_folder() {
    let port = start_provider().await;
    let canary = GOOD;
    // the real key store: an owner-only file in the keys folder
    let keys_dir = tempfile::tempdir().unwrap();
    let store = Arc::new(FileKeyStore::new(keys_dir.path().join("keys.env")));
    let handle = store.clone();
    let h = Harness::build_with_keys("new", with_providers(port), handle as Arc<dyn KeyStore>);
    let router = h.router();
    let seen: Mutex<Vec<(String, Answer)>> = Mutex::new(Vec::new());
    let note = |what: &str, answer: Answer| {
        seen.lock().unwrap().push((what.to_string(), answer));
    };
    note(
        "save",
        post_json(
            &router,
            "/api/bible/keys",
            &json!({"id": "esv", "key": canary}),
        )
        .await,
    );
    note(
        "save api",
        post_json(
            &router,
            "/api/bible/keys",
            &json!({"id": "api_bible", "key": canary}),
        )
        .await,
    );
    note(
        "test typed",
        post_json(
            &router,
            "/api/bible/keys/test",
            &json!({"id": "youversion", "key": canary}),
        )
        .await,
    );
    note(
        "test stored",
        post_json(&router, "/api/bible/keys/test", &json!({"id": "esv"})).await,
    );
    note(
        "test wrong",
        post_json(
            &router,
            "/api/bible/keys/test",
            &json!({"id": "esv", "key": "other-CANARY-0"}),
        )
        .await,
    );
    note("list", get(&router, "/api/bible/keys").await);
    note("config", get(&router, "/api/config").await);
    note("status", get(&router, "/api/status").await);
    note(
        "source",
        post_json(&router, "/api/bible/source", &json!({"source": "own_keys"})).await,
    );
    note(
        "bad json",
        send(
            &router,
            axum::http::Method::POST,
            "/api/bible/keys",
            &[("host", "localhost")],
            format!("{{\"id\": \"esv\", \"key\": \"{canary}\"").into_bytes(),
        )
        .await,
    );
    // a whole import in between: the run folder, the history and the audit log get written
    let planned = upload(&router, "?version=NIV", "n.txt", NOTES.as_bytes()).await;
    let job = finished(&router, planned.json()["job"].as_str().unwrap()).await;
    note(
        "job",
        get(
            &router,
            &format!("/api/jobs/{}", planned.json()["job"].as_str().unwrap()),
        )
        .await,
    );
    let applied = post_json(
        &router,
        "/api/apply",
        &json!({"job": planned.json()["job"], "accept_flags": true}),
    )
    .await;
    finished(&router, applied.json()["job"].as_str().unwrap()).await;
    assert_eq!(job["state"], "done");
    note("history", get(&router, "/api/history").await);
    note("runs", get(&router, "/api/runs").await);
    note(
        "remove",
        post_json(
            &router,
            "/api/bible/keys/remove",
            &json!({"id": "api_bible"}),
        )
        .await,
    );
    let mut checked = 0;
    for (what, answer) in seen.lock().unwrap().iter() {
        assert!(
            !answer.text().contains(canary),
            "{what}: the key is in the body"
        );
        assert!(
            !answer.text().contains("other-CANARY-0"),
            "{what}: the key is in the body"
        );
        for (name, value) in &answer.headers {
            assert!(
                !value.to_str().unwrap_or("").contains(canary),
                "{what}: the key is in header {name}"
            );
        }
        checked += 1;
    }
    assert!(checked >= 14);
    // every file this app wrote (config, history, audit, run folders): no key
    let mut files = Vec::new();
    walk(h.root.path(), &mut files);
    assert!(
        files.iter().any(|f| f.to_string_lossy().contains("runs")),
        "a run folder was written"
    );
    for file in &files {
        let bytes = std::fs::read(file).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            !text.contains(canary) && !text.contains("other-CANARY-0"),
            "{} holds a key",
            file.display()
        );
    }
    // the key is in the keys file only (it was removed for api_bible, kept for esv), and that file is the owner's alone
    let keys_file = std::fs::read_to_string(keys_dir.path().join("keys.env")).unwrap();
    assert!(keys_file.contains(&format!("ESV_API_KEY={canary}")));
    assert!(!keys_file.contains("API_BIBLE_API_KEY"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(keys_dir.path().join("keys.env"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o077,
            0,
            "keys.env is private to its owner (mode {mode:o})"
        );
    }
}

/// The crate prints and logs nothing: no log line can hold a key because there is no log line.
#[test]
fn the_crate_writes_no_log_lines() {
    let mut sources = Vec::new();
    walk(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut sources,
    );
    assert!(sources.len() >= 7);
    for file in sources
        .into_iter()
        .filter(|f| f.extension().is_some_and(|e| e == "rs"))
    {
        let text = std::fs::read_to_string(&file).unwrap();
        for forbidden in [
            "println!",
            "eprintln!",
            "print!(",
            "dbg!(",
            "tracing::",
            "log::",
        ] {
            // as a whole word: `runlog::` is the run folder's module, not the `log` crate
            let found = text.match_indices(forbidden).any(|(at, _)| {
                !text[..at]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_')
            });
            assert!(!found, "{} uses {forbidden}", file.display());
        }
    }
}
