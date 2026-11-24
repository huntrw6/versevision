//! Every ProPresenter file in the fixtures decodes and encodes back to the same bytes: the schema bindings, the field order and
//! the unknown fields are right.

use std::path::{Path, PathBuf};

use versevision_pp::Message;

fn files_named(dir: &Path, wanted: &dyn Fn(&Path) -> bool, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            files_named(&path, wanted, out);
        } else if wanted(&path) {
            out.push(path);
        }
    }
}

fn roundtrip(message: &str, files: &[PathBuf]) {
    for file in files {
        let bytes = std::fs::read(file).unwrap();
        let decoded = Message::decode_named(message, &bytes).unwrap();
        let encoded = decoded.encode();
        assert!(
            encoded == bytes,
            "{}: re-encoded bytes differ",
            file.display()
        );
    }
}

#[test]
fn presentation_fixtures_roundtrip() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../python/tests/fixtures");
    let mut files = Vec::new();
    files_named(
        &root,
        &|p| p.extension().is_some_and(|e| e == "pro"),
        &mut files,
    );
    assert!(files.len() >= 3, "fixtures found: {files:?}");
    roundtrip("rv.data.Presentation", &files);
    println!(
        "round-trip presentations: {}/{} byte-identical",
        files.len(),
        files.len()
    );
}

#[test]
fn theme_fixtures_roundtrip() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../python/tests/fixtures/themes");
    let mut files = Vec::new();
    files_named(
        &root,
        &|p| p.file_name().is_some_and(|n| n == "Theme"),
        &mut files,
    );
    assert_eq!(files.len(), 3);
    roundtrip("rv.data.Template.Document", &files);
    println!(
        "round-trip themes: {}/{} byte-identical",
        files.len(),
        files.len()
    );
}

#[test]
fn negative_zero_and_unknown_fields_survive() {
    // background.color.red = -0.0 (upb writes it; a `!= 0.0` test would drop it), and field 99 that the schema does not know
    let bytes = b"B\x07\n\x05\r\x00\x00\x00\x80\x98\x06\x07";
    let decoded = Message::decode_named("rv.data.Presentation", bytes).unwrap();
    assert_eq!(decoded.unknown_len(), 3);
    assert_eq!(decoded.encode(), bytes);
}
