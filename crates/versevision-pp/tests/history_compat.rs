//! `imports.json` written by an older Python app (fewer fields, other date forms) is read with Python's defaults, a file that is not a list of
//! valid records reads as empty, and what Rust writes is `json.dump(indent=2)` as Python writes it.

use std::fs;
use std::sync::Arc;

use versevision_pp::clock::VirtualClock;
use versevision_pp::history::HistoryStore;

fn store(dir: &std::path::Path) -> HistoryStore {
    HistoryStore::new(
        dir.join("data/imports.json"),
        Arc::new(VirtualClock::new(1_784_419_571, "h")),
    )
}

#[test]
fn an_older_file_gets_the_defaults_of_the_new_fields() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("data")).unwrap();
    fs::write(
        dir.path().join("data/imports.json"),
        r#"[{"import_id": "abc", "created": "2026-01-02T03:04:05+00:00", "source_name": "n.txt", "source_sha256": "s", "target_uuid": "T", "target_name": "N",
             "future_field": 1}]"#,
    )
    .unwrap();
    let records = store(dir.path()).load();
    assert_eq!(records.len(), 1);
    let r = &records[0];
    assert_eq!(
        (
            r.node.as_str(),
            r.status.as_str(),
            r.mode.as_str(),
            r.file_sha256.as_str()
        ),
        ("local", "active", "append", "")
    );
    assert_eq!(r.created, "2026-01-02T03:04:05Z");
    assert!(
        r.cue_uuids.is_empty()
            && r.playlist_items.is_empty()
            && r.backup_path.is_none()
            && r.library.is_none()
    );
}

#[test]
fn a_file_that_is_not_a_list_of_valid_records_reads_as_empty() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("data")).unwrap();
    let path = dir.path().join("data/imports.json");
    for text in [
        "",
        "not json",
        "{}",
        r#"[{"import_id": "a"}]"#,
        r#"[{"import_id": 1, "source_name": "n", "source_sha256": "s", "target_uuid": "t", "target_name": "n"}]"#,
    ] {
        fs::write(&path, text).unwrap();
        assert!(store(dir.path()).load().is_empty(), "{text:?}");
    }
}

#[test]
fn what_is_written_reads_back_and_looks_like_python_dumped_it() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("data")).unwrap();
    let path = dir.path().join("data/imports.json");
    fs::write(
        &path,
        "[\n  {\n    \"import_id\": \"abc\",\n    \"created\": \"2026-01-02T03:04:05Z\",\n    \"source_name\": \"caf\\u00e9.txt\",\n    \"source_sha256\": \"s\",\n    \"target_uuid\": \"T\",\n    \"target_name\": \"N\"\n  }\n]",
    )
    .unwrap();
    let store = store(dir.path());
    let records = store.load();
    assert_eq!(records[0].source_name, "café.txt");
    store.set_status("abc", "undone").unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("[\n  {\n    \"import_id\": \"abc\",\n    \"created\": \"2026-01-02T03:04:05Z\",\n    \"source_name\": \"caf\\u00e9.txt\""), "{text}");
    assert!(text.contains("\"status\": \"undone\""));
    assert!(!text.ends_with('\n'), "json.dump writes no final newline");
    assert_eq!(store.get("abc").unwrap().status, "undone");
}
