//! A local stand-in for the control plane's `bible/passage` route: it records every request and answers with a closure, so a
//! test can play the contract's success and every error row. No real network.
#![allow(dead_code)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::{to_bytes, Body};
use axum::extract::State;
use axum::http::{Request, Response, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use versevision_bible::{InstallationAuth, Secret};

pub const CREDENTIAL: &str = "CANARY-sp-credential-5c1a-do-not-print";
pub const INSTALLATION: &str = "9f3a1c7e-inst";

#[derive(Debug, Clone)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
    pub authorization: Option<String>,
    pub user_agent: Option<String>,
    pub content_type: Option<String>,
    pub body: String,
}

impl Seen {
    /// `version` and `ref` from the query (GET) or the JSON body (POST).
    pub fn asked(&self) -> (String, String) {
        if self.method == "POST" {
            let body: Value = serde_json::from_str(&self.body).unwrap();
            (
                body["version"].as_str().unwrap().to_string(),
                body["ref"].as_str().unwrap().to_string(),
            )
        } else {
            (self.query["version"].clone(), self.query["ref"].clone())
        }
    }
}

pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub delay: Option<Duration>,
}

impl Reply {
    pub fn json(status: u16, body: Value) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.to_string(),
            delay: None,
        }
    }

    pub fn with(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }
}

type Responder = dyn Fn(&Seen, usize) -> Reply + Send + Sync;

pub struct Service {
    pub log: Mutex<Vec<Seen>>,
    responder: Box<Responder>,
}

impl Service {
    pub fn requests(&self) -> Vec<Seen> {
        self.log.lock().unwrap().clone()
    }

    pub fn count(&self) -> usize {
        self.log.lock().unwrap().len()
    }
}

async fn handle(State(service): State<Arc<Service>>, request: Request<Body>) -> Response<Body> {
    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, 1 << 20).await.unwrap_or_default();
    let header = |name: &str| {
        parts
            .headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let query: HashMap<String, String> =
        reqwest::Url::parse(&format!("http://x/?{}", parts.uri.query().unwrap_or("")))
            .unwrap()
            .query_pairs()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
    let seen = Seen {
        method: parts.method.to_string(),
        path: parts.uri.path().to_string(),
        query,
        authorization: header("authorization"),
        user_agent: header("user-agent"),
        content_type: header("content-type"),
        body: String::from_utf8_lossy(&bytes).to_string(),
    };
    let index = {
        let mut log = service.log.lock().unwrap();
        log.push(seen.clone());
        log.len() - 1
    };
    let reply = (service.responder)(&seen, index);
    if let Some(delay) = reply.delay {
        tokio::time::sleep(delay).await;
    }
    let mut builder = Response::builder().status(StatusCode::from_u16(reply.status).unwrap());
    for (name, value) in &reply.headers {
        builder = builder.header(name, value);
    }
    builder.body(Body::from(reply.body)).unwrap()
}

/// Serve `responder` (request, index of the request) on a free local port; returns the origin and the recorder.
pub async fn serve_service(
    responder: impl Fn(&Seen, usize) -> Reply + Send + Sync + 'static,
) -> (String, Arc<Service>) {
    let service = Arc::new(Service {
        log: Mutex::new(Vec::new()),
        responder: Box::new(responder),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let app = Router::new().fallback(handle).with_state(service.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), service)
}

/// Which provider the contract's table says serves a version.
pub fn provider_of(version: &str) -> &'static str {
    match version.to_uppercase().as_str() {
        "ESV" => "esv",
        "NKJV" | "NLT" | "KJV" | "MSG" | "WEB" => "api_bible",
        _ => "youversion",
    }
}

/// The verse numbers a normalized reference (`JHN 3:16-18 & 20`) names, with the chapter.
pub fn verses_of(reference: &str) -> Vec<(String, u64, u64)> {
    let (book, rest) = reference.split_once(' ').unwrap();
    let (chapter, spans) = rest.split_once(':').unwrap();
    let chapter: u64 = chapter.parse().unwrap();
    let mut out = Vec::new();
    for span in spans.split(" & ") {
        let (a, b) = match span.split_once('-') {
            Some((a, b)) => (a.parse().unwrap(), b.parse().unwrap()),
            None => (span.parse().unwrap(), span.parse().unwrap()),
        };
        for verse in a..=b {
            out.push((book.to_string(), chapter, verse));
        }
    }
    out
}

/// What the real route answers for a good request (synthetic text, never Scripture): the contract's success shape.
pub fn contract_success(seen: &Seen) -> Reply {
    let (version, reference) = seen.asked();
    let label = version.to_uppercase();
    let verses: Vec<Value> = verses_of(&reference)
        .into_iter()
        .map(|(book, chapter, verse)| {
            json!({"book_id": book, "chapter": chapter, "verse": verse,
                   "text": format!("Synthetic {label} text {book} {chapter}:{verse}.")})
        })
        .collect();
    let provider = provider_of(&label);
    Reply::json(
        200,
        json!({
            "version": label,
            "reference": reference,
            "verses": verses,
            "copyright": format!("Synthetic {label} copyright"),
            "attribution": if provider == "esv" { Value::Null } else { json!(format!("Synthetic {label} description")) },
            "provider": provider,
            "cached": false,
        }),
    )
}

pub struct TestAuth {
    pub origin: String,
    pub id: Option<String>,
    pub credential: Option<String>,
}

impl TestAuth {
    pub fn enrolled(origin: &str) -> Arc<dyn InstallationAuth> {
        Arc::new(Self {
            origin: origin.to_string(),
            id: Some(INSTALLATION.to_string()),
            credential: Some(CREDENTIAL.to_string()),
        })
    }
}

impl InstallationAuth for TestAuth {
    fn origin(&self) -> String {
        self.origin.clone()
    }

    fn installation_id(&self) -> Option<String> {
        self.id.clone()
    }

    fn credential(&self) -> Option<Secret> {
        self.credential.as_deref().map(Secret::new)
    }
}
