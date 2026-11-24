//! The StagePilot Bible service: verse text from the control plane's `bible/passage` route (`BIBLE_PROXY_CONTRACT.md`), so an
//! install with no Bible key of its own still shows verses. The operator's keys stay on the server; this client sends only the
//! installation's own credential (`Authorization: Bearer ...`) and never prints it.
//!
//! * [`InstallationAuth`]: what the host (StagePilot) lends: the control plane's origin, the installation id and its credential.
//!   A host that has none (standalone VerseVision) passes no auth; the provider then reports itself unavailable.
//! * [`StagePilotProvider`]: a [`BibleProvider`]. One request asks for at most 40 verses of one chapter (the service refuses more
//!   and refuses a range across chapters), so a passage is split per chapter and per 40 verses and the answers are joined in order.
//! * Text from the service is stored like the underlying provider's text, except ESV (Crossway's terms): the ESV scope
//!   ([`StagePilotProvider::for_esv`]) is never cacheable, and the stored scope refuses to hand out a verse the service says
//!   came from `esv`.
//! * Text cleanup: the service returns API.Bible's text with its paragraph mark (`¶ `) in front of a verse that starts a
//!   paragraph; Python's API.Bible path (`get_text(" ", strip=True)` and the `[16]` marker) never shows it, so a leading pilcrow is
//!   removed ([`clean_text`]). Nothing else is touched.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use reqwest::header::{HeaderName, HeaderValue, CONTENT_TYPE};
use reqwest::{Client, Url};
use serde_json::Value;
use versevision_core::bibles::{BibleError, BibleProvider, BibleVerse, ProviderHealth};
use versevision_core::config::CachePolicy;
use versevision_core::references::{casefold, expand_span, VerseSpan};
use versevision_core::versions::BibleVersion;

use crate::http::{describe, ensure_crypto_provider};
use crate::secret::Secret;

/// What the host lends the Bible service: where the control plane is and who this installation is.
pub trait InstallationAuth: Send + Sync {
    /// The control plane's origin, for example `https://stagepilot.illuminary.studio` (no trailing path).
    fn origin(&self) -> String;
    /// This installation's id, or `None` while it has none (not enrolled yet).
    fn installation_id(&self) -> Option<String>;
    /// This installation's credential, or `None`. Never print it.
    fn credential(&self) -> Option<Secret>;
}

/// Every version the service offers (the contract's list), with the name shown for it and whether the ESV API serves it.
pub const SERVICE_VERSIONS: [(&str, &str); 14] = [
    ("ESV", "English Standard Version"),
    ("NIV", "New International Version"),
    ("AMP", "Amplified Bible"),
    ("BSB", "Berean Standard Bible"),
    ("LSV", "Literal Standard Version"),
    ("NIRV", "New International Reader's Version"),
    ("NASB1995", "New American Standard Bible 1995"),
    ("NASB2020", "New American Standard Bible 2020"),
    ("TPT", "The Passion Translation"),
    ("NKJV", "New King James Version"),
    ("NLT", "New Living Translation"),
    ("KJV", "King James Version"),
    ("MSG", "The Message"),
    ("WEB", "World English Bible"),
];

/// The label of `version` as the service spells it (upper case), when the service offers it (case-insensitive).
pub fn service_label(version: &str) -> Option<&'static str> {
    let wanted = casefold(version);
    SERVICE_VERSIONS
        .iter()
        .find(|(label, _)| casefold(label) == wanted)
        .map(|(label, _)| *label)
}

/// The service takes a request of at most this many verses (`MAX_VERSES_PER_REQUEST`).
pub const MAX_VERSES_PER_REQUEST: usize = 40;
/// A reference longer than this goes in a POST body instead of the URL.
pub const MAX_URL_REFERENCE_CHARS: usize = 1500;
/// A `429` is retried once, and only when its `Retry-After` is at most this many seconds.
pub const MAX_RETRY_AFTER_SECONDS: u64 = 5;

const LABEL: &str = "StagePilot Bible service";
const NOT_STORED: &str = "no presentation files changed";

/// Which versions one [`StagePilotProvider`] serves, and so whether its text may be stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// Every version but ESV; its text is stored like the underlying provider's.
    Stored,
    /// ESV only; never stored.
    Esv,
}

/// What the service answered, parsed exactly as the contract describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServicePassage {
    /// The requested version, upper case.
    pub version: String,
    /// The reference the service understood, normalized, without the version.
    pub reference: String,
    pub verses: Vec<BibleVerse>,
    pub copyright: Option<String>,
    pub attribution: Option<String>,
    /// `api_bible`, `youversion` or `esv`.
    pub provider: String,
    /// Informational only: the copy came from the service's cache.
    pub cached: bool,
}

/// The StagePilot Bible service as a [`BibleProvider`].
pub struct StagePilotProvider {
    name: String,
    scope: Scope,
    auth: Option<Arc<dyn InstallationAuth>>,
    timeout: Duration,
    client: OnceLock<Result<Client, String>>,
}

impl std::fmt::Debug for StagePilotProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Nothing of the auth is printed: not the origin, the id or the credential.
        f.debug_struct("StagePilotProvider")
            .field("name", &self.name)
            .field("scope", &self.scope)
            .field("has_auth", &self.auth.is_some())
            .finish_non_exhaustive()
    }
}

impl StagePilotProvider {
    /// Every version of the service except ESV; its text is stored like the underlying provider's.
    /// `auth` is `None` outside StagePilot: the provider then says it is not available.
    pub fn new(name: &str, auth: Option<Arc<dyn InstallationAuth>>) -> Self {
        Self::with_scope(name, auth, Scope::Stored)
    }

    /// ESV only. Its text is never stored (not on disk, not in memory beyond the call).
    pub fn for_esv(name: &str, auth: Option<Arc<dyn InstallationAuth>>) -> Self {
        Self::with_scope(name, auth, Scope::Esv)
    }

    fn with_scope(name: &str, auth: Option<Arc<dyn InstallationAuth>>, scope: Scope) -> Self {
        Self {
            name: name.to_string(),
            scope,
            auth,
            // Each connect/read step, as the other providers (httpx's `timeout=t`). The service itself waits up to 15 s for each
            // upstream request before it answers `504`, so a client that gave up at 15 s would never see that answer.
            timeout: Duration::from_secs(30),
            client: OnceLock::new(),
        }
    }

    /// Each connect/read step may take this long (default 30 s: the service waits up to 15 s for its provider).
    pub fn timeout(mut self, seconds: f64) -> Self {
        self.timeout = Duration::from_secs_f64(seconds.clamp(0.001, 3600.0));
        self
    }

    /// The installation id and credential, when the host has both.
    fn identity(&self) -> Option<(String, Secret)> {
        let auth = self.auth.as_ref()?;
        let id = auth.installation_id().filter(|id| !id.is_empty())?;
        let credential = auth.credential().filter(|c| !c.is_empty())?;
        Some((id, credential))
    }

    /// Is there an installation to ask on behalf of?
    pub fn is_available(&self) -> bool {
        self.identity().is_some()
    }

    fn serves(&self, version: &str) -> Option<&'static str> {
        let label = service_label(version)?;
        match (self.scope, label == "ESV") {
            (Scope::Esv, true) | (Scope::Stored, false) => Some(label),
            _ => None,
        }
    }

    fn client(&self) -> Result<&Client, BibleError> {
        self.client
            .get_or_init(|| {
                ensure_crypto_provider();
                Client::builder()
                    .connect_timeout(self.timeout)
                    .read_timeout(self.timeout)
                    .redirect(reqwest::redirect::Policy::none())
                    // Cloudflare answers the generic client agent with 403.
                    .user_agent(concat!("VerseVision/", env!("CARGO_PKG_VERSION")))
                    .build()
                    .map_err(|e| describe(&e))
            })
            .as_ref()
            .map_err(|message| failed(message))
    }

    /// One passage, asked for as the service wants it: `reference` is a normalized reference without the version
    /// (`John 3:16-18 & 20`), at most 40 verses of one chapter. A reference longer than [`MAX_URL_REFERENCE_CHARS`] is sent as a
    /// POST body. A `429` with `Retry-After` of at most 5 s is retried once.
    pub async fn fetch_reference(
        &self,
        version: &str,
        reference: &str,
    ) -> Result<ServicePassage, BibleError> {
        let Some((id, credential)) = self.identity() else {
            return Err(unavailable());
        };
        let origin = self.auth.as_ref().map(|a| a.origin()).unwrap_or_default();
        let mut retried = false;
        loop {
            match self
                .send_once(&origin, &id, &credential, version, reference)
                .await
            {
                Ok(value) => return self.parse(&value, &credential),
                Err(Refused {
                    error,
                    retry_after: Some(seconds),
                }) if !retried && seconds <= MAX_RETRY_AFTER_SECONDS => {
                    retried = true;
                    tokio::time::sleep(Duration::from_secs(seconds)).await;
                    drop(error);
                }
                Err(Refused { error, .. }) => return Err(error),
            }
        }
    }

    async fn send_once(
        &self,
        origin: &str,
        id: &str,
        credential: &Secret,
        version: &str,
        reference: &str,
    ) -> Result<Value, Refused> {
        let url = passage_url(origin, id)?;
        let client = self.client()?;
        let mut bearer = HeaderValue::from_str(&format!("Bearer {}", credential.expose()))
            .map_err(|_| failed("the credential has characters that cannot be sent in a header"))?;
        bearer.set_sensitive(true);
        let authorization = HeaderName::from_static("authorization");
        let request = if reference.chars().count() > MAX_URL_REFERENCE_CHARS {
            let body = serde_json::json!({ "version": version, "ref": reference });
            client
                .post(url)
                .header(authorization, bearer)
                .header(CONTENT_TYPE, "application/json")
                .body(body.to_string().into_bytes())
        } else {
            let mut url = url;
            url.query_pairs_mut()
                .append_pair("version", version)
                .append_pair("ref", reference);
            client.get(url).header(authorization, bearer)
        };
        let response = request
            .send()
            .await
            .map_err(|e| transport(&e, credential))?;
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok());
        let body = response
            .bytes()
            .await
            .map_err(|e| transport(&e, credential))?;
        if !(200..300).contains(&status) {
            return Err(Refused {
                error: status_error(status, &body, retry_after, version, credential),
                retry_after: if status == 429 { retry_after } else { None },
            });
        }
        let text = crate::pyjson::decode_body(&body).map_err(|m| failed(&credential.redact(&m)))?;
        serde_json::from_str::<Value>(&text)
            .map_err(|_| failed("the service did not answer with JSON"))
            .map_err(Refused::from)
    }

    /// Parse the success body exactly as the contract has it; anything else is an unexpected shape.
    fn parse(&self, value: &Value, credential: &Secret) -> Result<ServicePassage, BibleError> {
        let shape = || failed("unexpected response shape");
        let object = value.as_object().ok_or_else(shape)?;
        let text_of = |key: &str| object.get(key).and_then(Value::as_str);
        let optional = |key: &str| -> Result<Option<String>, BibleError> {
            match object.get(key) {
                Some(Value::Null) => Ok(None),
                Some(Value::String(s)) if s.is_empty() => Ok(None),
                Some(Value::String(s)) => Ok(Some(credential.redact(s))),
                _ => Err(shape()),
            }
        };
        let version = text_of("version").ok_or_else(shape)?.to_string();
        let reference = text_of("reference").ok_or_else(shape)?.to_string();
        let provider = match text_of("provider") {
            Some(p @ ("api_bible" | "youversion" | "esv")) => p.to_string(),
            _ => return Err(shape()),
        };
        let cached = object
            .get("cached")
            .and_then(Value::as_bool)
            .ok_or_else(shape)?;
        let copyright = optional("copyright")?;
        let attribution = optional("attribution")?;
        let items = object
            .get("verses")
            .and_then(Value::as_array)
            .ok_or_else(shape)?;
        let mut verses = Vec::with_capacity(items.len());
        for item in items {
            let item = item.as_object().ok_or_else(shape)?;
            let book_id = item
                .get("book_id")
                .and_then(Value::as_str)
                .ok_or_else(shape)?;
            let chapter = item
                .get("chapter")
                .and_then(Value::as_u64)
                .ok_or_else(shape)?;
            let verse = item
                .get("verse")
                .and_then(Value::as_u64)
                .ok_or_else(shape)?;
            let text = item.get("text").and_then(Value::as_str).ok_or_else(shape)?;
            verses.push(BibleVerse {
                book_id: book_id.to_string(),
                chapter,
                verse,
                text: clean_text(&credential.redact(text)),
                version: version.to_uppercase(),
                paragraph_start: false,
                poetry_indent: 0,
                copyright: copyright.clone(),
                attribution: attribution.clone(),
            });
        }
        Ok(ServicePassage {
            version,
            reference,
            verses,
            copyright,
            attribution,
            provider,
            cached,
        })
    }
}

/// The text of a verse as Python's providers show it: a leading API.Bible paragraph mark (`¶`, and the space after it) is
/// removed. This is the only rule: a pilcrow anywhere else, or any other character, stays.
pub fn clean_text(text: &str) -> String {
    let mut rest = text.trim_start();
    while let Some(after) = rest.strip_prefix('\u{b6}') {
        rest = after.trim_start();
    }
    rest.trim_end().to_string()
}

/// A failed attempt: its error and, for a `429`, the `Retry-After` it carried.
struct Refused {
    error: BibleError,
    retry_after: Option<u64>,
}

impl From<BibleError> for Refused {
    fn from(error: BibleError) -> Self {
        Self {
            error,
            retry_after: None,
        }
    }
}

fn unavailable() -> BibleError {
    BibleError::Provider(format!(
        "{LABEL} request failed: this copy of VerseVision is not connected to a StagePilot installation (add your own Bible key instead); {NOT_STORED}"
    ))
}

/// `{label} response failed: {message}; no presentation files changed`
fn failed(message: &str) -> BibleError {
    BibleError::Provider(format!("{LABEL} response failed: {message}; {NOT_STORED}"))
}

fn request_failed(reason: &str) -> BibleError {
    BibleError::Provider(format!("{LABEL} request failed: {reason}; {NOT_STORED}"))
}

fn transport(error: &reqwest::Error, credential: &Secret) -> Refused {
    if error.is_timeout() {
        return Refused::from(BibleError::Provider(format!(
            "{LABEL} request timed out; {NOT_STORED}"
        )));
    }
    if error.is_connect() {
        return Refused::from(failed("All connection attempts failed"));
    }
    Refused::from(failed(&credential.redact(&describe(error))))
}

/// `{origin}/v1/installations/{id}/bible/passage`, the id as one path segment.
fn passage_url(origin: &str, id: &str) -> Result<Url, BibleError> {
    let mut url = Url::parse(origin.trim_end_matches('/')).map_err(|_| {
        failed("the control plane address is missing an 'http://' or 'https://' protocol.")
    })?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(failed(&format!(
            "the control plane address has an unsupported protocol '{}://'.",
            url.scheme()
        )));
    }
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| failed("the control plane address cannot be used"))?;
        segments
            .pop_if_empty()
            .extend(["v1", "installations", id, "bible", "passage"]);
    }
    Ok(url)
}

/// The text a person reads for a refused request, from the status and the service's error body.
fn status_error(
    status: u16,
    body: &[u8],
    retry_after: Option<u64>,
    version: &str,
    credential: &Secret,
) -> BibleError {
    let parsed: Option<Value> = std::str::from_utf8(body)
        .ok()
        .and_then(|text| serde_json::from_str(text).ok());
    let field = |key: &str| {
        parsed
            .as_ref()
            .and_then(|v| v.get(key))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let code = field("error").unwrap_or_default();
    let message = {
        let text = credential.redact(&field("message").unwrap_or_default());
        let text: String = text.chars().take(200).collect();
        text.trim().to_string()
    };
    let wait = |seconds: Option<u64>| match seconds {
        Some(s) => format!(", try again in {s} seconds"),
        None => String::new(),
    };
    let reason = match (status, code.as_str()) {
        (401, _) => "this installation's credential was rejected (restart StagePilot to sign in again, or add your own Bible key)".to_string(),
        (429, _) => {
            let scope = match field("scope").as_deref() {
                Some("installation_hourly") => "hourly limit for this installation",
                Some("installation_daily") => "daily limit for this installation",
                Some("global_daily") => "daily limit of the service",
                _ => "limit",
            };
            format!(
                "rate limit exceeded ({scope}{}); you can add your own Bible key",
                wait(retry_after)
            )
        }
        (503, "provider_not_configured") => format!(
            "the service has no key for {version} yet (add your own Bible key)"
        ),
        (503, "upstream_busy") => format!("the Bible provider is busy{}", wait(retry_after)),
        (504, _) => "the Bible provider did not answer in time (HTTP 504)".to_string(),
        (400, "invalid_version") => format!("the service does not offer {version}"),
        (400, code) if !code.is_empty() => {
            if message.is_empty() {
                format!("the service refused the request ({code})")
            } else {
                format!("the service refused the request ({code}): {message}")
            }
        }
        (other, code) if !code.is_empty() => format!("HTTP {other} ({code})"),
        (other, _) => format!("HTTP {other}"),
    };
    request_failed(&reason)
}

/// One request's worth of verses: a chapter, at most 40 verses, and the reference text that asks for them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Request {
    pub reference: String,
    /// `(chapter, verse)` of every verse the answer must hold, in order.
    pub verses: Vec<(u64, u64)>,
}

/// Split a passage into requests: one chapter and at most 40 verses each, in the order asked. A range across chapters
/// (`John 3:16-4:2`) becomes one request per chapter. Verses that would repeat one already in the request (overlapping spans)
/// start a new request, because the service refuses overlapping spans.
pub(crate) fn plan_requests(book_id: &str, spans: &[VerseSpan]) -> Vec<Request> {
    let mut requests: Vec<Request> = Vec::new();
    let mut current: Vec<(u64, u64)> = Vec::new();
    let flush = |current: &mut Vec<(u64, u64)>, requests: &mut Vec<Request>| {
        if !current.is_empty() {
            requests.push(Request {
                reference: reference_text(book_id, current),
                verses: std::mem::take(current),
            });
        }
    };
    for span in spans {
        for (chapter, verse) in expand_span(book_id, span) {
            let fits = match current.last() {
                None => true,
                Some(&(c, v)) => {
                    c == chapter && verse > v && current.len() < MAX_VERSES_PER_REQUEST
                }
            };
            if !fits {
                flush(&mut current, &mut requests);
            }
            current.push((chapter, verse));
        }
    }
    flush(&mut current, &mut requests);
    requests
}

/// `JHN 3:16-18 & 20` for the verses of one chapter (strictly increasing).
fn reference_text(book_id: &str, verses: &[(u64, u64)]) -> String {
    let chapter = verses.first().map_or(0, |&(c, _)| c);
    let mut runs: Vec<(u64, u64)> = Vec::new();
    for &(_, verse) in verses {
        match runs.last_mut() {
            Some((_, end)) if *end + 1 == verse => *end = verse,
            _ => runs.push((verse, verse)),
        }
    }
    let runs: Vec<String> = runs
        .iter()
        .map(|&(a, b)| {
            if a == b {
                a.to_string()
            } else {
                format!("{a}-{b}")
            }
        })
        .collect();
    format!("{book_id} {chapter}:{}", runs.join(" & "))
}

impl BibleProvider for StagePilotProvider {
    async fn list_versions(&self) -> Result<Vec<BibleVersion>, BibleError> {
        Ok(SERVICE_VERSIONS
            .iter()
            .filter(|(label, _)| self.serves(label).is_some())
            .map(|(label, name)| BibleVersion {
                abbreviation: (*label).to_string(),
                name: (*name).to_string(),
                language: "eng".to_string(),
                provider: Some(self.name.clone()),
                // The service is asked by label; the cache keys a verse by this.
                provider_version_id: Some((*label).to_string()),
                copyright: None,
                attribution: None,
            })
            .collect())
    }

    async fn fetch_passage(
        &self,
        version: &str,
        book_id: &str,
        spans: &[VerseSpan],
    ) -> Result<Vec<BibleVerse>, BibleError> {
        let Some(label) = self.serves(version) else {
            return Err(BibleError::Provider(format!(
                "{LABEL} does not offer {version}"
            )));
        };
        if !self.is_available() {
            return Err(unavailable());
        }
        let mut verses = Vec::new();
        for request in plan_requests(book_id, spans) {
            let passage = self.fetch_reference(label, &request.reference).await?;
            if self.scope == Scope::Stored && passage.provider == "esv" {
                // Crossway's terms: the stored scope must never be handed ESV text (the service moved a version to ESV).
                return Err(BibleError::Provider(format!(
                    "{LABEL} answered {label} with ESV text, which is never stored; update VerseVision or add your own {label} key; {NOT_STORED}"
                )));
            }
            let got: Vec<(&str, u64, u64)> = passage
                .verses
                .iter()
                .map(|v| (v.book_id.as_str(), v.chapter, v.verse))
                .collect();
            let want: Vec<(&str, u64, u64)> = request
                .verses
                .iter()
                .map(|&(c, v)| (book_id, c, v))
                .collect();
            if got != want {
                return Err(failed(
                    "the verses of the answer are not the ones asked for",
                ));
            }
            for verse in passage.verses {
                if verse.text.is_empty() {
                    return Err(BibleError::Provider(format!(
                        "{LABEL} returned no text for {book_id} {}:{}",
                        verse.chapter, verse.verse
                    )));
                }
                verses.push(BibleVerse {
                    version: label.to_string(),
                    ..verse
                });
            }
        }
        Ok(verses)
    }

    async fn health_check(&self) -> ProviderHealth {
        let provider = Some(self.name.clone());
        if !self.is_available() {
            return ProviderHealth {
                healthy: false,
                message: "The StagePilot Bible service is not available: this copy of VerseVision is not connected to a StagePilot installation".to_string(),
                provider,
                versions: Vec::new(),
            };
        }
        let probe = if self.scope == Scope::Esv {
            "ESV"
        } else {
            "KJV"
        };
        match self.fetch_reference(probe, "JHN 3:16").await {
            Ok(_) => ProviderHealth {
                healthy: true,
                message: "StagePilot Bible service reachable".to_string(),
                provider,
                versions: SERVICE_VERSIONS
                    .iter()
                    .filter(|(label, _)| self.serves(label).is_some())
                    .map(|(label, _)| (*label).to_string())
                    .collect(),
            },
            Err(error) => ProviderHealth {
                healthy: false,
                message: error.message().to_string(),
                provider,
                versions: Vec::new(),
            },
        }
    }

    /// ESV text is never stored; the rest is stored like the underlying provider's.
    fn cacheable(&self) -> bool {
        self.scope == Scope::Stored
    }

    fn cache_policy(&self) -> CachePolicy {
        match self.scope {
            Scope::Stored => CachePolicy::Allowed,
            Scope::Esv => CachePolicy::Fresh,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use versevision_core::references::VersePoint;

    fn span(a: (u64, u64), b: Option<(u64, u64)>) -> VerseSpan {
        VerseSpan {
            start: VersePoint {
                chapter: a.0,
                verse: a.1,
            },
            end: b.map(|(chapter, verse)| VersePoint { chapter, verse }),
        }
    }

    #[test]
    fn the_pilcrow_rule_strips_only_a_leading_mark() {
        assert_eq!(clean_text("\u{b6} For God so loved"), "For God so loved");
        assert_eq!(clean_text("  \u{b6}\u{a0}For God"), "For God");
        assert_eq!(clean_text("\u{b6} \u{b6} For God"), "For God");
        // everything else stays, including a mark inside the verse and other symbols
        assert_eq!(clean_text("And he said \u{b6} go"), "And he said \u{b6} go");
        assert_eq!(clean_text("[16] For God"), "[16] For God");
        assert_eq!(
            clean_text("For God\u{a0}so \u{2014} loved"),
            "For God\u{a0}so \u{2014} loved"
        );
        assert_eq!(clean_text("\u{b6}"), "");
    }

    #[test]
    fn requests_are_split_per_chapter_and_per_forty_verses() {
        // John 3:16-4:2: the last verse of John 3 is 36
        let planned = plan_requests("JHN", &[span((3, 16), Some((4, 2)))]);
        let refs: Vec<&str> = planned.iter().map(|r| r.reference.as_str()).collect();
        assert_eq!(refs, ["JHN 3:16-36", "JHN 4:1-2"]);
        assert_eq!(planned[0].verses.len(), 21);
        // Psalm 119 has 176 verses: 40 + 40 + 40 + 40 + 16
        let planned = plan_requests("PSA", &[span((119, 1), Some((119, 176)))]);
        let sizes: Vec<usize> = planned.iter().map(|r| r.verses.len()).collect();
        assert_eq!(sizes, [40, 40, 40, 40, 16]);
        assert_eq!(planned[1].reference, "PSA 119:41-80");
        // spans in one chapter share a request; a jump to another chapter starts one
        let planned = plan_requests(
            "1SA",
            &[
                span((3, 4), None),
                span((3, 6), Some((3, 7))),
                span((4, 2), None),
            ],
        );
        let refs: Vec<&str> = planned.iter().map(|r| r.reference.as_str()).collect();
        assert_eq!(refs, ["1SA 3:4 & 6-7", "1SA 4:2"]);
        // overlapping spans never share a request (the service refuses them)
        let planned = plan_requests("JHN", &[span((3, 16), Some((3, 18))), span((3, 17), None)]);
        assert_eq!(planned.len(), 2);
        // a range that runs backwards asks for nothing, as the other providers
        assert!(plan_requests("JHN", &[span((3, 18), Some((3, 16)))]).is_empty());
    }
}
