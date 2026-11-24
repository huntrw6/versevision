//! `bible.source`: which of the user's own providers and the StagePilot service answers a version, with and without keys and an
//! installation, and what the scripture cache keeps of the service's text (ESV: nothing).

mod common;

use std::fs;
use std::path::Path;
use std::sync::Arc;

use common::service::*;
use common::*;
use serde_json::{json, Value};
use versevision_bible::{
    AnyProvider, BibleSources, BoxFuture, DynBibleProvider, InstallationAuth, KeyStore,
    MemoryKeyStore, RegistryBuilder, Route, ScriptureCache, Secret,
};
use versevision_core::bibles::{BibleError, BiblePassages, BibleVerse, ProviderHealth};
use versevision_core::notes::TraceLog;
use versevision_core::references::VerseSpan;
use versevision_core::versions::BibleVersion;

const API_KEY: &str = "API_BIBLE_API_KEY";
const ESV_KEY: &str = "ESV_API_KEY";

struct Setup {
    sources: BibleSources,
    own: Arc<Script>,
    service: Arc<Service>,
    keys: Arc<MemoryKeyStore>,
}

/// Own providers: API.Bible for NKJV and ASV (a version the service does not offer), the ESV API for ESV. The own server's text
/// starts with `OWN`, the service's with `Synthetic`.
async fn setup(
    source: &str,
    own_keys: &[&str],
    auth: Option<bool>,
    cache: Option<Arc<ScriptureCache>>,
    routes: Value,
) -> Setup {
    let own = Script::new(&json!({
        "/v1/bibles": [{"json": {"data": [{"id": "b1", "abbreviation": "NKJV"}, {"id": "b2", "abbreviation": "ASV"}]}}],
        "/v1/bibles/b1/verses/JHN.3.16": [{"json": {"data": {"content": "OWN api text 16"}}}],
        "/v1/bibles/b1/verses/JHN.3.17": [{"json": {"data": {"content": "OWN api text 17"}}}],
        "/v1/bibles/b2/verses/JHN.3.16": [{"json": {"data": {"content": "OWN asv text 16"}}}],
        "/v3/passage/text/": [{"json": {"passages": ["OWN esv text"]}}],
    }));
    let own_addr = serve(own.clone()).await;
    let (origin, service) = serve_service(|seen, _| contract_success(seen)).await;
    let bible = bible_config(
        &json!({
            "source": source,
            "version_routes": routes,
            "providers": {
                "api": {"type": "api_bible", "base_url": format!("{ORIGIN}/v1"), "api_key_env": API_KEY},
                "esv": {"type": "esv", "base_url": format!("{ORIGIN}/v3"), "api_key_env": ESV_KEY},
            },
        }),
        &format!("http://{own_addr}"),
    );
    let keys = Arc::new(MemoryKeyStore::new());
    for name in own_keys {
        keys.set_key(name, Secret::new(SYNTHETIC_KEY)).unwrap();
    }
    let mut builder = RegistryBuilder::new(keys.clone());
    if let Some(cache) = cache {
        builder = builder.cache(cache);
    }
    let auth: Option<Arc<dyn InstallationAuth>> = match auth {
        Some(true) => Some(TestAuth::enrolled(&origin)),
        Some(false) => Some(Arc::new(TestAuth {
            origin: origin.clone(),
            id: Some(INSTALLATION.to_string()),
            credential: None,
        })),
        None => None,
    };
    if let Some(auth) = auth {
        builder = builder.installation_auth(auth);
    }
    Setup {
        sources: builder.build_sources(&bible),
        own,
        service,
        keys,
    }
}

fn default_routes() -> Value {
    json!({"NKJV": "api", "ASV": "api", "ESV": "esv"})
}

/// `(first verse text or the error, provider name of the trace record, source of the text)`
async fn ask(setup: &Setup, text: &str) -> (Result<String, String>, String, String) {
    let parsed = versevision_core::references::parse_references(text, None).unwrap();
    let reference = serde_json::from_value(parsed["references"][0].clone()).unwrap();
    let mut trace = TraceLog::default();
    let result = setup.sources.fetch(&reference, &mut trace).await;
    let record = &trace.records.last().unwrap().1;
    let name = |key: &str| {
        record
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    (
        result
            .map(|p| p.verses[0].text.clone())
            .map_err(|e| e.message().to_string()),
        name("provider"),
        name("source"),
    )
}

fn own_requests(setup: &Setup) -> usize {
    setup.own.log.lock().unwrap().len()
}

const MISSING_API: &str =
    "API.Bible key is missing. Set the API_BIBLE_API_KEY environment variable.";

#[tokio::test]
async fn own_keys_never_uses_the_service() {
    for (keys, expect_own) in [(vec![API_KEY], true), (vec![], false)] {
        let s = setup("own_keys", &keys, Some(true), None, default_routes()).await;
        let (result, provider, _) = ask(&s, "John 3:16 NKJV").await;
        if expect_own {
            assert_eq!(result.unwrap(), "OWN api text 16");
            assert_eq!(provider, "api");
        } else {
            assert_eq!(result.unwrap_err(), MISSING_API);
        }
        // a version nothing routes: the normal "no route" error, not the service
        let (result, _, _) = ask(&s, "John 3:16 NIV").await;
        assert_eq!(
            result.unwrap_err(),
            "No Bible provider route is configured for NIV"
        );
        assert_eq!(s.service.count(), 0, "own_keys never asks the service");
    }
}

#[tokio::test]
async fn stagepilot_uses_only_the_service_for_the_versions_it_offers() {
    // a key does not matter: the service answers NKJV and ESV
    for keys in [vec![API_KEY, ESV_KEY], vec![]] {
        let s = setup("stagepilot", &keys, Some(true), None, default_routes()).await;
        let (result, provider, source) = ask(&s, "John 3:16 NKJV").await;
        assert_eq!(result.unwrap(), "Synthetic NKJV text JHN 3:16.");
        assert_eq!((provider.as_str(), source.as_str()), ("stagepilot", "live"));
        let (result, provider, source) = ask(&s, "John 3:16 ESV").await;
        assert_eq!(result.unwrap(), "Synthetic ESV text JHN 3:16.");
        assert_eq!(
            (provider.as_str(), source.as_str()),
            ("stagepilot-esv", "live (not stored)")
        );
        // no route at all is not needed for the service
        assert_eq!(
            ask(&s, "John 3:16 NIV").await.0.unwrap(),
            "Synthetic NIV text JHN 3:16."
        );
        assert_eq!(own_requests(&s), 0, "the own providers are not asked");
        // a version the service does not offer falls through to the own providers
        let (result, provider, _) = ask(&s, "John 3:16 ASV").await;
        if keys.is_empty() {
            assert_eq!(result.unwrap_err(), MISSING_API);
        } else {
            assert_eq!(result.unwrap(), "OWN asv text 16");
            assert_eq!(provider, "api");
        }
    }
    // outside StagePilot there is no service: the error says so, and the own providers are not asked
    for auth in [None, Some(false)] {
        let s = setup("stagepilot", &[API_KEY], auth, None, default_routes()).await;
        let (result, provider, _) = ask(&s, "John 3:16 NKJV").await;
        assert!(result
            .unwrap_err()
            .contains("not connected to a StagePilot installation"));
        assert_eq!(provider, "");
        assert_eq!(own_requests(&s), 0);
        assert_eq!(s.service.count(), 0);
    }
}

#[tokio::test]
async fn auto_prefers_the_own_key_then_the_service_then_the_missing_key_text() {
    // own key + installation: own wins
    let s = setup(
        "auto",
        &[API_KEY, ESV_KEY],
        Some(true),
        None,
        default_routes(),
    )
    .await;
    assert_eq!(
        ask(&s, "John 3:16 NKJV").await.0.unwrap(),
        "OWN api text 16"
    );
    assert_eq!(ask(&s, "John 3:16 ESV").await.0.unwrap(), "OWN esv text");
    assert_eq!(s.service.count(), 0);
    assert_eq!(s.sources.route("NKJV"), Route::Own);

    // no key + installation: the service
    let s = setup("auto", &[], Some(true), None, default_routes()).await;
    let (result, provider, _) = ask(&s, "John 3:16 NKJV").await;
    assert_eq!(result.unwrap(), "Synthetic NKJV text JHN 3:16.");
    assert_eq!(provider, "stagepilot");
    assert_eq!(s.sources.route("nkjv"), Route::Service);
    assert_eq!(own_requests(&s), 0);

    // the key of one provider decides for the versions that provider serves, not for the others
    let s = setup("auto", &[API_KEY], Some(true), None, default_routes()).await;
    assert_eq!(
        ask(&s, "John 3:16 NKJV").await.0.unwrap(),
        "OWN api text 16"
    );
    assert_eq!(
        ask(&s, "John 3:16 ESV").await.0.unwrap(),
        "Synthetic ESV text JHN 3:16."
    );

    // no key, no installation (standalone, or no credential yet): the normal missing-key text
    for auth in [None, Some(false)] {
        let s = setup("auto", &[], auth, None, default_routes()).await;
        assert_eq!(ask(&s, "John 3:16 NKJV").await.0.unwrap_err(), MISSING_API);
        assert_eq!(s.service.count(), 0);
        assert_eq!(s.sources.route("NKJV"), Route::Own);
    }
    // a key and no installation: own
    let s = setup("auto", &[API_KEY], None, None, default_routes()).await;
    assert_eq!(
        ask(&s, "John 3:16 NKJV").await.0.unwrap(),
        "OWN api text 16"
    );

    // a fresh install: no route configured at all, so the service answers (and without it the normal "no route" text)
    let s = setup("auto", &[], Some(true), None, json!({})).await;
    assert_eq!(
        ask(&s, "John 3:16 NIV").await.0.unwrap(),
        "Synthetic NIV text JHN 3:16."
    );
    let s = setup("auto", &[], None, None, json!({})).await;
    assert_eq!(
        ask(&s, "John 3:16 NIV").await.0.unwrap_err(),
        "No Bible provider route is configured for NIV"
    );

    // a version the service does not offer (ASV, a custom version) goes to the own providers even with an installation
    let s = setup("auto", &[], Some(true), None, default_routes()).await;
    assert_eq!(ask(&s, "John 3:16 ASV").await.0.unwrap_err(), MISSING_API);
    assert_eq!(s.service.count(), 0);
    assert_eq!(s.sources.route("ASV"), Route::Own);
    assert_eq!(s.sources.route("MyOwnBible"), Route::Own);
}

#[tokio::test]
async fn a_key_added_while_running_counts_at_once() {
    let s = setup("auto", &[], Some(true), None, default_routes()).await;
    assert_eq!(
        ask(&s, "John 3:16 NKJV").await.0.unwrap(),
        "Synthetic NKJV text JHN 3:16."
    );
    s.keys.set_key(API_KEY, Secret::new(SYNTHETIC_KEY)).unwrap();
    assert_eq!(
        ask(&s, "John 3:16 NKJV").await.0.unwrap(),
        "OWN api text 16"
    );
    s.keys.remove_key(API_KEY).unwrap();
    assert_eq!(
        ask(&s, "John 3:16 NKJV").await.0.unwrap(),
        "Synthetic NKJV text JHN 3:16."
    );
}

#[derive(Debug)]
struct Local;

impl DynBibleProvider for Local {
    fn list_versions(&self) -> BoxFuture<'_, Result<Vec<BibleVersion>, BibleError>> {
        Box::pin(async {
            Ok(vec![BibleVersion {
                abbreviation: "NIV".to_string(),
                name: "Local".to_string(),
                language: "eng".to_string(),
                provider: Some("local".to_string()),
                provider_version_id: Some("local-niv".to_string()),
                copyright: None,
                attribution: None,
            }])
        })
    }

    fn fetch_passage<'a>(
        &'a self,
        _version: &'a str,
        book_id: &'a str,
        _spans: &'a [VerseSpan],
    ) -> BoxFuture<'a, Result<Vec<BibleVerse>, BibleError>> {
        Box::pin(async move {
            Ok(vec![BibleVerse::new(
                book_id,
                3,
                16,
                "LOCAL files text",
                "NIV",
            )])
        })
    }

    fn health_check(&self) -> BoxFuture<'_, ProviderHealth> {
        Box::pin(async {
            ProviderHealth {
                healthy: true,
                message: "local".to_string(),
                provider: None,
                versions: vec![],
            }
        })
    }
}

#[tokio::test]
async fn a_provider_that_needs_no_key_counts_as_the_users_own() {
    // local files (here a custom provider standing for the USFM kind) win in auto, but not in stagepilot
    for (source, want) in [
        ("auto", "LOCAL files text"),
        ("own_keys", "LOCAL files text"),
        ("stagepilot", "Synthetic NIV text JHN 3:16."),
    ] {
        let (origin, service) = serve_service(|seen, _| contract_success(seen)).await;
        let bible = bible_config(
            &json!({"source": source, "version_routes": {"NIV": "mine"}, "providers": {}}),
            "http://unused",
        );
        let sources = RegistryBuilder::new(Arc::new(MemoryKeyStore::new()))
            .provider("mine", AnyProvider::Custom(Box::new(Local)))
            .installation_auth(TestAuth::enrolled(&origin))
            .build_sources(&bible);
        let parsed = versevision_core::references::parse_references("John 3:16 NIV", None).unwrap();
        let reference = serde_json::from_value(parsed["references"][0].clone()).unwrap();
        let passage = sources
            .fetch(&reference, &mut TraceLog::default())
            .await
            .unwrap();
        assert_eq!(passage.verses[0].text, want, "{source}");
        assert_eq!(service.count(), usize::from(want.starts_with("Synth")));
    }
}

fn entries(dir: &Path) -> Vec<String> {
    fn walk(dir: &Path, out: &mut Vec<String>) {
        if let Ok(read) = fs::read_dir(dir) {
            for entry in read.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else {
                    out.push(path.display().to_string());
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, &mut out);
    out
}

#[tokio::test]
async fn service_text_is_stored_like_the_providers_text_but_esv_never() {
    let host = tempfile::tempdir().unwrap();
    let cache = Arc::new(ScriptureCache::new(host.path().join("cache")));
    let s = setup(
        "stagepilot",
        &[],
        Some(true),
        Some(cache.clone()),
        json!({}),
    )
    .await;

    // a non-ESV passage: asked once, then read from the cache
    let (first, provider, source) = ask(&s, "John 3:16-17 NKJV").await;
    assert_eq!(first.unwrap(), "Synthetic NKJV text JHN 3:16.");
    assert_eq!((provider.as_str(), source.as_str()), ("stagepilot", "live"));
    assert_eq!(cache.count(), 2);
    let (second, _, source) = ask(&s, "John 3:16-17 NKJV").await;
    assert_eq!(second.unwrap(), "Synthetic NKJV text JHN 3:16.");
    assert_eq!(source, "cache");
    assert_eq!(
        s.service.count(),
        1,
        "the second lookup did not ask the service"
    );
    let stored = entries(host.path());
    assert_eq!(stored.len(), 2);
    assert!(fs::read_to_string(&stored[0])
        .unwrap()
        .contains("Synthetic NKJV text"));
    // a version stored by the service is separate from the same version from the user's own key
    let before = cache.count();
    s.keys.set_key(API_KEY, Secret::new(SYNTHETIC_KEY)).unwrap();

    // ESV: asked every time, never stored, not on disk anywhere under the host folder
    let calls_before = s.service.count();
    for _ in 0..3 {
        let (result, provider, source) = ask(&s, "John 3:16-17 ESV").await;
        assert_eq!(result.unwrap(), "Synthetic ESV text JHN 3:16.");
        assert_eq!(provider, "stagepilot-esv");
        assert_eq!(source, "live (not stored)");
    }
    assert_eq!(s.service.count(), calls_before + 3);
    assert_eq!(cache.count(), before, "no ESV entry in the cache");
    for path in entries(host.path()) {
        assert!(
            !fs::read_to_string(&path)
                .unwrap_or_default()
                .contains("Synthetic ESV"),
            "ESV text found in {path}"
        );
    }
    let root = host.path().display().to_string();
    assert!(!entries(host.path())
        .iter()
        .any(|p| p[root.len()..].to_lowercase().contains("esv")));

    // the response's `cached` flag changes nothing: a service copy that says cached:true is stored like any other text
    let (origin, service) = serve_service(|seen, _| {
        let mut reply = contract_success(seen);
        let mut body: Value = serde_json::from_str(&reply.body).unwrap();
        body["cached"] = json!(true);
        reply.body = body.to_string();
        reply
    })
    .await;
    let host = tempfile::tempdir().unwrap();
    let cache = Arc::new(ScriptureCache::new(host.path()));
    let bible = bible_config(&json!({"source": "stagepilot"}), "http://unused");
    let sources = RegistryBuilder::new(Arc::new(MemoryKeyStore::new()))
        .cache(cache.clone())
        .installation_auth(TestAuth::enrolled(&origin))
        .build_sources(&bible);
    let parsed = versevision_core::references::parse_references("John 3:16 KJV", None).unwrap();
    let reference = serde_json::from_value(parsed["references"][0].clone()).unwrap();
    sources
        .fetch(&reference, &mut TraceLog::default())
        .await
        .unwrap();
    assert_eq!(cache.count(), 1);
    assert_eq!(service.count(), 1);
}

#[tokio::test]
async fn description_and_health_follow_the_source() {
    let s = setup("auto", &[], Some(true), None, default_routes()).await;
    assert!(s.sources.description().contains("StagePilot service"));
    let health = s.sources.health_checks().await;
    // the own providers (no keys) and the two service providers
    assert_eq!(health.len(), 4);
    let s = setup("own_keys", &[], Some(true), None, default_routes()).await;
    assert_eq!(s.sources.description(), "providers: api, esv");
    assert_eq!(s.sources.health_checks().await.len(), 2);
    assert_eq!(s.service.count(), 0);
}
