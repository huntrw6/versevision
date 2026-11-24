//! The planner (S3) reading verse text from the real providers: `plan_notes` over a `ProviderRegistry<AnyProvider>` with local USFM
//! files gives the plan the Python app wrote when the same text came from `bible.json` (case `synthetic-acceptance-usfm`).

mod common;

use std::fs;

use common::*;
use serde_json::{json, Value};
use versevision_bible::{KeyStore, RegistryBuilder};
use versevision_core::bibles::BiblePassages;
use versevision_core::config::AppConfig;
use versevision_core::notes::TraceLog;
use versevision_core::plan::PlanOptions;
use versevision_core::workflow::plan_notes;

#[tokio::test(flavor = "multi_thread")]
async fn the_planner_gets_its_text_from_usfm_files() {
    let case = repo_root().join("oracle/cases/synthetic-acceptance-usfm");
    let text = fs::read_to_string(case.join("expected/extracted.txt")).unwrap();
    let expected: Value =
        serde_json::from_str(&fs::read_to_string(case.join("expected/plan.json")).unwrap())
            .unwrap();
    let fixture = format!(
        "{}/python/tests/fixtures/usfm/synthetic",
        repo_root().display()
    );
    let bible = bible_config(
        &json!({
            "version_routes": {"NIV": "synthetic_niv", "NKJV": "synthetic_nkjv"},
            "providers": {
                "synthetic_niv": {"type": "usfm", "version": "NIV", "path": fixture, "name": "Synthetic NIV acceptance fixture", "copyright": "Synthetic test content \u{2014} not Scripture", "cache_policy": "fresh"},
                "synthetic_nkjv": {"type": "usfm", "version": "NKJV", "path": fixture, "name": "Synthetic NKJV acceptance fixture", "copyright": "Synthetic test content \u{2014} not Scripture", "cache_policy": "fresh"},
            },
        }),
        "http://unused",
    );
    let mut config =
        AppConfig::from_toml_str(&fs::read_to_string(case.join("options.toml")).unwrap()).unwrap();
    config.bible = bible.clone();
    let registry =
        RegistryBuilder::new(keys_with(&[]) as std::sync::Arc<dyn KeyStore>).build(&bible);
    let options = PlanOptions {
        default_version: Some("NKJV".to_string()),
        no_fetch: false,
    };
    let mut trace = TraceLog::default();
    // run on a worker thread of the multi-threaded runtime: the planner future with these providers is `Send`
    let result = tokio::spawn(async move {
        let mut trace = TraceLog::default();
        let result = plan_notes(&text, &config, &options, Some(&registry), &mut trace)
            .await
            .unwrap();
        (
            serde_json::to_value(&result).unwrap(),
            trace,
            registry.description(),
        )
    })
    .await
    .unwrap();
    let (actual, provider_trace, description) = result;
    trace.records = provider_trace.records;
    assert_eq!(description, "providers: synthetic_niv, synthetic_nkjv");
    assert_eq!(expected["decisions"], actual["decisions"], "decisions");
    // what differs is what Python's offline provider cannot say: the copyright of the text (it is the folder's, from the config)
    let mut slides = actual["slide_plan"]["slides"].clone();
    for slide in slides.as_array_mut().unwrap() {
        assert_eq!(
            slide["copyright_text"],
            "Synthetic test content \u{2014} not Scripture"
        );
        slide["copyright_text"] = Value::Null;
    }
    assert_eq!(expected["slide_plan"]["slides"], slides, "slides");
    let providers: Vec<&str> = trace
        .records
        .iter()
        .filter(|(stage, _)| stage == "provider")
        .filter_map(|(_, f)| f["provider"].as_str())
        .collect();
    assert!(!providers.is_empty() && providers.iter().all(|p| p.starts_with("synthetic_")));
    println!(
        "plan over USFM: {} decisions and {} slides identical to Python's plan over bible.json",
        expected["decisions"].as_array().unwrap().len(),
        expected["slide_plan"]["slides"].as_array().unwrap().len()
    );
}
