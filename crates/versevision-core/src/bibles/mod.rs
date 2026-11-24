//! Bible text access: the trait the planner reads verses through, the provider routing of `bibles/registry.py`, and the offline
//! provider the oracle uses. No network and no runtime here: the real providers (API.Bible, YouVersion, ESV, USFM files) are S5
//! and implement [`BibleProvider`]; the tests drive the `async fn`s with `pollster`.
//!
//! Python's `ProviderRegistry.fetch` also looks in a cache ([`VerseCache`], implemented by `versevision-bible`; the registry
//! only ever stores what a provider says is cacheable, so ESV text never reaches it) and logs an `events.jsonl` line
//! (`bible_fetch`); the event log is not part of the oracle and is not ported.

mod memory;
mod registry;

pub use memory::InMemoryBible;
pub use registry::ProviderRegistry;

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::config::CachePolicy;
use crate::notes::TraceSink;
pub use crate::references::models::{BibleVerse, ResolvedPassage};
use crate::references::{PassageReference, VerseSpan};
use crate::versions::BibleVersion;

/// Why a verse could not be fetched. The text is what the planner shows after `not added: `.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BibleError {
    /// `ConfigurationError`: a version with no route, a route to a provider that is not configured.
    Configuration(String),
    /// `BibleProviderError`: the provider does not have the version or the text, a network or key problem.
    Provider(String),
    /// Anything else (`VerseVisionError("provider returned no verses")`).
    Other(String),
}

impl BibleError {
    /// The Python exception class this stands for (`ConfigurationError`, `BibleProviderError`, `VerseVisionError`).
    pub fn python_exception(&self) -> &'static str {
        match self {
            Self::Configuration(_) => "ConfigurationError",
            Self::Provider(_) => "BibleProviderError",
            Self::Other(_) => "VerseVisionError",
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Self::Configuration(m) | Self::Provider(m) | Self::Other(m) => m,
        }
    }
}

impl fmt::Display for BibleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for BibleError {}

/// `ProviderHealth`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderHealth {
    pub healthy: bool,
    pub message: String,
    pub provider: Option<String>,
    #[serde(default)]
    pub versions: Vec<String>,
}

/// What the planner needs from the Bible text: one passage for one reference (the version already resolved).
///
/// The implementation writes the `provider` trace record of the lookup (`provider`, `source`, `ms`, `reference`, `version`, or
/// `error` instead of `provider`/`source`) to `trace`, as Python's registry does, so the record lands in the run's trace at the
/// place of the lookup.
#[allow(async_fn_in_trait)]
pub trait BiblePassages {
    async fn fetch(
        &self,
        reference: &PassageReference,
        trace: &mut dyn TraceSink,
    ) -> Result<ResolvedPassage, BibleError>;

    /// A line for people: where the text comes from (`bible.json (offline)`, `providers: api_bible, esv`).
    fn description(&self) -> String;
}

/// One Bible text source behind the registry (`bibles/base.py`: `BibleProvider`).
#[allow(async_fn_in_trait)]
pub trait BibleProvider {
    async fn list_versions(&self) -> Result<Vec<BibleVersion>, BibleError>;
    async fn fetch_passage(
        &self,
        version: &str,
        book_id: &str,
        spans: &[VerseSpan],
    ) -> Result<Vec<BibleVerse>, BibleError>;
    async fn health_check(&self) -> ProviderHealth;
    /// `provider.cacheable` (default true): ESV text must never be stored.
    fn cacheable(&self) -> bool {
        true
    }
    /// `config.providers[name].cache_policy`: `Allowed` reads the cache before asking the provider, `Fresh` always asks
    /// (the answer is still stored when the provider is cacheable, as in Python).
    fn cache_policy(&self) -> CachePolicy {
        CachePolicy::Allowed
    }
}

/// `bibles/cache.py` (`ScriptureCache`) as the registry sees it: verses by provider, provider version id, book, chapter and verse.
/// A cache failure is a miss (reading) or is ignored (writing): the planner must not fail because a cache file could not be
/// written. Implementations are shared between threads.
pub trait VerseCache: fmt::Debug + Send + Sync {
    fn get(
        &self,
        provider: &str,
        provider_version_id: &str,
        book_id: &str,
        chapter: u64,
        verse: u64,
    ) -> Option<BibleVerse>;
    fn put(&self, provider: &str, provider_version_id: &str, verse: &BibleVerse);
}
