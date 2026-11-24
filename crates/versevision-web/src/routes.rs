#![allow(clippy::result_large_err)] // the error is the finished answer (a Response); these run once per request
//! The routes: `web/server.py`'s `do_GET` and `do_POST`, plus the Bible keys section's. Paths, JSON shapes, status codes and error texts are
//! Python's. Every URL the page uses is relative, so this router works at `/` and nested under a prefix.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use serde_json::{json, Map, Value};
use versevision_core::plan::ReviewChoices;
use versevision_pp::errors::VvError;
use versevision_pp::pipeline::{list_playlist_presentations, list_presentations};
use versevision_pp::runlog;
use versevision_pp::themes::{picker_rows, read_theme_tree};

use crate::assets;
use crate::jobs::{self, PlanRequest};
use crate::state::WebState;
use crate::{biblekeys, configpage, save_config};

const MAX_UPLOAD: usize = versevision_docs::MAX_UPLOAD_BYTES;
const CSP: &str =
    "default-src 'self'; style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline'";

pub fn build(state: WebState) -> Router {
    let mut router = Router::new().route("/", get(index));
    for asset in assets::ASSETS.iter() {
        router = router.route(&format!("/{}", asset.name), get(static_file));
    }
    router = router.route("/favicon.ico", get(static_file));
    router
        .route("/api/status", get(status))
        .route("/api/playlists", get(playlists))
        .route("/api/config", get(config_get).post(config_post))
        .route("/api/themethumb", get(themethumb))
        .route("/api/themes", get(themes))
        .route("/api/check", get(check))
        .route("/api/jobs/{id}", get(job))
        .route("/api/runs", get(runs))
        .route("/api/run/{id}", get(run))
        .route("/api/presentations", get(presentations))
        .route("/api/autostart", get(autostart_get).post(autostart_post))
        .route("/api/history", get(history))
        .route("/api/plan", post(plan))
        .route("/api/apply", post(apply))
        .route("/api/replan", post(replan))
        .route("/api/undo", post(undo))
        .route("/api/bible/keys", get(keys_get).post(keys_save))
        .route("/api/bible/keys/remove", post(keys_remove))
        .route("/api/bible/keys/test", post(keys_test))
        .route("/api/bible/source", post(source_set))
        .fallback(not_found)
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

// ------------------------------------------------------------------------------------------------------------ responses

fn json_response(status: StatusCode, payload: &Value) -> Response {
    let body = serde_json::to_vec(payload).unwrap_or_else(|_| b"null".to_vec());
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

fn ok(payload: Value) -> Response {
    json_response(StatusCode::OK, &payload)
}

fn bad(message: impl Into<String>) -> Response {
    json_response(StatusCode::BAD_REQUEST, &json!({"error": message.into()}))
}

async fn not_found() -> Response {
    json_response(StatusCode::NOT_FOUND, &json!({"error": "Not found"}))
}

fn bytes_response(content_type: &'static str, bytes: &'static [u8]) -> Response {
    let mut response = Response::new(Body::from(bytes));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
}

/// Run blocking work (ProPresenter's transport, files) off the async threads.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Response> {
    tokio::task::spawn_blocking(work).await.map_err(|e| {
        json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &json!({"error": format!("Unexpected problem: {e}")}),
        )
    })
}

// ------------------------------------------------------------------------------------------------------------ the guard

fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        diff |= usize::from(a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0));
    }
    diff == 0
}

fn query_pairs(query: Option<&str>) -> Vec<(String, String)> {
    let Some(query) = query else {
        return Vec::new();
    };
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(k), decode(v))
        })
        .collect()
}

fn decode(text: &str) -> String {
    let spaced = text.replace('+', " ");
    percent_encoding::percent_decode_str(&spaced)
        .decode_utf8_lossy()
        .into_owned()
}

fn authorized(state: &WebState, headers: &HeaderMap, query: Option<&str>) -> bool {
    let Some(token) = state.inner.token.as_deref() else {
        return true;
    };
    let supplied = headers
        .get("x-versevision-token")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .or_else(|| {
            query_pairs(query)
                .into_iter()
                .find(|(k, _)| k == "token")
                .map(|(_, v)| v)
        })
        .unwrap_or_default();
    constant_time_eq(&supplied, token)
}

/// The Origin of a browser's cross-origin request is not this page's own host: refuse it. `Sec-Fetch-Site` says the same when the
/// browser sends it, and also catches a sibling site on the same host name.
fn same_origin(headers: &HeaderMap) -> bool {
    if let Some(site) = headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()) {
        if site != "same-origin" && site != "none" {
            return false;
        }
    }
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    let origin = origin.to_str().unwrap_or("null");
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let authority = origin
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(origin);
    !host.is_empty() && authority.eq_ignore_ascii_case(host)
}

fn host_name(host: &str) -> String {
    let host = host.trim();
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or("").to_ascii_lowercase();
    }
    host.rsplit_once(':')
        .map(|(name, port)| {
            if port.chars().all(|c| c.is_ascii_digit()) {
                name
            } else {
                host
            }
        })
        .unwrap_or(host)
        .to_ascii_lowercase()
}

/// Host check, token, origin; then the headers Python puts on every answer.
async fn guard(State(state): State<WebState>, request: Request, next: Next) -> Response {
    let headers = request.headers();
    let allowed = state
        .inner
        .allowed_hosts
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if let Some(allowed) = allowed {
        let host = headers
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .map(host_name)
            .unwrap_or_default();
        if !allowed.iter().any(|a| a.eq_ignore_ascii_case(&host)) {
            return finish(json_response(
                StatusCode::FORBIDDEN,
                &json!({"error": "Not allowed"}),
            ));
        }
    }
    let path = request.uri().path().to_string();
    let query = request.uri().query().map(str::to_string);
    let method = request.method().clone();
    let writes = !matches!(method, Method::GET | Method::HEAD);
    if writes {
        if !authorized(&state, headers, query.as_deref()) || !same_origin(headers) {
            return finish(json_response(
                StatusCode::FORBIDDEN,
                &json!({"error": "Not allowed"}),
            ));
        }
    } else if path.starts_with("/api/") && !authorized(&state, headers, query.as_deref()) {
        return finish(json_response(
            StatusCode::UNAUTHORIZED,
            &json!({"error": "Access token required"}),
        ));
    }
    finish(next.run(request).await)
}

fn finish(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    response
}

// ------------------------------------------------------------------------------------------------------------ the page and its files

async fn index() -> Response {
    bytes_response("text/html; charset=utf-8", assets::INDEX)
}

async fn static_file(request: Request) -> Response {
    let name = request.uri().path().trim_start_matches('/').to_string();
    match assets::find(&name) {
        Some(asset) => bytes_response(asset.content_type, asset.bytes),
        None => json_response(StatusCode::NOT_FOUND, &json!({"error": "Not found"})),
    }
}

// ------------------------------------------------------------------------------------------------------------ reading routes

async fn status(State(state): State<WebState>) -> Response {
    let config = state.config();
    let mut profiles: Vec<&String> = config.profiles.keys().collect();
    profiles.sort();
    ok(json!({
        "target": config.presentation_template.as_ref().map(|t| t.presentation_name.clone()),
        "node": config.node.name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| "this computer".into()),
        "default_version": config.bible.default_version,
        "version": state.inner.app_version,
        "template_ready": config.presentation_template.is_some(),
        "library": config.output.library,
        "profiles": profiles,
    }))
}

async fn playlists(State(state): State<WebState>) -> Response {
    let config = state.config();
    let make = state.inner.transport.clone();
    let result = blocking(move || {
        let transport = make(&config);
        list_playlist_presentations(transport.as_ref())
    })
    .await;
    match result {
        Err(response) => response,
        Ok(Ok(lists)) => {
            ok(json!({"playlists": lists.iter().map(|p| p.to_value()).collect::<Vec<_>>()}))
        }
        Ok(Err(error)) => ok(json!({"playlists": [], "error": error.message})),
    }
}

async fn config_get(State(state): State<WebState>) -> Response {
    ok(configpage::describe(
        &state.config(),
        &state.inner.dirs.config_file(),
    ))
}

fn int_arg(text: &str) -> Option<i64> {
    configpage::py_int_text(text).ok()
}

async fn themethumb(
    State(state): State<WebState>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let theme = query.get("theme").cloned().unwrap_or_default();
    let number = query
        .get("slide")
        .filter(|s| !s.is_empty())
        .map(|s| int_arg(s).unwrap_or(1))
        .unwrap_or(1);
    let pp = state.config().propresenter;
    let image = blocking(move || theme_thumbnail(&pp.host, pp.port, &theme, number)).await;
    match image {
        Ok(Some(bytes)) => {
            let mut response = Response::new(Body::from(bytes));
            response
                .headers_mut()
                .insert(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
            response
        }
        Ok(None) => {
            let mut response = Response::new(Body::empty());
            *response.status_mut() = StatusCode::NOT_FOUND;
            response
                .headers_mut()
                .insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
            response
        }
        Err(response) => response,
    }
}

/// `theme_thumbnail`: ProPresenter's own picture of a theme slide. Only themes outside a group have one through its API.
fn theme_thumbnail(host: &str, port: u16, theme: &str, number: i64) -> Option<Vec<u8>> {
    use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
    const SAFE: &AsciiSet = &NON_ALPHANUMERIC
        .remove(b'_')
        .remove(b'.')
        .remove(b'-')
        .remove(b'~');
    let url = format!(
        "http://{host}:{port}/v1/theme/{}/slides/{}/thumbnail?quality=200",
        utf8_percent_encode(theme, SAFE),
        (number - 1).max(0)
    );
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(3)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut response = agent.get(&url).call().ok()?;
    if response.status().as_u16() != 200 {
        return None;
    }
    response.body_mut().read_to_vec().ok()
}

async fn themes(State(state): State<WebState>) -> Response {
    let config = state.config();
    let inner = state.inner.clone();
    let rows = blocking(move || list_looks(&inner, &config)).await;
    let config = state.config();
    match rows {
        Err(response) => response,
        Ok(Ok(rows)) => {
            ok(json!({"themes": rows, "verse": config.verses.theme, "point": config.points.theme}))
        }
        Ok(Err(error)) => ok(json!({"themes": [], "error": error.message})),
    }
}

/// `list_looks`: the theme slides loaded on the Mac that have a text box, newest read cached for a minute.
fn list_looks(
    inner: &crate::state::Inner,
    config: &versevision_core::config::AppConfig,
) -> Result<Vec<Value>, VvError> {
    if let Some((at, rows)) = inner
        .looks
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
    {
        if at.elapsed() < Duration::from_secs(60) {
            return Ok(rows.clone());
        }
    }
    let transport = (inner.transport)(config);
    let folder = tempfile::Builder::new()
        .prefix("versevision-themes-")
        .tempdir()?;
    transport.fetch_themes(folder.path())?;
    let slides = read_theme_tree(folder.path());
    let rows: Vec<Value> = picker_rows(&slides)
        .iter()
        .map(|row| serde_json::to_value(row).unwrap_or(Value::Null))
        .collect();
    *inner
        .looks
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((Instant::now(), rows.clone()));
    Ok(rows)
}

async fn check(State(state): State<WebState>) -> Response {
    let config = state.config();
    let make = state.inner.transport.clone();
    let result = blocking(move || {
        let transport = make(&config);
        let info = transport.api_get("/version")?;
        let running = transport.is_running();
        Ok::<_, VvError>((info, running))
    })
    .await;
    match result {
        Err(response) => response,
        Ok(Ok((info, running))) => ok(json!({
            "ok": true,
            "propresenter": info.get("host_description").cloned().unwrap_or(Value::Null),
            "running": running,
        })),
        Ok(Err(error)) => ok(json!({"ok": false, "error": error.message})),
    }
}

async fn job(State(state): State<WebState>, Path(id): Path<String>) -> Response {
    let found = state
        .inner
        .control
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .jobs
        .get(&id)
        .cloned();
    match found {
        None => json_response(StatusCode::NOT_FOUND, &json!({"error": "Unknown job"})),
        Some(job) => {
            let j = job
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            ok(json!({
                "id": j.id, "kind": j.kind.as_str(), "state": j.state, "message": j.message,
                "result": j.result, "error": j.error, "exit_code": j.exit_code,
            }))
        }
    }
}

async fn runs(State(state): State<WebState>) -> Response {
    let dir = state.inner.dirs.runs_dir();
    match blocking(move || runlog::list_runs(&dir, 30)).await {
        Ok(rows) => ok(Value::Array(rows)),
        Err(response) => response,
    }
}

async fn run(State(state): State<WebState>, Path(id): Path<String>) -> Response {
    let dir = state.inner.dirs.runs_dir();
    let found = blocking(move || {
        let resolved = runlog::resolve(&dir, &id)?;
        let summary = dir.join(&resolved).join("summary.txt");
        let text = std::fs::read_to_string(&summary).ok()?;
        Some((resolved.clone(), text, dir.join(&resolved)))
    })
    .await;
    match found {
        Err(response) => response,
        Ok(None) => json_response(StatusCode::NOT_FOUND, &json!({"error": "No such run"})),
        Ok(Some((id, summary, folder))) => {
            ok(json!({"id": id, "summary": summary, "folder": folder.to_string_lossy()}))
        }
    }
}

async fn presentations(State(state): State<WebState>) -> Response {
    let config = state.config();
    let make = state.inner.transport.clone();
    let default_library = config.output.library.clone();
    let result = blocking(move || {
        let transport = make(&config);
        list_presentations(transport.as_ref())
    })
    .await;
    match result {
        Err(response) => response,
        Ok(Ok(items)) => {
            let mut libraries: std::collections::BTreeSet<String> =
                items.iter().map(|i| i.library.clone()).collect();
            libraries.insert(default_library.clone());
            ok(json!({
                "presentations": items.iter().map(|i| i.to_value()).collect::<Vec<_>>(),
                "libraries": libraries,
                "default_library": default_library,
            }))
        }
        Ok(Err(error)) => ok(json!({
            "presentations": [], "libraries": [default_library], "default_library": default_library, "error": error.message,
        })),
    }
}

async fn autostart_get(State(state): State<WebState>) -> Response {
    let Some(host) = state.inner.autostart.clone() else {
        return ok(json!({
            "supported": false, "enabled": false, "platform": std::env::consts::OS,
            "detail": "Start at login is set up by the app that runs this page",
        }));
    };
    match blocking(move || host.status()).await {
        Err(response) => response,
        Ok(Ok(status)) => ok(status),
        Ok(Err(detail)) => ok(json!({"supported": false, "enabled": false, "detail": detail})),
    }
}

async fn history(State(state): State<WebState>) -> Response {
    let inner = state.inner.clone();
    match blocking(move || inner.history.load()).await {
        Err(response) => response,
        Ok(records) => ok(Value::Array(
            records
                .iter()
                .rev()
                .take(30)
                .map(|r| {
                    let when = match r.created.strip_suffix('Z') {
                        Some(rest) => format!("{rest}+00:00"),
                        None => r.created.clone(),
                    };
                    json!({
                        "id": r.import_id, "when": when, "source": r.source_name, "target": r.target_name,
                        "slides": r.slide_count, "status": r.status, "mode": r.mode,
                    })
                })
                .collect(),
        )),
    }
}

// ------------------------------------------------------------------------------------------------------------ writing routes

/// `_body()`: the request's body, refused over the upload limit.
async fn read_body(request: Request) -> Result<(HeaderMap, Option<String>, Bytes), Response> {
    let (parts, body) = request.into_parts();
    let too_large = || bad("That file is too large (limit 25 MB)");
    let declared = parts
        .headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    if declared > MAX_UPLOAD {
        return Err(too_large());
    }
    let bytes = axum::body::to_bytes(body, MAX_UPLOAD)
        .await
        .map_err(|_| too_large())?;
    Ok((parts.headers, parts.uri.query().map(str::to_string), bytes))
}

fn parse_json(bytes: &Bytes) -> Result<Value, Response> {
    if bytes.is_empty() {
        return Ok(json!({}));
    }
    match serde_json::from_slice::<Value>(bytes) {
        Ok(value @ Value::Object(_)) => Ok(value),
        Ok(_) => Err(bad("Bad request: expected a JSON object")),
        // never echo the body: it can hold a key
        Err(_) => Err(bad("Bad request: the request is not valid JSON")),
    }
}

fn text_field<'a>(body: &'a Value, key: &str) -> &'a str {
    body.get(key).and_then(Value::as_str).unwrap_or("")
}

fn accepted(job_id: &str) -> Response {
    json_response(StatusCode::ACCEPTED, &json!({"job": job_id}))
}

/// `Path(name).name`: the last part of what the browser called the file.
fn file_name(raw: &str) -> String {
    raw.rsplit(['/', '\\']).next().unwrap_or("").to_string()
}

async fn plan(State(state): State<WebState>, request: Request) -> Response {
    let (headers, query, bytes) = match read_body(request).await {
        Ok(parts) => parts,
        Err(response) => return response,
    };
    // `http.server` hands headers over as Latin-1 text
    let raw_name = headers
        .get("x-file-name")
        .map(|v| v.as_bytes().iter().map(|&b| b as char).collect::<String>())
        .unwrap_or_else(|| "notes.txt".to_string());
    let mut name = file_name(&raw_name);
    if name.is_empty() {
        name = "notes.txt".into();
    }
    let suffix = versevision_docs::suffix_of(&name);
    if !versevision_docs::ALLOWED_SUFFIXES.contains(&suffix.as_str()) {
        return bad(versevision_docs::unsupported_upload_message(&suffix));
    }
    let pairs = query_pairs(query.as_deref());
    let first = |key: &str| -> Option<String> {
        pairs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
            .filter(|v| !v.is_empty())
    };
    let request = PlanRequest {
        name,
        data: bytes.to_vec(),
        version: first("version"),
        target: first("target"),
        mode: pairs
            .iter()
            .find(|(k, _)| k == "mode")
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| "new".into()),
        title: first("title"),
        library: first("library"),
        profile: first("profile"),
        verse_look: first("verse_look"),
        point_look: first("point_look"),
        text_override: None,
        replan_of: None,
    };
    let job = jobs::start_plan(&state, request);
    let id = job
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .id
        .clone();
    accepted(&id)
}

async fn apply(State(state): State<WebState>, request: Request) -> Response {
    let (_, _, bytes) = match read_body(request).await {
        Ok(parts) => parts,
        Err(response) => return response,
    };
    let body = match parse_json(&bytes) {
        Ok(body) => body,
        Err(response) => return response,
    };
    let choices = (|| -> Result<ReviewChoices, String> {
        Ok(ReviewChoices {
            skip: jobs::id_set(&body, "skip")?,
            include: jobs::id_set(&body, "include")?,
            reject_fixes: jobs::id_set(&body, "reject_fixes")?,
        })
    })();
    let choices = match choices {
        Ok(choices) => choices,
        Err(message) => return bad(message),
    };
    let accept = body.get("accept_flags").is_some_and(truthy);
    match jobs::start_apply(&state, text_field(&body, "job"), accept, choices) {
        Ok(job) => {
            let id = job
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .id
                .clone();
            accepted(&id)
        }
        Err(message) => bad(message),
    }
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|x| x != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

async fn replan(State(state): State<WebState>, request: Request) -> Response {
    let (_, _, bytes) = match read_body(request).await {
        Ok(parts) => parts,
        Err(response) => return response,
    };
    let body = match parse_json(&bytes) {
        Ok(body) => body,
        Err(response) => return response,
    };
    let edits: Map<String, Value> = match body.get("edits") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(map)) => map.clone(),
        Some(_) => return bad("Bad request: edits must be an object"),
    };
    match jobs::start_replan(&state, text_field(&body, "job"), &edits) {
        Ok(job) => {
            let id = job
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .id
                .clone();
            accepted(&id)
        }
        Err(message) => bad(message),
    }
}

async fn undo(State(state): State<WebState>, request: Request) -> Response {
    let (_, _, bytes) = match read_body(request).await {
        Ok(parts) => parts,
        Err(response) => return response,
    };
    let body = match parse_json(&bytes) {
        Ok(body) => body,
        Err(response) => return response,
    };
    match jobs::start_undo(&state, text_field(&body, "id")) {
        Ok(job) => {
            let id = job
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .id
                .clone();
            accepted(&id)
        }
        Err(message) => bad(message),
    }
}

async fn config_post(State(state): State<WebState>, request: Request) -> Response {
    let (_, _, bytes) = match read_body(request).await {
        Ok(parts) => parts,
        Err(response) => return response,
    };
    let body = match parse_json(&bytes) {
        Ok(body) => body,
        Err(response) => return response,
    };
    let values: Map<String, Value> = match body.get("values") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(map)) => map.clone(),
        Some(_) => return bad("Bad request: values must be an object"),
    };
    let inner = state.inner.clone();
    let described = blocking(move || -> Result<Value, String> {
        let control = inner
            .control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if control.busy {
            return Err("An import is running; change settings when it finishes".into());
        }
        let mut config = inner
            .config
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let new = configpage::apply(&config, &values)?;
        save_config(&inner.dirs, &new)?;
        *config = new;
        Ok(configpage::describe(&config, &inner.dirs.config_file()))
    })
    .await;
    match described {
        Err(response) => response,
        Ok(Ok(page)) => ok(page),
        Ok(Err(message)) => bad(message),
    }
}

async fn autostart_post(State(state): State<WebState>, request: Request) -> Response {
    let (_, _, bytes) = match read_body(request).await {
        Ok(parts) => parts,
        Err(response) => return response,
    };
    let listen = *state
        .inner
        .listen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if listen.is_some_and(|addr| !addr.ip().is_loopback()) {
        return bad(
            "Start-at-login can only be changed from the page running on this computer (127.0.0.1)",
        );
    }
    let body = match parse_json(&bytes) {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(host) = state.inner.autostart.clone() else {
        return bad(
            "Start-at-login is set up by the app that runs this page, and it did not offer it",
        );
    };
    let enabled = body.get("enabled").is_some_and(truthy);
    let port = listen.map(|a| a.port()).unwrap_or(0);
    let result = blocking(move || {
        if enabled {
            host.enable("127.0.0.1", port)
        } else {
            host.disable()
        }
    })
    .await;
    match result {
        Err(response) => response,
        Ok(Ok(status)) => ok(status),
        Ok(Err(message)) => bad(message),
    }
}

// ------------------------------------------------------------------------------------------------------------ the Bible keys

async fn keys_get(State(state): State<WebState>) -> Response {
    ok(biblekeys::describe(&state))
}

/// The body of a keys request holds a key: it is read, used and dropped, never echoed, logged or stored anywhere but the key store.
async fn key_body(request: Request) -> Result<Value, Response> {
    let (_, _, bytes) = read_body(request).await?;
    parse_json(&bytes)
}

async fn keys_save(State(state): State<WebState>, request: Request) -> Response {
    let body = match key_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    match biblekeys::save(&state, text_field(&body, "id"), text_field(&body, "key")) {
        Ok(section) => ok(section),
        Err(message) => bad(message),
    }
}

async fn keys_remove(State(state): State<WebState>, request: Request) -> Response {
    let body = match key_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    match biblekeys::remove(&state, text_field(&body, "id")) {
        Ok(section) => ok(section),
        Err(message) => bad(message),
    }
}

async fn keys_test(State(state): State<WebState>, request: Request) -> Response {
    let body = match key_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let supplied = body.get("key").and_then(Value::as_str);
    match biblekeys::test(&state, text_field(&body, "id"), supplied).await {
        Ok(answer) => ok(answer),
        Err(message) => bad(message),
    }
}

async fn source_set(State(state): State<WebState>, request: Request) -> Response {
    let body = match key_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let inner = Arc::new(state.clone());
    let value = text_field(&body, "source").to_string();
    match blocking(move || biblekeys::set_source(&inner, &value)).await {
        Err(response) => response,
        Ok(Ok(section)) => ok(section),
        Ok(Err(message)) => bad(message),
    }
}
