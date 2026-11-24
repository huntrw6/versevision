//! One HTTP GET the way the Python providers make it (`httpx.AsyncClient(base_url, timeout)` and `_get`): the same error texts,
//! no redirects, no retry. A key travels only in a header marked sensitive; nothing here prints it.

use std::sync::OnceLock;
use std::time::Duration;

use reqwest::header::{HeaderName, HeaderValue};
use reqwest::{Client, Url};
use serde_json::Value;
use versevision_core::bibles::BibleError;

use crate::secret::Secret;

/// How a provider words its failures (`API.Bible`, `YouVersion`, `ESV`) and what a 401/403 means.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Wording {
    pub label: &'static str,
    pub invalid_key: &'static str,
}

#[derive(Debug)]
pub(crate) struct Endpoint {
    wording: Wording,
    /// `base_url.rstrip("/")`
    base_url: String,
    timeout: Duration,
    client: OnceLock<Result<Client, String>>,
}

/// `rustls` needs a crypto provider chosen once per process. StagePilot installs `ring` itself; when nothing has, this does.
pub(crate) fn ensure_crypto_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}

impl Endpoint {
    pub fn new(wording: Wording, base_url: &str, timeout_seconds: f64) -> Self {
        Self {
            wording,
            base_url: base_url.trim_end_matches('/').to_string(),
            timeout: Duration::from_secs_f64(timeout_seconds.clamp(0.001, 3600.0)),
            client: OnceLock::new(),
        }
    }

    fn client(&self) -> Result<&Client, BibleError> {
        self.client
            .get_or_init(|| {
                ensure_crypto_provider();
                // httpx's `timeout=t` is `connect=t, read=t, write=t, pool=t`: each step, not the whole request.
                Client::builder()
                    .connect_timeout(self.timeout)
                    .read_timeout(self.timeout)
                    .redirect(reqwest::redirect::Policy::none())
                    .user_agent(format!(
                        "versevision-bible/{}",
                        versevision_core::VERSEVISION_VERSION
                    ))
                    .build()
                    .map_err(|e| describe(&e))
            })
            .as_ref()
            .map_err(|message| self.failed(message))
    }

    fn timed_out(&self) -> BibleError {
        BibleError::Provider(format!(
            "{} request timed out; no presentation files changed",
            self.wording.label
        ))
    }

    /// `{label} response failed: {message}; no presentation files changed`
    pub fn failed(&self, message: &str) -> BibleError {
        BibleError::Provider(format!(
            "{} response failed: {message}; no presentation files changed",
            self.wording.label
        ))
    }

    fn status_failed(&self, status: u16) -> BibleError {
        let reason = match status {
            401 | 403 => self.wording.invalid_key.to_string(),
            429 => "rate limit exceeded".to_string(),
            other => format!("HTTP {other}"),
        };
        BibleError::Provider(format!(
            "{} request failed: {reason}; no presentation files changed",
            self.wording.label
        ))
    }

    fn url(&self, route: &str, params: &[(&str, &str)]) -> Result<Url, BibleError> {
        let text = format!("{}{route}", self.base_url);
        let mut url = match Url::parse(&text) {
            Ok(url) => url,
            Err(_) => {
                return Err(
                    self.failed("Request URL is missing an 'http://' or 'https://' protocol.")
                )
            }
        };
        if url.scheme() != "http" && url.scheme() != "https" {
            return Err(self.failed(&format!(
                "Request URL has an unsupported protocol '{}://'.",
                url.scheme()
            )));
        }
        if !params.is_empty() {
            url.query_pairs_mut().extend_pairs(params.iter().copied());
        }
        Ok(url)
    }

    /// `response = client.get(route, headers={header: key}, params=params); response.raise_for_status(); response.json()`
    pub async fn get_json(
        &self,
        route: &str,
        params: &[(&str, &str)],
        header: (&'static str, String),
        key: &Secret,
    ) -> Result<Value, BibleError> {
        let url = self.url(route, params)?;
        let client = self.client()?;
        let mut value = HeaderValue::from_str(&header.1)
            .map_err(|_| self.failed("the key has characters that cannot be sent in a header"))?;
        value.set_sensitive(true);
        let request = client
            .get(url)
            .header(HeaderName::from_static(header.0), value);
        let response = request.send().await.map_err(|e| self.transport(&e, key))?;
        let status = response.status();
        if !status.is_success() {
            return Err(self.status_failed(status.as_u16()));
        }
        let body = response
            .bytes()
            .await
            .map_err(|e| self.transport(&e, key))?;
        self.json(&body, key)
    }

    fn transport(&self, error: &reqwest::Error, key: &Secret) -> BibleError {
        if error.is_timeout() {
            return self.timed_out();
        }
        if error.is_connect() {
            return self.failed("All connection attempts failed");
        }
        self.failed(&key.redact(&describe(error)))
    }

    /// `response.json()`: the body is decoded the way `json.loads(bytes)` does (UTF-8 with or without a BOM, UTF-16/32); the error
    /// text is Python's `JSONDecodeError` / `UnicodeDecodeError`.
    fn json(&self, body: &[u8], key: &Secret) -> Result<Value, BibleError> {
        let text = crate::pyjson::decode_body(body).map_err(|m| self.failed(&key.redact(&m)))?;
        match serde_json::from_str::<Value>(&text) {
            Ok(value) => Ok(value),
            Err(error) => {
                let message = crate::pyjson::decode_error(&text).unwrap_or_else(|| {
                    // Python accepts it (`NaN`, a lone surrogate escape) but the value cannot be held: say what failed, not the body
                    format!("unsupported JSON value ({})", error.classify_name())
                });
                Err(self.failed(&key.redact(&message)))
            }
        }
    }
}

trait ClassifyName {
    fn classify_name(&self) -> &'static str;
}

impl ClassifyName for serde_json::Error {
    fn classify_name(&self) -> &'static str {
        match self.classify() {
            serde_json::error::Category::Io => "io",
            serde_json::error::Category::Syntax => "syntax",
            serde_json::error::Category::Data => "data",
            serde_json::error::Category::Eof => "eof",
        }
    }
}

/// The text of a transport error without the URL, with the causes after it.
pub(crate) fn describe(error: &reqwest::Error) -> String {
    use std::error::Error;
    let mut text = String::new();
    let plain = error.without_url_ref();
    text.push_str(&plain);
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

trait WithoutUrl {
    fn without_url_ref(&self) -> String;
}

impl WithoutUrl for reqwest::Error {
    fn without_url_ref(&self) -> String {
        // `Display` of a reqwest error carries the URL; `url()` is what to cut from the text.
        let text = self.to_string();
        match self.url() {
            Some(url) => text.replace(url.as_str(), "").replace("  ", " "),
            None => text,
        }
    }
}
