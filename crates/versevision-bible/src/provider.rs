//! [`AnyProvider`]: the one provider type the planner's registry holds, and how a registry is built from `[bible]`.
//!
//! The four kinds Python has are variants; [`AnyProvider::Custom`] holds any other kind behind an object-safe trait
//! ([`DynBibleProvider`]), so a new kind (S5b's StagePilot service) is added by implementing that trait and handing it to
//! [`RegistryBuilder::provider`]. The planner only sees `ProviderRegistry<AnyProvider>` as `BiblePassages`, and never changes.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use indexmap::IndexMap;
use versevision_core::bibles::{
    BibleError, BibleProvider, BibleVerse, ProviderHealth, ProviderRegistry, VerseCache,
};
use versevision_core::config::{BibleConfig, CachePolicy, ProviderConfig};
use versevision_core::references::VerseSpan;
use versevision_core::versions::BibleVersion;

use crate::api_bible::ApiBibleProvider;
use crate::esv::EsvProvider;
use crate::keys::KeyStore;
use crate::usfm::LocalUsfmProvider;
use crate::youversion::YouVersionProvider;

/// A future a provider returns when it is used through [`DynBibleProvider`].
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// [`BibleProvider`] as an object-safe trait, for provider kinds this crate does not know.
pub trait DynBibleProvider: std::fmt::Debug + Send + Sync {
    fn list_versions(&self) -> BoxFuture<'_, Result<Vec<BibleVersion>, BibleError>>;
    fn fetch_passage<'a>(
        &'a self,
        version: &'a str,
        book_id: &'a str,
        spans: &'a [VerseSpan],
    ) -> BoxFuture<'a, Result<Vec<BibleVerse>, BibleError>>;
    fn health_check(&self) -> BoxFuture<'_, ProviderHealth>;
    fn cacheable(&self) -> bool {
        true
    }
    fn cache_policy(&self) -> CachePolicy {
        CachePolicy::Allowed
    }
}

#[derive(Debug)]
pub enum AnyProvider {
    ApiBible(ApiBibleProvider),
    YouVersion(YouVersionProvider),
    Esv(EsvProvider),
    Usfm(LocalUsfmProvider),
    Custom(Box<dyn DynBibleProvider>),
}

impl BibleProvider for AnyProvider {
    async fn list_versions(&self) -> Result<Vec<BibleVersion>, BibleError> {
        match self {
            Self::ApiBible(p) => p.list_versions().await,
            Self::YouVersion(p) => p.list_versions().await,
            Self::Esv(p) => p.list_versions().await,
            Self::Usfm(p) => p.list_versions().await,
            Self::Custom(p) => p.list_versions().await,
        }
    }

    async fn fetch_passage(
        &self,
        version: &str,
        book_id: &str,
        spans: &[VerseSpan],
    ) -> Result<Vec<BibleVerse>, BibleError> {
        match self {
            Self::ApiBible(p) => p.fetch_passage(version, book_id, spans).await,
            Self::YouVersion(p) => p.fetch_passage(version, book_id, spans).await,
            Self::Esv(p) => p.fetch_passage(version, book_id, spans).await,
            Self::Usfm(p) => p.fetch_passage(version, book_id, spans).await,
            Self::Custom(p) => p.fetch_passage(version, book_id, spans).await,
        }
    }

    async fn health_check(&self) -> ProviderHealth {
        match self {
            Self::ApiBible(p) => p.health_check().await,
            Self::YouVersion(p) => p.health_check().await,
            Self::Esv(p) => p.health_check().await,
            Self::Usfm(p) => p.health_check().await,
            Self::Custom(p) => p.health_check().await,
        }
    }

    fn cacheable(&self) -> bool {
        match self {
            Self::ApiBible(p) => p.cacheable(),
            Self::YouVersion(p) => p.cacheable(),
            Self::Esv(p) => p.cacheable(),
            Self::Usfm(p) => p.cacheable(),
            Self::Custom(p) => p.cacheable(),
        }
    }

    fn cache_policy(&self) -> CachePolicy {
        match self {
            Self::ApiBible(p) => p.cache_policy(),
            Self::YouVersion(p) => p.cache_policy(),
            Self::Esv(p) => p.cache_policy(),
            Self::Usfm(p) => p.cache_policy(),
            Self::Custom(p) => p.cache_policy(),
        }
    }
}

/// Builds the planner's registry from `[bible]`: one provider per `[bible.providers.NAME]` (in the file's order), then any
/// provider added with [`RegistryBuilder::provider`], routed by `bible.version_routes`.
pub struct RegistryBuilder {
    keys: Arc<dyn KeyStore>,
    cache: Option<Arc<dyn VerseCache>>,
    extra: IndexMap<String, AnyProvider>,
}

impl std::fmt::Debug for RegistryBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegistryBuilder")
            .field("extra", &self.extra.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl RegistryBuilder {
    /// `keys`: where the providers find their API keys.
    pub fn new(keys: Arc<dyn KeyStore>) -> Self {
        Self {
            keys,
            cache: None,
            extra: IndexMap::new(),
        }
    }

    /// Keep verses in `cache` (a [`crate::ScriptureCache`]); without one every lookup asks the provider.
    pub fn cache(mut self, cache: Arc<dyn VerseCache>) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Add a provider of a kind that is not in the config (it replaces a configured provider of the same name).
    pub fn provider(mut self, name: &str, provider: AnyProvider) -> Self {
        self.extra.insert(name.to_string(), provider);
        self
    }

    pub fn build(self, bible: &BibleConfig) -> ProviderRegistry<AnyProvider> {
        let mut providers: IndexMap<String, AnyProvider> = IndexMap::new();
        for (name, config) in &bible.providers {
            let provider = match config {
                ProviderConfig::ApiBible(c) => {
                    AnyProvider::ApiBible(ApiBibleProvider::new(name, c.clone(), self.keys.clone()))
                }
                ProviderConfig::YouVersion(c) => AnyProvider::YouVersion(YouVersionProvider::new(
                    name,
                    c.clone(),
                    self.keys.clone(),
                )),
                ProviderConfig::Esv(c) => {
                    AnyProvider::Esv(EsvProvider::new(name, c.clone(), self.keys.clone()))
                }
                ProviderConfig::Usfm(c) => {
                    AnyProvider::Usfm(LocalUsfmProvider::new(name, c.clone()))
                }
            };
            providers.insert(name.clone(), provider);
        }
        providers.extend(self.extra);
        let registry = ProviderRegistry::new(bible.version_routes.clone(), providers);
        match self.cache {
            Some(cache) => registry.with_cache(cache),
            None => registry,
        }
    }
}
