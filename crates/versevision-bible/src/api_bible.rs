//! `bibles/api_bible.py`: API.Bible (`api-key` header, `GET /bibles?language=eng`, `GET /bibles/{id}/verses/{BOOK.C.V}`).

use std::sync::{Arc, OnceLock};

use regex::Regex;

use serde_json::Value;
use versevision_core::bibles::{BibleError, BibleProvider, BibleVerse, ProviderHealth};
use versevision_core::config::{CachePolicy, RemoteProviderConfig};
use versevision_core::references::VerseSpan;
use versevision_core::versions::BibleVersion;

use crate::html::soup_text;
use crate::http::{Endpoint, Wording};
use crate::keys::KeyStore;
use crate::remote::{
    configured_id, require_key, select_version, single_chapter_verses, truthy, upper, VersionList,
};
use crate::secret::Secret;

const WORDING: Wording = Wording {
    label: "API.Bible",
    invalid_key: "invalid API key",
};

pub struct ApiBibleProvider {
    name: String,
    config: RemoteProviderConfig,
    keys: Arc<dyn KeyStore>,
    http: Endpoint,
    versions: VersionList,
}

impl std::fmt::Debug for ApiBibleProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiBibleProvider")
            .field("name", &self.name)
            .field("base_url", &self.config.base_url)
            .field("api_key_env", &self.config.api_key_env)
            .finish_non_exhaustive()
    }
}

/// What the provider makes of one verse's `content`: `BeautifulSoup(content, "html.parser").get_text(" ", strip=True)` with a
/// leading `[16]` / `[16-17]` marker removed.
pub fn verse_text(content: &str) -> String {
    strip_leading_verse_marker(&soup_text(content)).to_string()
}

/// `_LEADING_VERSE_MARKER.sub("", text)`: `^\[\d+(?:-\d+)?\]\s*` (`\d` is any decimal digit, `\s` any space Python knows).
fn strip_leading_verse_marker(text: &str) -> &str {
    static MARKER: OnceLock<Regex> = OnceLock::new();
    let marker = MARKER.get_or_init(|| Regex::new(r"^\[\p{Nd}+(?:-\p{Nd}+)?\]").expect("pattern"));
    match marker.find(text) {
        Some(found) => text[found.end()..].trim_start_matches(versevision_core::pystr::is_space),
        None => text,
    }
}

impl ApiBibleProvider {
    pub fn new(name: &str, config: RemoteProviderConfig, keys: Arc<dyn KeyStore>) -> Self {
        let http = Endpoint::new(WORDING, &config.base_url, config.timeout_seconds);
        Self {
            name: name.to_string(),
            config,
            keys,
            http,
            versions: VersionList::default(),
        }
    }

    fn key(&self) -> Result<Secret, BibleError> {
        require_key(
            &self.keys,
            &self.config.api_key_env,
            "API.Bible key is missing.",
        )
    }

    async fn get(&self, route: &str, params: &[(&str, &str)]) -> Result<Value, BibleError> {
        let key = self.key()?;
        let header = ("api-key", key.expose().to_string());
        self.http.get_json(route, params, header, &key).await
    }

    fn shape(&self) -> BibleError {
        self.http.failed("unexpected response shape")
    }

    async fn list(&self) -> Result<Arc<Vec<BibleVersion>>, BibleError> {
        if let Some(versions) = self.versions.get() {
            return Ok(versions);
        }
        let payload = self.get("/bibles", &[("language", "eng")]).await?;
        let object = payload.as_object().ok_or_else(|| self.shape())?;
        let data: &[Value] = match object.get("data") {
            None => &[],
            Some(Value::Array(items)) => items,
            Some(_) => return Err(self.shape()),
        };
        let mut versions = Vec::new();
        for item in data {
            let item = item.as_object().ok_or_else(|| self.shape())?;
            let abbreviation = upper(
                truthy(item.get("abbreviationLocal"))
                    .or_else(|| truthy(item.get("abbreviation")))
                    .unwrap_or(""),
            );
            if abbreviation.is_empty() {
                continue;
            }
            let id = match item.get("id") {
                Some(Value::String(id)) if !id.is_empty() => id.clone(),
                None | Some(Value::Null | Value::String(_)) => continue,
                Some(_) => return Err(self.shape()),
            };
            let language = match item.get("language") {
                None | Some(Value::Null) => None,
                Some(Value::Object(language)) => truthy(language.get("id")).map(str::to_string),
                Some(_) => return Err(self.shape()),
            };
            versions.push(BibleVersion {
                name: truthy(item.get("nameLocal"))
                    .or_else(|| truthy(item.get("name")))
                    .map_or_else(|| abbreviation.clone(), str::to_string),
                abbreviation,
                language: language.unwrap_or_else(|| "eng".to_string()),
                provider: Some(self.name.clone()),
                provider_version_id: Some(id),
                copyright: self.optional_text(item.get("copyright"))?,
                attribution: self.optional_text(item.get("description"))?,
            });
        }
        Ok(self.versions.set(versions))
    }

    /// A field the model holds as `str | None`: null/missing is `None`, a string is itself, anything else is not a Bible response.
    fn optional_text(&self, value: Option<&Value>) -> Result<Option<String>, BibleError> {
        match value {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(text)) => Ok(Some(text.clone())),
            Some(_) => Err(self.shape()),
        }
    }

    async fn version(&self, abbreviation: &str) -> Result<BibleVersion, BibleError> {
        let versions = self.list().await?;
        select_version(
            "API.Bible",
            &self.name,
            abbreviation,
            &versions,
            configured_id(&self.config.bible_ids, abbreviation),
        )
    }
}

impl BibleProvider for ApiBibleProvider {
    async fn list_versions(&self) -> Result<Vec<BibleVersion>, BibleError> {
        Ok(self.list().await?.as_ref().clone())
    }

    async fn fetch_passage(
        &self,
        version: &str,
        book_id: &str,
        spans: &[VerseSpan],
    ) -> Result<Vec<BibleVerse>, BibleError> {
        let selected = self.version(version).await?;
        let version_id = selected.provider_version_id.clone().unwrap_or_default();
        let mut verses = Vec::new();
        for span in spans {
            let numbers = single_chapter_verses(
                span,
                "Cross-chapter API.Bible ranges are not enabled yet; no files changed",
            )?;
            for (chapter, number) in numbers {
                let verse_id = format!("{book_id}.{chapter}.{number}");
                let payload = self
                    .get(
                        &format!("/bibles/{version_id}/verses/{verse_id}"),
                        &[
                            ("content-type", "text"),
                            ("include-notes", "false"),
                            ("include-titles", "false"),
                        ],
                    )
                    .await?;
                let object = payload.as_object().ok_or_else(|| self.shape())?;
                let empty = serde_json::Map::new();
                let data = match object.get("data") {
                    None | Some(Value::Null) => &empty,
                    Some(Value::Object(data)) => data,
                    Some(_) => return Err(self.shape()),
                };
                let content = match data.get("content") {
                    None | Some(Value::Null) => "",
                    Some(Value::String(text)) => text,
                    Some(_) => return Err(self.shape()),
                };
                let text = verse_text(content);
                if text.is_empty() {
                    return Err(BibleError::Provider(format!(
                        "API.Bible returned no text for {verse_id}"
                    )));
                }
                verses.push(BibleVerse {
                    book_id: book_id.to_string(),
                    chapter,
                    verse: number,
                    text,
                    version: selected.abbreviation.clone(),
                    paragraph_start: false,
                    poetry_indent: 0,
                    copyright: truthy(data.get("copyright"))
                        .map(str::to_string)
                        .or_else(|| selected.copyright.clone()),
                    attribution: selected.attribution.clone(),
                });
            }
        }
        Ok(verses)
    }

    async fn health_check(&self) -> ProviderHealth {
        let provider = Some(self.name.clone());
        if self.keys.resolve(&self.config.api_key_env).is_none() {
            return ProviderHealth {
                healthy: false,
                message: format!(
                    "Environment variable {} is not set",
                    self.config.api_key_env
                ),
                provider,
                versions: Vec::new(),
            };
        }
        match self.list().await {
            Ok(versions) => ProviderHealth {
                healthy: !versions.is_empty(),
                message: format!("API.Bible exposes {} translation(s)", versions.len()),
                provider,
                versions: versions.iter().map(|v| v.abbreviation.clone()).collect(),
            },
            Err(error) => ProviderHealth {
                healthy: false,
                message: error.message().to_string(),
                provider,
                versions: Vec::new(),
            },
        }
    }

    fn cache_policy(&self) -> CachePolicy {
        self.config.cache_policy
    }
}
