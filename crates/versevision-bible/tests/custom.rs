//! Another provider kind plugs into the registry without changing the planner or `AnyProvider` (S5b's StagePilot service goes here).

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use common::*;
use serde_json::json;
use versevision_bible::{
    AnyProvider, BoxFuture, DynBibleProvider, RegistryBuilder, ScriptureCache,
};
use versevision_core::bibles::{BibleError, BiblePassages, BibleVerse, ProviderHealth};
use versevision_core::config::CachePolicy;
use versevision_core::notes::TraceLog;
use versevision_core::references::VerseSpan;
use versevision_core::versions::BibleVersion;

#[derive(Debug)]
struct Service {
    calls: Arc<AtomicUsize>,
    storable: bool,
}

impl DynBibleProvider for Service {
    fn list_versions(&self) -> BoxFuture<'_, Result<Vec<BibleVersion>, BibleError>> {
        Box::pin(async {
            Ok(vec![BibleVersion {
                abbreviation: "NIV".to_string(),
                name: "Synthetic service Bible".to_string(),
                language: "eng".to_string(),
                provider: Some("service".to_string()),
                provider_version_id: Some("svc-niv".to_string()),
                copyright: Some("Synthetic".to_string()),
                attribution: None,
            }])
        })
    }

    fn fetch_passage<'a>(
        &'a self,
        _version: &'a str,
        book_id: &'a str,
        spans: &'a [VerseSpan],
    ) -> BoxFuture<'a, Result<Vec<BibleVerse>, BibleError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(spans
                .iter()
                .flat_map(|span| {
                    let end = span.end.unwrap_or(span.start);
                    (span.start.verse..=end.verse).map(move |n| (span.start.chapter, n))
                })
                .map(|(chapter, verse)| {
                    BibleVerse::new(
                        book_id,
                        chapter,
                        verse,
                        &format!("Service text {chapter}:{verse}."),
                        "NIV",
                    )
                })
                .collect())
        })
    }

    fn health_check(&self) -> BoxFuture<'_, ProviderHealth> {
        Box::pin(async {
            ProviderHealth {
                healthy: true,
                message: "service reachable".to_string(),
                provider: Some("service".to_string()),
                versions: vec!["NIV".to_string()],
            }
        })
    }

    fn cacheable(&self) -> bool {
        self.storable
    }

    fn cache_policy(&self) -> CachePolicy {
        CachePolicy::Allowed
    }
}

async fn run(storable: bool) -> (Vec<String>, usize, usize) {
    let calls = Arc::new(AtomicUsize::new(0));
    let bible = bible_config(
        &json!({"version_routes": {"NIV": "service"}, "providers": {}}),
        "http://unused",
    );
    let directory = tempfile::tempdir().unwrap();
    let cache = Arc::new(ScriptureCache::new(directory.path()));
    let registry = RegistryBuilder::new(keys_with(&[]))
        .cache(cache.clone())
        .provider(
            "service",
            AnyProvider::Custom(Box::new(Service {
                calls: calls.clone(),
                storable,
            })),
        )
        .build(&bible);
    let mut sources = Vec::new();
    for _ in 0..2 {
        let parsed =
            versevision_core::references::parse_references("John 3:16-17 NIV", None).unwrap();
        let reference = serde_json::from_value(parsed["references"][0].clone()).unwrap();
        let mut trace = TraceLog::default();
        let passage = registry.fetch(&reference, &mut trace).await.unwrap();
        assert_eq!(passage.verses.len(), 2);
        assert_eq!(passage.verses[0].text, "Service text 3:16.");
        let record = &trace.records.last().unwrap().1;
        assert_eq!(record["provider"], "service");
        sources.push(record["source"].as_str().unwrap().to_string());
    }
    assert_eq!(registry.description(), "providers: service");
    assert!(registry.health_checks().await[0].healthy);
    (sources, calls.load(Ordering::SeqCst), cache.count())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_custom_provider_is_routed_cached_and_never_stored_when_it_says_so() {
    let (sources, calls, stored) = run(true).await;
    assert_eq!(sources, ["live", "cache"]);
    assert_eq!((calls, stored), (1, 2));
    let (sources, calls, stored) = run(false).await;
    assert_eq!(sources, ["live (not stored)", "live (not stored)"]);
    assert_eq!((calls, stored), (2, 0));
}
