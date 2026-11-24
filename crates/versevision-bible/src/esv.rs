//! `bibles/esv.py`: the Crossway ESV API (`Authorization: Token ...`, `GET /passage/text/?q=John 3:16&...`). Its text is never
//! cached or stored: [`BibleProvider::cacheable`] is false, so the registry never hands it to a cache.

use std::sync::Arc;

use serde_json::Value;
use versevision_core::bibles::{BibleError, BibleProvider, BibleVerse, ProviderHealth};
use versevision_core::config::{CachePolicy, EsvProviderConfig};
use versevision_core::references::{book_by_id, VerseSpan};
use versevision_core::versions::BibleVersion;

use crate::http::{Endpoint, Wording};
use crate::keys::KeyStore;
use crate::remote::{flatten, require_key, single_chapter_verses};
use crate::secret::Secret;

const WORDING: Wording = Wording {
    label: "ESV",
    invalid_key: "invalid API key",
};

pub const COPYRIGHT: &str = "Scripture quotations are from the ESV\u{ae} Bible (The Holy Bible, English Standard Version\u{ae}), \u{a9} 2001 by Crossway, a publishing ministry of Good News Publishers. Used by permission. All rights reserved. www.esv.org";

/// The format options of every request: bare verse text.
const OPTIONS: [(&str, &str); 15] = [
    ("include-passage-references", "false"),
    ("include-verse-numbers", "false"),
    ("include-first-verse-numbers", "false"),
    ("include-footnotes", "false"),
    ("include-footnote-body", "false"),
    ("include-headings", "false"),
    ("include-short-copyright", "false"),
    ("include-copyright", "false"),
    ("include-passage-horizontal-lines", "false"),
    ("include-heading-horizontal-lines", "false"),
    ("indent-poetry", "false"),
    ("indent-paragraphs", "0"),
    ("indent-declares", "0"),
    ("indent-psalm-doxology", "0"),
    ("line-length", "0"),
];

pub struct EsvProvider {
    name: String,
    config: EsvProviderConfig,
    keys: Arc<dyn KeyStore>,
    http: Endpoint,
}

impl std::fmt::Debug for EsvProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EsvProvider")
            .field("name", &self.name)
            .field("base_url", &self.config.base_url)
            .field("api_key_env", &self.config.api_key_env)
            .finish_non_exhaustive()
    }
}

impl EsvProvider {
    pub fn new(name: &str, config: EsvProviderConfig, keys: Arc<dyn KeyStore>) -> Self {
        let http = Endpoint::new(WORDING, &config.base_url, config.timeout_seconds);
        Self {
            name: name.to_string(),
            config,
            keys,
            http,
        }
    }

    fn key(&self) -> Result<Secret, BibleError> {
        require_key(&self.keys, &self.config.api_key_env, "ESV key is missing.")
    }

    /// One verse's text, as the API gives it, with the whitespace flattened.
    async fn verse(&self, book_id: &str, chapter: u64, verse: u64) -> Result<String, BibleError> {
        let Some(book) = book_by_id(book_id) else {
            return Err(BibleError::Provider(format!("ESV: unknown book {book_id}")));
        };
        let key = self.key()?;
        let query = format!("{} {chapter}:{verse}", book.name);
        let mut params: Vec<(&str, &str)> = vec![("q", query.as_str())];
        params.extend(OPTIONS);
        let header = ("authorization", format!("Token {}", key.expose()));
        let payload = self
            .http
            .get_json("/passage/text/", &params, header, &key)
            .await?;
        let object = payload
            .as_object()
            .ok_or_else(|| self.http.failed("unexpected response shape"))?;
        let passages: Vec<&str> = match object.get("passages") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => items
                .iter()
                .map(|item| item.as_str())
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| self.http.failed("unexpected response shape"))?,
            Some(_) => return Err(self.http.failed("unexpected response shape")),
        };
        Ok(flatten(&passages.join(" ")))
    }
}

impl BibleProvider for EsvProvider {
    async fn list_versions(&self) -> Result<Vec<BibleVersion>, BibleError> {
        Ok(vec![BibleVersion {
            abbreviation: "ESV".to_string(),
            name: "English Standard Version".to_string(),
            language: "eng".to_string(),
            provider: Some(self.name.clone()),
            provider_version_id: Some("ESV".to_string()),
            copyright: Some(COPYRIGHT.to_string()),
            attribution: None,
        }])
    }

    async fn fetch_passage(
        &self,
        version: &str,
        book_id: &str,
        spans: &[VerseSpan],
    ) -> Result<Vec<BibleVerse>, BibleError> {
        if versevision_core::references::casefold(version) != "esv" {
            return Err(BibleError::Provider(format!(
                "ESV provider does not serve {version}"
            )));
        }
        let mut verses = Vec::new();
        for span in spans {
            let numbers = single_chapter_verses(
                span,
                "Cross-chapter ESV ranges are not enabled yet; no files changed",
            )?;
            for (chapter, number) in numbers {
                let text = self.verse(book_id, chapter, number).await?;
                if text.is_empty() {
                    return Err(BibleError::Provider(format!(
                        "ESV returned no text for {book_id} {chapter}:{number}"
                    )));
                }
                verses.push(BibleVerse {
                    book_id: book_id.to_string(),
                    chapter,
                    verse: number,
                    text,
                    version: "ESV".to_string(),
                    paragraph_start: false,
                    poetry_indent: 0,
                    copyright: Some(COPYRIGHT.to_string()),
                    attribution: None,
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
        match self.verse("JHN", 3, 16).await {
            Ok(_) => ProviderHealth {
                healthy: true,
                message: "ESV API reachable".to_string(),
                provider,
                versions: vec!["ESV".to_string()],
            },
            Err(error) => ProviderHealth {
                healthy: false,
                message: error.message().to_string(),
                provider,
                versions: Vec::new(),
            },
        }
    }

    /// Crossway's terms limit local storage: ESV text is never stored.
    fn cacheable(&self) -> bool {
        false
    }

    fn cache_policy(&self) -> CachePolicy {
        CachePolicy::Fresh
    }
}
