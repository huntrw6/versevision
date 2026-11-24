//! `bibles/registry.py`: route a version to the provider that has it, ask the provider, write the `provider` trace record.
//!
//! Which provider answers a version is `bible.version_routes` (version label, case-insensitive -> provider name); the providers
//! themselves are built from `bible.providers` by the caller (S5), one type `P` (an enum of the four kinds, or a test double).

use std::time::Instant;

use indexmap::IndexMap;

use super::{BibleError, BiblePassages, BibleProvider, ProviderHealth, ResolvedPassage};
use crate::notes::trace::emit;
use crate::notes::TraceSink;
use crate::pyfmt::fixed;
use crate::references::pytext::casefold;
use crate::references::PassageReference;
use crate::trace_fields;

#[derive(Debug)]
pub struct ProviderRegistry<P> {
    /// `bible.version_routes`: version label -> provider name.
    pub version_routes: IndexMap<String, String>,
    /// The configured providers by name (`bible.providers`).
    pub providers: IndexMap<String, P>,
}

impl<P: BibleProvider> ProviderRegistry<P> {
    pub fn new(version_routes: IndexMap<String, String>, providers: IndexMap<String, P>) -> Self {
        Self {
            version_routes,
            providers,
        }
    }

    /// `_provider`: the name and the provider that answers `version`.
    fn provider(&self, version: &str) -> Result<(&str, &P), BibleError> {
        let wanted = casefold(version);
        let route = self
            .version_routes
            .iter()
            .find(|(key, _)| casefold(key) == wanted)
            .map(|(_, provider)| provider.as_str())
            .ok_or_else(|| {
                BibleError::Configuration(format!(
                    "No Bible provider route is configured for {version}"
                ))
            })?;
        match self.providers.get_key_value(route) {
            Some((name, provider)) => Ok((name.as_str(), provider)),
            None => Err(BibleError::Configuration(format!(
                "Bible provider route {version} points to unknown provider {route}"
            ))),
        }
    }

    /// `_fetch`: the passage, the name of the provider that answered and where the text came from.
    async fn answer(
        &self,
        reference: &PassageReference,
    ) -> Result<(ResolvedPassage, String, &'static str), BibleError> {
        let Some(version) = reference.version.as_deref().filter(|v| !v.is_empty()) else {
            return Err(BibleError::Provider(
                "Cannot fetch a reference without a resolved version".to_string(),
            ));
        };
        let (name, provider) = self.provider(version)?;
        let versions = provider.list_versions().await?;
        let wanted = casefold(version);
        let metadata = versions
            .iter()
            .find(|item| casefold(&item.abbreviation) == wanted);
        let Some(metadata) = metadata.filter(|m| {
            m.provider_version_id
                .as_deref()
                .is_some_and(|id| !id.is_empty())
        }) else {
            return Err(BibleError::Provider(format!(
                "Provider {name} does not expose {version}"
            )));
        };
        let verses = provider
            .fetch_passage(version, &reference.book_id, &reference.spans)
            .await?;
        let source = if provider.cacheable() {
            "live"
        } else {
            "live (not stored)"
        };
        Ok((
            ResolvedPassage {
                reference: reference.clone(),
                resolved_version: metadata.abbreviation.clone(),
                verses,
            },
            name.to_string(),
            source,
        ))
    }

    /// `health_checks`.
    pub async fn health_checks(&self) -> Vec<ProviderHealth> {
        if self.providers.is_empty() {
            return vec![ProviderHealth {
                healthy: false,
                message: "No Bible providers are configured".to_string(),
                provider: None,
                versions: Vec::new(),
            }];
        }
        let mut out = Vec::new();
        for provider in self.providers.values() {
            out.push(provider.health_check().await);
        }
        out
    }
}

impl<P: BibleProvider> BiblePassages for ProviderRegistry<P> {
    async fn fetch(
        &self,
        reference: &PassageReference,
        trace: &mut dyn TraceSink,
    ) -> Result<ResolvedPassage, BibleError> {
        let started = Instant::now();
        let ms = |started: Instant| {
            fixed(started.elapsed().as_secs_f64() * 1000.0, 0)
                .parse::<u64>()
                .unwrap_or(0)
        };
        let info = trace_fields! {
            "reference" => reference.normalized_text,
            "version" => reference.version,
        };
        match self.answer(reference).await {
            Ok((passage, provider, source)) => {
                let mut fields = trace_fields! { "provider" => provider, "source" => source, "ms" => ms(started) };
                fields.extend(info);
                emit(trace, "provider", fields);
                Ok(passage)
            }
            Err(error) => {
                let mut fields = trace_fields! { "error" => error.message(), "ms" => ms(started) };
                fields.extend(info);
                emit(trace, "provider", fields);
                Err(error)
            }
        }
    }

    fn description(&self) -> String {
        let names: Vec<&str> = self.providers.keys().map(String::as_str).collect();
        format!("providers: {}", names.join(", "))
    }
}
