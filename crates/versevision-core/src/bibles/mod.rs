//! Bible text access: the trait the planner reads verses through, the provider routing of `bibles/registry.py`, and the offline
//! provider the oracle uses. No network and no runtime here: the real providers (API.Bible, YouVersion, ESV, USFM files) are S5
//! and implement [`BibleProvider`]; the tests drive the `async fn`s with `pollster`.
//!
//! Python's `ProviderRegistry.fetch` also looks in a cache and logs an `events.jsonl` line (`bible_fetch`). The cache (it must
//! never hold ESV text) arrives with the providers in S5; the event log is not part of the oracle.

mod memory;
mod registry;

pub use memory::InMemoryBible;
pub use registry::ProviderRegistry;

use std::fmt;

use serde::{Deserialize, Serialize};

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
}
