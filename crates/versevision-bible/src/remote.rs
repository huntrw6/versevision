//! What API.Bible, YouVersion and the ESV API share: reading the key, asking for a verse one at a time, picking the Bible
//! the config or the key's list says (`_version`).

use std::sync::{Arc, Mutex};

use serde_json::Value;
use versevision_core::bibles::BibleError;
use versevision_core::pystr;
use versevision_core::references::casefold;
use versevision_core::references::VerseSpan;
use versevision_core::versions::BibleVersion;

use crate::keys::KeyStore;
use crate::secret::Secret;

/// The key named by `api_key_env` (the environment first, then the keys file), or Python's "key is missing" error.
pub(crate) fn require_key(
    keys: &Arc<dyn KeyStore>,
    env_name: &str,
    missing: &str,
) -> Result<Secret, BibleError> {
    keys.resolve(env_name).ok_or_else(|| {
        BibleError::Provider(format!(
            "{missing} Set the {env_name} environment variable."
        ))
    })
}

/// Python `item.get(key) or ...`: a non-empty string (anything else is "no value").
pub(crate) fn truthy(value: Option<&Value>) -> Option<&str> {
    match value {
        Some(Value::String(text)) if !text.is_empty() => Some(text),
        _ => None,
    }
}

/// `str.upper()`.
pub(crate) fn upper(text: &str) -> String {
    text.to_uppercase()
}

/// `" ".join(text.split())`.
pub(crate) fn flatten(text: &str) -> String {
    pystr::flatten(text)
}

/// Python's `end = span.end or span.start`, and the cross-chapter refusal.
pub(crate) fn single_chapter_verses(
    span: &VerseSpan,
    cross_chapter_message: &str,
) -> Result<impl Iterator<Item = (u64, u64)>, BibleError> {
    let end = span.end.unwrap_or(span.start);
    if end.chapter != span.start.chapter {
        return Err(BibleError::Provider(cross_chapter_message.to_string()));
    }
    let chapter = span.start.chapter;
    Ok((span.start.verse..=end.verse).map(move |verse| (chapter, verse)))
}

/// The list of Bibles a key can read, asked for once and kept (`self._versions`).
#[derive(Debug, Default)]
pub(crate) struct VersionList(Mutex<Option<Arc<Vec<BibleVersion>>>>);

impl VersionList {
    pub fn get(&self) -> Option<Arc<Vec<BibleVersion>>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn set(&self, versions: Vec<BibleVersion>) -> Arc<Vec<BibleVersion>> {
        let versions = Arc::new(versions);
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(versions.clone());
        versions
    }
}

/// `_version` of API.Bible and YouVersion: the configured id for the label when there is one, else the only Bible with the label.
pub(crate) fn select_version(
    label: &str,
    provider_name: &str,
    abbreviation: &str,
    versions: &[BibleVersion],
    configured_id: Option<&str>,
) -> Result<BibleVersion, BibleError> {
    let wanted = casefold(abbreviation);
    let matches: Vec<&BibleVersion> = versions
        .iter()
        .filter(|item| casefold(&item.abbreviation) == wanted)
        .collect();
    if let Some(id) = configured_id.filter(|id| !id.is_empty()) {
        return matches
            .iter()
            .find(|item| item.provider_version_id.as_deref() == Some(id))
            .map(|item| (*item).clone())
            .ok_or_else(|| {
                BibleError::Provider(format!(
                    "Configured {label} ID for {abbreviation} is not available to this key"
                ))
            });
    }
    match matches.as_slice() {
        [] => Err(BibleError::Provider(format!(
            "{label} key does not provide translation {abbreviation}"
        ))),
        [one] => Ok((*one).clone()),
        _ => Err(BibleError::Provider(format!(
            "{label} returned multiple Bible IDs for {abbreviation}; configure providers.{provider_name}.bible_ids.{abbreviation}"
        ))),
    }
}

/// The configured id of `abbreviation` in `bible_ids` (case-insensitive key, first match).
pub(crate) fn configured_id<'a>(
    bible_ids: &'a indexmap::IndexMap<String, String>,
    abbreviation: &str,
) -> Option<&'a str> {
    let wanted = casefold(abbreviation);
    bible_ids
        .iter()
        .find(|(key, _)| casefold(key) == wanted)
        .map(|(_, value)| value.as_str())
}
