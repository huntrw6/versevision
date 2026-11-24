//! `bible.source`: when the StagePilot Bible service supplies verse text.
//!
//! [`BibleSources`] holds two registries: the user's own providers (what `[bible.providers]` and `[bible.version_routes]` say, the
//! registry [`RegistryBuilder::build`](crate::RegistryBuilder::build) makes) and the StagePilot service (one stored provider for every
//! version but ESV, one never-stored provider for ESV). Every lookup picks one for the reference's version:
//!
//! | `bible.source` | a version the service offers | any other version |
//! |---|---|---|
//! | `own_keys` | own providers | own providers |
//! | `stagepilot` | the service | own providers |
//! | `auto` | own providers when the key store has the key of the provider that serves the version (or the provider needs no key: local USFM); else the service when the host gave an [`InstallationAuth`]; else own providers (their normal missing-key text) | own providers |
//!
//! The choice is made at every lookup and the key is read then, so a key added in the Config page counts at once.
//!
//! **Default install** (deliberate, additive difference from Python): a config with no `[bible.providers.*]` gets the built-in API.Bible,
//! YouVersion and ESV providers and a route for each version the service offers ([`versevision_core::config::BibleConfig::builtin_providers`]),
//! so "own" above means those, and a key saved in the key store is used by the versions its provider serves (ESV key: ESV; API.Bible key:
//! NKJV NLT KJV MSG WEB; YouVersion key: NIV AMP BSB LSV NIRV NASB1995 NASB2020 TPT). A config that defines any provider table is not touched.

use std::sync::Arc;

use versevision_core::bibles::{
    BibleError, BiblePassages, ProviderHealth, ProviderRegistry, ResolvedPassage, VerseCache,
};
use versevision_core::config::BibleSource;
use versevision_core::notes::TraceSink;
use versevision_core::references::{casefold, PassageReference};

use crate::keys::KeyStore;
use crate::provider::AnyProvider;
use crate::stagepilot::{service_label, InstallationAuth, StagePilotProvider, SERVICE_VERSIONS};

/// The name the service's stored provider has in the registry (and in the scripture cache's keys).
pub const SERVICE_PROVIDER: &str = "stagepilot";
/// The name of the service's never-stored ESV provider.
pub const SERVICE_ESV_PROVIDER: &str = "stagepilot-esv";

/// Where one lookup is answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// The user's own providers.
    Own,
    /// The StagePilot service.
    Service,
}

/// What the planner reads Bible text through when the StagePilot service may be used; it follows `bible.source`.
pub struct BibleSources {
    own: ProviderRegistry<AnyProvider>,
    /// The built-in providers and routes, in use when the config defines no provider table (then they answer instead of `own`).
    builtin: Option<ProviderRegistry<AnyProvider>>,
    service: ProviderRegistry<AnyProvider>,
    keys: Arc<dyn KeyStore>,
    auth: Option<Arc<dyn InstallationAuth>>,
    source: BibleSource,
}

impl std::fmt::Debug for BibleSources {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BibleSources")
            .field("source", &self.source)
            .field("has_auth", &self.auth.is_some())
            .field("own", &self.own.providers.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl BibleSources {
    pub(crate) fn new(
        own: ProviderRegistry<AnyProvider>,
        builtin: Option<ProviderRegistry<AnyProvider>>,
        source: BibleSource,
        keys: Arc<dyn KeyStore>,
        cache: Option<Arc<dyn VerseCache>>,
        auth: Option<Arc<dyn InstallationAuth>>,
    ) -> Self {
        let mut routes = indexmap::IndexMap::new();
        for (label, _) in SERVICE_VERSIONS {
            let name = if label == "ESV" {
                SERVICE_ESV_PROVIDER
            } else {
                SERVICE_PROVIDER
            };
            routes.insert(label.to_string(), name.to_string());
        }
        let mut providers = indexmap::IndexMap::new();
        providers.insert(
            SERVICE_PROVIDER.to_string(),
            AnyProvider::StagePilot(StagePilotProvider::new(SERVICE_PROVIDER, auth.clone())),
        );
        providers.insert(
            SERVICE_ESV_PROVIDER.to_string(),
            AnyProvider::StagePilot(StagePilotProvider::for_esv(
                SERVICE_ESV_PROVIDER,
                auth.clone(),
            )),
        );
        let mut service = ProviderRegistry::new(routes, providers);
        if let Some(cache) = cache {
            service = service.with_cache(cache);
        }
        Self {
            own,
            builtin,
            service,
            keys,
            auth,
            source,
        }
    }

    /// The user's own providers, as [`RegistryBuilder::build`](crate::RegistryBuilder::build) makes them.
    pub fn own(&self) -> &ProviderRegistry<AnyProvider> {
        &self.own
    }

    /// The registry that answers "own" lookups: the built-in providers when the config defines none, else [`BibleSources::own`].
    fn effective_own(&self) -> &ProviderRegistry<AnyProvider> {
        self.builtin.as_ref().unwrap_or(&self.own)
    }

    /// Is the config using the built-in providers (it defines no `[bible.providers.*]`)?
    pub fn uses_builtin_providers(&self) -> bool {
        self.builtin.is_some()
    }

    pub fn source(&self) -> BibleSource {
        self.source
    }

    /// Does the host have an installation to ask on behalf of (an id and a credential)?
    pub fn service_available(&self) -> bool {
        self.auth.as_ref().is_some_and(|auth| {
            auth.installation_id().is_some_and(|id| !id.is_empty())
                && auth.credential().is_some_and(|c| !c.is_empty())
        })
    }

    /// Can the user's own providers answer `version` right now: a provider routes it and has its key (or needs none)?
    fn own_ready(&self, version: &str) -> bool {
        let wanted = casefold(version);
        let own = self.effective_own();
        let Some(route) = own
            .version_routes
            .iter()
            .find(|(label, _)| casefold(label) == wanted)
            .map(|(_, provider)| provider)
        else {
            return false;
        };
        own.providers
            .get(route)
            .is_some_and(|provider| provider.own_ready(self.keys.as_ref()))
    }

    /// Where `version` is answered under the current `bible.source`, keys and installation.
    pub fn route(&self, version: &str) -> Route {
        if service_label(version).is_none() {
            return Route::Own;
        }
        match self.source {
            BibleSource::OwnKeys => Route::Own,
            BibleSource::StagePilot => Route::Service,
            BibleSource::Auto => {
                if self.own_ready(version) {
                    Route::Own
                } else if self.service_available() {
                    Route::Service
                } else {
                    Route::Own
                }
            }
        }
    }

    /// One health line per provider that can be used under `bible.source`.
    pub async fn health_checks(&self) -> Vec<ProviderHealth> {
        let mut out = Vec::new();
        if !self.own.providers.is_empty() || self.source == BibleSource::OwnKeys {
            out.extend(self.own.health_checks().await);
        }
        if self.source != BibleSource::OwnKeys {
            out.extend(self.service.health_checks().await);
        }
        out
    }
}

impl BiblePassages for BibleSources {
    async fn fetch(
        &self,
        reference: &PassageReference,
        trace: &mut dyn TraceSink,
    ) -> Result<ResolvedPassage, BibleError> {
        let route = match reference.version.as_deref().filter(|v| !v.is_empty()) {
            Some(version) => self.route(version),
            None => Route::Own,
        };
        match route {
            Route::Own => self.effective_own().fetch(reference, trace).await,
            Route::Service => self.service.fetch(reference, trace).await,
        }
    }

    fn description(&self) -> String {
        let effective = self.effective_own();
        let own = effective.description();
        let own = if effective.providers.is_empty() {
            "providers: none of your own".to_string()
        } else {
            own
        };
        match self.source {
            BibleSource::OwnKeys => own,
            BibleSource::StagePilot => "providers: StagePilot service".to_string(),
            BibleSource::Auto => format!("{own}; StagePilot service when there is no key"),
        }
    }
}
