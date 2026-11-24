//! A default install (a config with no `[bible.providers.*]`): the built-in API.Bible, YouVersion and ESV providers, so a key saved in
//! the key store is used by the versions its provider serves, and `bible.source` works as on a configured install.

mod common;

use std::sync::Arc;

use common::service::*;
use common::*;
use serde_json::{json, Value};
use versevision_bible::{BibleSources, KeyStore, MemoryKeyStore, RegistryBuilder, Secret};
use versevision_core::bibles::BiblePassages;
use versevision_core::config::{AppConfig, BibleConfig, BUILTIN_VERSION_ROUTES};
use versevision_core::notes::TraceLog;

struct Setup {
    sources: BibleSources,
    own: Arc<Script>,
    service: Arc<Service>,
}

/// Own servers: API.Bible (`/v1`: NKJV), YouVersion (`/yv`: NIV, id 111) and ESV (`/v3`); their text starts with `OWN`.
async fn setup(config: Value, keys: &[&str], auth: bool, builtin_for_tests: bool) -> Setup {
    let own = Script::new(&json!({
        "/v1/bibles": [{"json": {"data": [{"id": "63097d2a0a2f7db3-01", "abbreviation": "NKJV"}, {"id": "other", "abbreviation": "NKJV"}]}}],
        "/v1/bibles/63097d2a0a2f7db3-01/verses/JHN.3.16": [{"json": {"data": {"content": "OWN api text 16"}}}],
        "/yv/bibles": [{"json": {"data": [{"id": 111, "abbreviation": "XXX"}]}}],
        "/yv/bibles/111/passages/JHN.3.16": [{"json": {"content": "OWN yv text 16"}}],
        "/v3/passage/text/": [{"json": {"passages": ["OWN esv text"]}}],
    }));
    let addr = serve(own.clone()).await;
    let (origin, service) = serve_service(|seen, _| contract_success(seen)).await;
    let store = Arc::new(MemoryKeyStore::new());
    for name in keys {
        store.set_key(name, Secret::new(SYNTHETIC_KEY)).unwrap();
    }
    let mut builder = RegistryBuilder::new(store);
    if builtin_for_tests {
        builder = builder.builtin_providers(builtin_at(&format!("http://{addr}")));
    }
    if auth {
        builder = builder.installation_auth(TestAuth::enrolled(&origin));
    }
    let text = config
        .to_string()
        .replace(ORIGIN, &format!("http://{addr}"));
    let bible =
        AppConfig::from_value(&json!({"bible": serde_json::from_str::<Value>(&text).unwrap()}))
            .unwrap()
            .bible;
    Setup {
        sources: builder.build_sources(&bible),
        own,
        service,
    }
}

/// `(first verse text or the error, provider of the trace record)`
async fn ask(s: &Setup, text: &str) -> (Result<String, String>, String) {
    let parsed = versevision_core::references::parse_references(text, None).unwrap();
    let reference = serde_json::from_value(parsed["references"][0].clone()).unwrap();
    let mut trace = TraceLog::default();
    let result = s.sources.fetch(&reference, &mut trace).await;
    let provider = trace
        .records
        .last()
        .and_then(|r| {
            r.1.get("provider")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default();
    (
        result
            .map(|p| p.verses[0].text.clone())
            .map_err(|e| e.message().to_string()),
        provider,
    )
}

#[test]
fn the_built_in_table_is_the_services_table() {
    let bible = BibleConfig::builtin_providers();
    assert_eq!(
        bible.providers.keys().collect::<Vec<_>>(),
        ["api_bible", "youversion", "esv"]
    );
    assert_eq!(bible.version_routes.len(), 14);
    let labels: Vec<&str> = versevision_bible::SERVICE_VERSIONS
        .iter()
        .map(|(l, _)| *l)
        .collect();
    let routed: Vec<&str> = BUILTIN_VERSION_ROUTES.iter().map(|(v, _, _)| *v).collect();
    assert_eq!(routed, labels, "the same versions as the service offers");
    for (version, provider, id) in BUILTIN_VERSION_ROUTES {
        assert_eq!(bible.version_routes[version], provider);
        assert_eq!(
            provider_of(version),
            match provider {
                "esv" => "esv",
                p => p,
            }
        );
        let ids = match &bible.providers[provider] {
            versevision_core::config::ProviderConfig::ApiBible(c)
            | versevision_core::config::ProviderConfig::YouVersion(c) => Some(c.bible_ids.clone()),
            _ => None,
        };
        match ids {
            Some(ids) => assert_eq!(ids[version], id),
            None => assert!(id.is_empty()),
        }
    }
    // the settings are the schema's own defaults
    let api = serde_json::to_value(&bible.providers["api_bible"]).unwrap();
    assert_eq!(api["api_key_env"], "API_BIBLE_API_KEY");
    assert_eq!(api["base_url"], "https://api.scripture.api.bible/v1");
    let esv = serde_json::to_value(&bible.providers["esv"]).unwrap();
    assert_eq!(esv["cache_policy"], "fresh");
}

#[test]
fn a_users_route_wins_over_a_built_in_one_without_regard_to_case() {
    let user = AppConfig::from_value(
        &json!({"bible": {"version_routes": {"nkjv": "mine", "ASV": "mine"}}}),
    )
    .unwrap()
    .bible;
    let merged = user.with_builtin_providers(&BibleConfig::builtin_providers());
    assert_eq!(merged.version_routes["nkjv"], "mine");
    assert!(!merged.version_routes.contains_key("NKJV"));
    assert_eq!(merged.version_routes["NIV"], "youversion");
    assert_eq!(merged.version_routes["ASV"], "mine");
    assert_eq!(merged.providers.len(), 3);
    // a config with a provider table is returned as it is
    let own = AppConfig::from_value(&json!({"bible": {"providers": {"x": {"type": "esv"}}}}))
        .unwrap()
        .bible;
    assert_eq!(
        own.with_builtin_providers(&BibleConfig::builtin_providers()),
        own
    );
}

#[tokio::test]
async fn with_only_an_esv_key_esv_uses_it_and_niv_uses_the_service_or_says_the_key_is_missing() {
    // installation: ESV is answered with the own key; NIV and NKJV (no key) by the service
    let s = setup(json!({}), &["ESV_API_KEY"], true, true).await;
    assert!(s.sources.uses_builtin_providers());
    assert_eq!(
        ask(&s, "John 3:16 ESV").await,
        (Ok("OWN esv text".into()), "esv".into())
    );
    assert_eq!(s.service.count(), 0);
    assert_eq!(
        ask(&s, "John 3:16 NIV").await,
        (
            Ok("Synthetic NIV text JHN 3:16.".into()),
            "stagepilot".into()
        )
    );
    assert_eq!(
        ask(&s, "John 3:16 NKJV").await.0.unwrap(),
        "Synthetic NKJV text JHN 3:16."
    );
    assert_eq!(s.service.count(), 2);

    // no installation: NIV says what a missing key says (no request at all), ESV still works
    let s = setup(json!({}), &["ESV_API_KEY"], false, true).await;
    assert_eq!(ask(&s, "John 3:16 ESV").await.0.unwrap(), "OWN esv text");
    assert_eq!(
        ask(&s, "John 3:16 NIV").await.0.unwrap_err(),
        "YouVersion key is missing. Set the YOUVERSION_API_KEY environment variable."
    );
    assert_eq!(s.service.count(), 0);
}

#[tokio::test]
async fn each_key_serves_its_own_versions_by_the_built_in_bible_id() {
    // API.Bible key: NKJV uses the configured id (the second NKJV in the list is not ambiguous); the others go to the service
    let s = setup(json!({}), &["API_BIBLE_API_KEY"], true, true).await;
    assert_eq!(
        ask(&s, "John 3:16 NKJV").await,
        (Ok("OWN api text 16".into()), "api_bible".into())
    );
    assert_eq!(ask(&s, "John 3:16 NIV").await.1, "stagepilot");
    // YouVersion key: NIV is the id 111 whatever the provider calls it
    let s = setup(json!({}), &["YOUVERSION_API_KEY"], true, true).await;
    assert_eq!(
        ask(&s, "John 3:16 NIV").await,
        (Ok("OWN yv text 16".into()), "youversion".into())
    );
    assert_eq!(s.service.count(), 0);
}

#[tokio::test]
async fn with_no_keys_auto_uses_the_service() {
    let s = setup(json!({}), &[], true, true).await;
    for (version, expected) in [
        ("ESV", "stagepilot-esv"),
        ("NIV", "stagepilot"),
        ("KJV", "stagepilot"),
    ] {
        let (text, provider) = ask(&s, &format!("John 3:16 {version}")).await;
        assert_eq!(text.unwrap(), format!("Synthetic {version} text JHN 3:16."));
        assert_eq!(provider, expected);
    }
    assert_eq!(
        s.own.log.lock().unwrap().len(),
        0,
        "no own server was asked"
    );
}

#[tokio::test]
async fn own_keys_and_stagepilot_follow_their_meaning_on_a_default_install() {
    // own_keys: never the service; no key = the missing-key text instead of "no route"
    let s = setup(json!({"source": "own_keys"}), &[], true, true).await;
    assert_eq!(
        ask(&s, "John 3:16 ESV").await.0.unwrap_err(),
        "ESV key is missing. Set the ESV_API_KEY environment variable."
    );
    assert_eq!(s.service.count(), 0);
    // stagepilot: the service even with a key
    let s = setup(
        json!({"source": "stagepilot"}),
        &["ESV_API_KEY"],
        true,
        true,
    )
    .await;
    assert_eq!(ask(&s, "John 3:16 ESV").await.1, "stagepilot-esv");
    assert_eq!(s.own.log.lock().unwrap().len(), 0);
    // a version nobody offers keeps the old "no route" text
    assert_eq!(
        ask(&s, "John 3:16 ASV").await.0.unwrap_err(),
        "No Bible provider route is configured for ASV"
    );
}

#[tokio::test]
async fn a_config_with_provider_tables_behaves_as_before() {
    let config = json!({
        "version_routes": {"NKJV": "api"},
        "providers": {"api": {"type": "api_bible", "base_url": format!("{ORIGIN}/v1")}},
    });
    // the ESV key is saved, but this config has no ESV provider and no ESV route: nothing built in is added
    let s = setup(
        config.clone(),
        &["ESV_API_KEY", "API_BIBLE_API_KEY"],
        false,
        true,
    )
    .await;
    assert!(!s.sources.uses_builtin_providers());
    assert_eq!(
        ask(&s, "John 3:16 ESV").await.0.unwrap_err(),
        "No Bible provider route is configured for ESV"
    );
    assert_eq!(
        ask(&s, "John 3:16 NIV").await.0.unwrap_err(),
        "No Bible provider route is configured for NIV"
    );
    // no built-in bible id is added to the user's provider (the fake server lists NKJV twice): Python's own error
    assert_eq!(
        ask(&s, "John 3:16 NKJV").await.0.unwrap_err(),
        "API.Bible returned multiple Bible IDs for NKJV; configure providers.api.bible_ids.NKJV"
    );
    assert_eq!(s.sources.own().providers.len(), 1);
}

#[test]
fn the_description_names_the_built_in_providers() {
    let sources = RegistryBuilder::new(Arc::new(MemoryKeyStore::new()))
        .build_sources(&BibleConfig::default());
    assert_eq!(
        sources.description(),
        "providers: api_bible, youversion, esv; StagePilot service when there is no key"
    );
    // the config's own list stays what `versevision providers test` shows
    assert!(sources.own().providers.is_empty());
}
