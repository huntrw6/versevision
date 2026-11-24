//! `bibles/youversion.py`: YouVersion (`X-YVP-App-Key` header, `GET /bibles?language_ranges[]=en&page_size=99`,
//! `GET /bibles/{id}/passages/{BOOK.C.V}?format=text`).

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;
use versevision_core::bibles::{BibleError, BibleProvider, BibleVerse, ProviderHealth};
use versevision_core::config::{CachePolicy, RemoteProviderConfig};
use versevision_core::references::VerseSpan;
use versevision_core::versions::BibleVersion;

use crate::http::{Endpoint, Wording};
use crate::keys::KeyStore;
use crate::remote::{
    configured_id, flatten, require_key, select_version, single_chapter_verses, truthy, upper,
    VersionList,
};
use crate::secret::Secret;

const WORDING: Wording = Wording {
    label: "YouVersion",
    invalid_key: "invalid API key or unavailable license",
};

pub struct YouVersionProvider {
    name: String,
    config: RemoteProviderConfig,
    keys: Arc<dyn KeyStore>,
    http: Endpoint,
    versions: VersionList,
}

impl std::fmt::Debug for YouVersionProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("YouVersionProvider")
            .field("name", &self.name)
            .field("base_url", &self.config.base_url)
            .field("api_key_env", &self.config.api_key_env)
            .finish_non_exhaustive()
    }
}

/// Python's `str(identifier)` for the JSON values an id can be.
fn identifier_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(true) => Some("True".to_string()),
        Value::Bool(false) => Some("False".to_string()),
        _ => None,
    }
}

impl YouVersionProvider {
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
            "YouVersion key is missing.",
        )
    }

    async fn get(&self, route: &str, params: &[(&str, &str)]) -> Result<Value, BibleError> {
        let key = self.key()?;
        let header = ("x-yvp-app-key", key.expose().to_string());
        self.http.get_json(route, params, header, &key).await
    }

    fn shape(&self) -> BibleError {
        self.http.failed("unexpected response shape")
    }

    fn optional_text(&self, value: Option<&Value>) -> Result<Option<String>, BibleError> {
        match value {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(text)) => Ok(Some(text.clone())),
            Some(_) => Err(self.shape()),
        }
    }

    async fn list(&self) -> Result<Arc<Vec<BibleVersion>>, BibleError> {
        if let Some(versions) = self.versions.get() {
            return Ok(versions);
        }
        let payload = self
            .get(
                "/bibles",
                &[("language_ranges[]", "en"), ("page_size", "99")],
            )
            .await?;
        let object = payload.as_object().ok_or_else(|| self.shape())?;
        // `{str(value): key.upper()}`: a later key with the same id wins
        let mut aliases: HashMap<&str, String> = HashMap::new();
        for (key, value) in &self.config.bible_ids {
            aliases.insert(value.as_str(), upper(key));
        }
        let data: &[Value] = match object.get("data") {
            None => &[],
            Some(Value::Array(items)) => items,
            Some(_) => return Err(self.shape()),
        };
        let mut versions = Vec::new();
        for item in data {
            let item = item.as_object().ok_or_else(|| self.shape())?;
            let identifier = match item.get("id") {
                None | Some(Value::Null) => None,
                Some(value) => Some(identifier_text(value).ok_or_else(|| self.shape())?),
            };
            let abbreviation = identifier
                .as_deref()
                .and_then(|id| aliases.get(id))
                .filter(|alias| !alias.is_empty())
                .cloned()
                .unwrap_or_else(|| {
                    upper(
                        truthy(item.get("localized_abbreviation"))
                            .or_else(|| truthy(item.get("abbreviation")))
                            .unwrap_or(""),
                    )
                });
            let Some(identifier) = identifier else {
                continue;
            };
            if abbreviation.is_empty() {
                continue;
            }
            versions.push(BibleVersion {
                name: truthy(item.get("localized_title"))
                    .or_else(|| truthy(item.get("title")))
                    .map_or_else(|| abbreviation.clone(), str::to_string),
                language: truthy(item.get("language_tag"))
                    .map_or_else(|| "en".to_string(), str::to_string),
                abbreviation,
                provider: Some(self.name.clone()),
                provider_version_id: Some(identifier),
                copyright: self.optional_text(item.get("copyright"))?,
                attribution: match truthy(item.get("promotional_content")) {
                    Some(text) => Some(text.to_string()),
                    None => self.optional_text(item.get("publisher_url"))?,
                },
            });
        }
        Ok(self.versions.set(versions))
    }

    async fn version(&self, abbreviation: &str) -> Result<BibleVersion, BibleError> {
        let versions = self.list().await?;
        select_version(
            "YouVersion",
            &self.name,
            abbreviation,
            &versions,
            configured_id(&self.config.bible_ids, abbreviation),
        )
    }
}

impl BibleProvider for YouVersionProvider {
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
                "Cross-chapter YouVersion ranges are not enabled yet; no files changed",
            )?;
            for (chapter, number) in numbers {
                let verse_id = format!("{book_id}.{chapter}.{number}");
                let payload = self
                    .get(
                        &format!("/bibles/{version_id}/passages/{verse_id}"),
                        &[
                            ("format", "text"),
                            ("include_headings", "false"),
                            ("include_notes", "false"),
                        ],
                    )
                    .await?;
                let object = payload.as_object().ok_or_else(|| self.shape())?;
                let content = match object.get("content") {
                    None | Some(Value::Null) => String::new(),
                    Some(Value::String(text)) => flatten(text),
                    Some(_) => return Err(self.shape()),
                };
                if content.is_empty() {
                    return Err(BibleError::Provider(format!(
                        "YouVersion returned no text for {verse_id}"
                    )));
                }
                verses.push(BibleVerse {
                    book_id: book_id.to_string(),
                    chapter,
                    verse: number,
                    text: content,
                    version: selected.abbreviation.clone(),
                    paragraph_start: false,
                    poetry_indent: 0,
                    copyright: selected.copyright.clone(),
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
                message: format!("YouVersion exposes {} translation(s)", versions.len()),
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

impl YouVersionProvider {
    /// The name of the key this provider reads (for `bible.source = auto`: is there one?).
    pub(crate) fn key_env(&self) -> &str {
        &self.config.api_key_env
    }
}
