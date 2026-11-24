//! The scripture cache: keys like Python's `ScriptureCache`, safe for concurrent use, and ESV text never reaches it.

mod common;

use std::fs;
use std::path::Path;
use std::sync::Arc;

use common::*;
use serde_json::json;
use versevision_bible::{RegistryBuilder, ScriptureCache};
use versevision_core::bibles::{BiblePassages, BibleVerse, VerseCache};
use versevision_core::notes::TraceLog;

fn verse(book: &str, chapter: u64, number: u64, text: &str) -> BibleVerse {
    BibleVerse {
        book_id: book.to_string(),
        chapter,
        verse: number,
        text: text.to_string(),
        version: "TST".to_string(),
        paragraph_start: true,
        poetry_indent: 2,
        copyright: Some("Synthetic copyright".to_string()),
        attribution: Some("Synthetic attribution".to_string()),
    }
}

#[test]
fn round_trip_count_and_clear_like_python() {
    let directory = tempfile::tempdir().unwrap();
    let cache = ScriptureCache::new(directory.path().join("scripture"));
    assert_eq!(cache.count(), 0);
    assert!(cache.get("p", "v1", "JHN", 3, 16).is_none());
    let stored = verse("JHN", 3, 16, "Synthetic verse.");
    cache.put("p", "v1", &stored);
    assert_eq!(cache.get("p", "v1", "JHN", 3, 16), Some(stored.clone()));
    // every part of the key matters
    for (provider, id, book, chapter, number) in [
        ("q", "v1", "JHN", 3, 16),
        ("p", "v2", "JHN", 3, 16),
        ("p", "v1", "ROM", 3, 16),
        ("p", "v1", "JHN", 4, 16),
        ("p", "v1", "JHN", 3, 17),
    ] {
        assert!(cache.get(provider, id, book, chapter, number).is_none());
    }
    // INSERT OR REPLACE
    let replaced = verse("JHN", 3, 16, "Replaced text.");
    cache.put("p", "v1", &replaced);
    assert_eq!(cache.get("p", "v1", "JHN", 3, 16), Some(replaced));
    cache.put("p", "v1", &verse("JHN", 3, 17, "Next."));
    cache.put("q", "v1", &verse("JHN", 3, 16, "Other provider."));
    assert_eq!(cache.count(), 3);
    assert_eq!(cache.clear(), 3);
    assert_eq!(cache.count(), 0);
    assert!(cache.get("p", "v1", "JHN", 3, 16).is_none());
    // ids and names that are not path-safe stay inside the folder
    cache.put("../../escape", "a/b\\c:d", &verse("JHN", 1, 1, "x"));
    assert_eq!(
        cache
            .get("../../escape", "a/b\\c:d", "JHN", 1, 1)
            .unwrap()
            .text,
        "x"
    );
    assert!(!directory.path().join("escape").exists());
}

#[test]
fn a_damaged_or_altered_entry_is_a_miss() {
    let directory = tempfile::tempdir().unwrap();
    let cache = ScriptureCache::new(directory.path());
    cache.put("p", "v1", &verse("JHN", 3, 16, "Synthetic verse."));
    let file = fs::read_dir(directory.path())
        .unwrap()
        .flat_map(|d| fs::read_dir(d.unwrap().path()).unwrap())
        .map(|f| f.unwrap().path())
        .next()
        .unwrap();
    let good = fs::read_to_string(&file).unwrap();
    for bad in [
        String::new(),
        "{".to_string(),
        good.replace("Synthetic verse.", "Edited text."),
        good.replace("\"chapter\":3", "\"chapter\":4"),
        good.replace("\"provider\":\"p\"", "\"provider\":\"x\""),
    ] {
        fs::write(&file, bad).unwrap();
        assert!(cache.get("p", "v1", "JHN", 3, 16).is_none());
    }
    fs::write(&file, good).unwrap();
    assert!(cache.get("p", "v1", "JHN", 3, 16).is_some());
}

#[test]
fn a_cache_that_cannot_be_written_is_silent() {
    let directory = tempfile::tempdir().unwrap();
    let blocker = directory.path().join("file");
    fs::write(&blocker, "not a folder").unwrap();
    let cache = ScriptureCache::new(blocker.join("inside"));
    cache.put("p", "v1", &verse("JHN", 3, 16, "x"));
    assert!(cache.get("p", "v1", "JHN", 3, 16).is_none());
    assert_eq!(cache.count(), 0);
}

#[test]
fn concurrent_readers_and_writers_never_see_a_half_written_entry() {
    let directory = tempfile::tempdir().unwrap();
    let cache = Arc::new(ScriptureCache::new(directory.path()));
    let long = |n: usize| format!("{} {n}", "Synthetic words ".repeat(400));
    let writers: Vec<_> = (0..4)
        .map(|n| {
            let cache = cache.clone();
            std::thread::spawn(move || {
                for round in 0..60 {
                    let text = format!("{} {n}", "Synthetic words ".repeat(400));
                    cache.put("p", "v1", &verse("JHN", 3, (round % 5) + 1, &text));
                }
            })
        })
        .collect();
    let readers: Vec<_> = (0..4)
        .map(|_| {
            let cache = cache.clone();
            let allowed: Vec<String> = (0..4).map(long).collect();
            std::thread::spawn(move || {
                let mut seen = 0;
                for _ in 0..400 {
                    for number in 1..=5 {
                        if let Some(found) = cache.get("p", "v1", "JHN", 3, number) {
                            assert!(allowed.contains(&found.text), "a mixed or cut text");
                            seen += 1;
                        }
                    }
                }
                seen
            })
        })
        .collect();
    for writer in writers {
        writer.join().unwrap();
    }
    let seen: usize = readers.into_iter().map(|r| r.join().unwrap()).sum();
    println!("readers saw {seen} complete entries while 4 writers replaced them");
    assert_eq!(cache.count(), 5);
    // no temporary file left over
    for dir in fs::read_dir(directory.path()).unwrap() {
        for file in fs::read_dir(dir.unwrap().path()).unwrap() {
            assert!(file
                .unwrap()
                .path()
                .extension()
                .is_some_and(|e| e == "json"));
        }
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

/// ESV text is never stored: not in the cache folder, not anywhere under the folder the host gave, whatever the cache policy says.
#[tokio::test(flavor = "multi_thread")]
async fn esv_text_is_never_cached_or_stored() {
    const ESV_TEXT: &str = "Synthetic ESV sentence that must never reach a disk.";
    let host_folder = tempfile::tempdir().unwrap();
    let script = Script::new(&json!({"/v3/passage/text/": [{"json": {"passages": [ESV_TEXT]}}]}));
    let addr = serve(script.clone()).await;
    for policy in ["fresh"] {
        let bible = bible_config(
            &json!({
                "version_routes": {"ESV": "esv"},
                "providers": {"esv": {"type": "esv", "base_url": format!("{ORIGIN}/v3"), "api_key_env": env_name("esv"), "cache_policy": policy}},
            }),
            &format!("http://{addr}"),
        );
        let cache = Arc::new(ScriptureCache::new(host_folder.path().join("cache")));
        let registry = RegistryBuilder::new(keys_with(&[(env_name("esv"), SYNTHETIC_KEY)]))
            .cache(cache.clone())
            .build(&bible);
        for _ in 0..3 {
            let parsed =
                versevision_core::references::parse_references("John 3:16-17 ESV", None).unwrap();
            let reference = serde_json::from_value(parsed["references"][0].clone()).unwrap();
            let mut trace = TraceLog::default();
            let passage = registry.fetch(&reference, &mut trace).await.unwrap();
            assert_eq!(passage.verses[0].text, ESV_TEXT);
            let record = &trace.records.last().unwrap().1;
            assert_eq!(record["source"], "live (not stored)");
            assert!(
                !format!("{:?}", trace.records).contains(ESV_TEXT),
                "the text is not in the trace"
            );
        }
        assert_eq!(cache.count(), 0);
    }
    // the same ESV text through a provider that is cacheable *is* stored (so the check above can fail)
    let script = Script::new(&json!({
        "/v1/bibles": [{"json": {"data": [{"id": "b1", "abbreviation": "NIV"}]}}],
        "/v1/bibles/b1/verses/JHN.3.16": [{"json": {"data": {"content": ESV_TEXT}}}],
    }));
    let addr = serve(script).await;
    let bible = bible_config(
        &json!({
            "version_routes": {"NIV": "api"},
            "providers": {"api": {"type": "api_bible", "base_url": format!("{ORIGIN}/v1"), "api_key_env": env_name("api_bible")}},
        }),
        &format!("http://{addr}"),
    );
    let cache = Arc::new(ScriptureCache::new(host_folder.path().join("cache")));
    let registry = RegistryBuilder::new(keys_with(&[(env_name("api_bible"), SYNTHETIC_KEY)]))
        .cache(cache.clone())
        .build(&bible);
    let parsed = versevision_core::references::parse_references("John 3:16 NIV", None).unwrap();
    let reference = serde_json::from_value(parsed["references"][0].clone()).unwrap();
    registry
        .fetch(&reference, &mut TraceLog::default())
        .await
        .unwrap();
    assert_eq!(cache.count(), 1);
    let stored = entries(host_folder.path());
    assert_eq!(stored.len(), 1);
    assert!(fs::read_to_string(&stored[0]).unwrap().contains(ESV_TEXT));

    // and the ESV run left nothing at all in a folder of its own
    let esv_folder = tempfile::tempdir().unwrap();
    let script = Script::new(&json!({"/v3/passage/text/": [{"json": {"passages": [ESV_TEXT]}}]}));
    let addr = serve(script).await;
    let bible = bible_config(
        &json!({
            "version_routes": {"ESV": "esv"},
            "providers": {"esv": {"type": "esv", "base_url": format!("{ORIGIN}/v3"), "api_key_env": env_name("esv")}},
        }),
        &format!("http://{addr}"),
    );
    let registry = RegistryBuilder::new(keys_with(&[(env_name("esv"), SYNTHETIC_KEY)]))
        .cache(Arc::new(ScriptureCache::new(
            esv_folder.path().join("cache"),
        )))
        .build(&bible);
    let parsed = versevision_core::references::parse_references("John 3:16 ESV", None).unwrap();
    let reference = serde_json::from_value(parsed["references"][0].clone()).unwrap();
    registry
        .fetch(&reference, &mut TraceLog::default())
        .await
        .unwrap();
    assert!(
        entries(esv_folder.path()).is_empty(),
        "{:?}",
        entries(esv_folder.path())
    );
    assert!(
        !esv_folder.path().join("cache").exists(),
        "not even the folder is made"
    );
}
