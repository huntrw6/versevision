//! One plain-HTTP conversation with ProPresenter's network API on this computer: a GET that returns JSON and a PUT that sends JSON.
//! No TLS, no redirects, no retry. Wording of the failures is `LocalTransport.api_get` / `api_put` of `node/transport.py`.

use std::io::Read;
use std::time::Duration;

use serde_json::Value;

use crate::errors::{Kind, VvError, VvResult};

/// Why a request did not give an answer (before it is worded for people).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpFailure {
    /// The server answered with this status.
    Status(u16),
    /// The connection or the exchange failed (refused, timed out, closed, no valid HTTP).
    Transport(String),
    /// A 200 whose body is not JSON.
    Json(String),
}

#[derive(Debug, Clone)]
pub struct HttpApi {
    pub host: String,
    pub port: u16,
    agent: ureq::Agent,
    put_agent: ureq::Agent,
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(timeout))
        .timeout_send_request(Some(timeout))
        .timeout_recv_response(Some(timeout))
        .timeout_recv_body(Some(timeout))
        .http_status_as_error(false)
        .max_redirects(0)
        .build()
        .into()
}

/// What Python's `OSError` says for a failed connection (`[Errno 111] Connection refused`; `[WinError 10061] ...` on Windows),
/// instead of the Rust wording (`io: Connection refused (os error 111)`).
pub fn transport_text(error: &ureq::Error) -> String {
    if let ureq::Error::Io(io) = error {
        if let Some(code) = io.raw_os_error() {
            let text = io.to_string();
            let strerror = text
                .rsplit_once(" (os error")
                .map_or(text.as_str(), |(head, _)| head);
            return if cfg!(windows) {
                // CPython trims trailing dots and line ends from the system message
                format!(
                    "[WinError {code}] {}",
                    strerror.trim_end_matches(['.', '\r', '\n', ' '])
                )
            } else {
                format!("[Errno {code}] {strerror}")
            };
        }
    }
    error.to_string()
}

/// httpx's `reason_phrase` for the statuses a server is likely to send (the phrase is part of the error text Python shows).
pub fn reason_phrase(code: u16) -> &'static str {
    match code {
        100 => "Continue",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "",
    }
}

/// `str(httpx.HTTPStatusError)`.
pub fn status_error_text(code: u16, url: &str) -> String {
    let kind = match code / 100 {
        1 => "Informational response",
        3 => "Redirect response",
        4 => "Client error",
        _ => "Server error",
    };
    format!(
        "{kind} '{code} {}' for url '{url}'\nFor more information check: https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/{code}",
        reason_phrase(code)
    )
}

impl HttpApi {
    pub fn new(host: &str, port: u16, timeout_seconds: f64) -> HttpApi {
        let timeout = Duration::from_secs_f64(timeout_seconds.clamp(0.001, 3600.0));
        HttpApi {
            host: host.to_string(),
            port,
            agent: agent(timeout),
            put_agent: agent(timeout + Duration::from_secs(5)),
        }
    }

    pub fn base_url(&self) -> String {
        format!("http://{}:{}", self.host, self.port)
    }

    pub fn url(&self, route: &str) -> String {
        format!("{}{route}", self.base_url())
    }

    fn read_body(mut response: ureq::http::Response<ureq::Body>) -> Result<(u16, Vec<u8>), String> {
        let status = response.status().as_u16();
        let mut bytes = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(512 * 1024 * 1024)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        Ok((status, bytes))
    }

    /// `GET route`, the body read as JSON.
    pub fn get(&self, route: &str) -> Result<Value, HttpFailure> {
        let response = self
            .agent
            .get(self.url(route))
            .call()
            .map_err(|e| HttpFailure::Transport(transport_text(&e)))?;
        let (status, bytes) = Self::read_body(response).map_err(HttpFailure::Transport)?;
        if status >= 300 {
            return Err(HttpFailure::Status(status));
        }
        serde_json::from_slice(&bytes).map_err(|e| HttpFailure::Json(e.to_string()))
    }

    /// `PUT route` with a JSON body; the answer's body is not used.
    pub fn put(&self, route: &str, body: &Value) -> Result<(), HttpFailure> {
        let text = serde_json::to_string(body).map_err(|e| HttpFailure::Json(e.to_string()))?;
        let response = self
            .put_agent
            .put(self.url(route))
            .header("Content-Type", "application/json")
            .send(text.as_bytes())
            .map_err(|e| HttpFailure::Transport(transport_text(&e)))?;
        let (status, _) = Self::read_body(response).map_err(HttpFailure::Transport)?;
        if status >= 300 {
            return Err(HttpFailure::Status(status));
        }
        Ok(())
    }

    /// `LocalTransport.api_get`: a 404 is `ProPresenterNotFound` (ProPresenter is there, it just has nothing at that route).
    pub fn api_get(&self, route: &str) -> VvResult<Value> {
        match self.get(route) {
            Ok(value) => Ok(value),
            Err(HttpFailure::Status(404)) => Err(VvError::not_found(format!(
                "ProPresenter is running and answered, but has nothing at {route} (404). For a presentation this usually means it has not \
                 loaded that file yet: open it once in ProPresenter, then try again"
            ))),
            Err(HttpFailure::Status(code)) => Err(VvError::connection(format!(
                "ProPresenter answered {code} for {route}"
            ))),
            Err(HttpFailure::Transport(why)) => Err(VvError::connection(format!(
                "Could not reach ProPresenter at {}:{}: {why}",
                self.host, self.port
            ))),
            // `response.json()` raises a bare ValueError in Python: not a ProPresenter error
            Err(HttpFailure::Json(why)) => Err(VvError::new(Kind::Other("JSONDecodeError"), why)),
        }
    }

    /// `LocalTransport.api_put`.
    pub fn api_put(&self, route: &str, body: &Value) -> VvResult<()> {
        self.put(route, body).map_err(|failure| {
            let why = match failure {
                HttpFailure::Status(code) => status_error_text(code, &self.url(route)),
                HttpFailure::Transport(why) | HttpFailure::Json(why) => why,
            };
            VvError::connection(format!(
                "ProPresenter did not accept the change to {route}: {why}"
            ))
        })
    }

    /// `ProPresenterApiClient._get`: the wording of the async client (used by the older `import` command and the dry run).
    pub fn client_get(&self, route: &str) -> VvResult<Value> {
        match self.get(route) {
            Ok(value) => Ok(value),
            Err(HttpFailure::Status(code)) => Err(VvError::connection(format!(
                "ProPresenter API returned HTTP {code} for {route}"
            ))),
            Err(HttpFailure::Transport(why)) => {
                let lower = why.to_lowercase();
                if lower.contains("timed out") || lower.contains("timeout") {
                    Err(VvError::connection(format!(
                        "ProPresenter API timed out at {}",
                        self.base_url()
                    )))
                } else {
                    Err(VvError::connection(format!(
                        "Could not connect to ProPresenter at {}. Confirm that ProPresenter is running and its Network API is enabled.",
                        self.base_url()
                    )))
                }
            }
            Err(HttpFailure::Json(why)) => Err(VvError::connection(format!(
                "Malformed ProPresenter response for {route}: {why}"
            ))),
        }
    }

    /// `ProPresenterApiClient._put`.
    pub fn client_put(&self, route: &str, body: &Value) -> VvResult<()> {
        self.put(route, body).map_err(|failure| match failure {
            HttpFailure::Status(code) => {
                VvError::connection(format!("ProPresenter API returned HTTP {code} for {route}"))
            }
            HttpFailure::Transport(why) => {
                let lower = why.to_lowercase();
                if lower.contains("timed out") || lower.contains("timeout") {
                    VvError::connection(format!(
                        "ProPresenter API timed out at {}",
                        self.base_url()
                    ))
                } else {
                    VvError::connection(format!(
                        "Could not connect to ProPresenter at {}",
                        self.base_url()
                    ))
                }
            }
            HttpFailure::Json(why) => VvError::connection(format!(
                "Malformed ProPresenter response for {route}: {why}"
            )),
        })
    }
}
