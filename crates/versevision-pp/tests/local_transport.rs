//! The file operations of the local transport and the words it uses when it refuses (`node/transport.py: LocalTransport`).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use versevision_core::config::AppConfig;
use versevision_pp::clock::VirtualClock;
use versevision_pp::errors::Kind;
use versevision_pp::process::FixedProbe;
use versevision_pp::transport::{LocalTransport, Transport};

fn sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../python/tests/fixtures/sample/Sample.pro")
}

fn transport(root: &Path) -> LocalTransport {
    let mut config = AppConfig::default();
    config.node.library_root = root.join("Libraries").to_string_lossy().into_owned();
    config.node.theme_root = root.join("Themes").to_string_lossy().into_owned();
    config.node.backup_dir = root.join("backups").to_string_lossy().into_owned();
    LocalTransport::new(
        &config.propresenter,
        &config.node,
        Arc::new(VirtualClock::new(1_784_419_571, "t")),
        Arc::new(FixedProbe::new(false)),
        root.to_path_buf(),
    )
}

#[test]
fn a_new_file_never_replaces_an_existing_one() {
    let root = tempfile::tempdir().unwrap();
    let library = root.path().join("Libraries/Verses");
    fs::create_dir_all(&library).unwrap();
    let t = transport(root.path());
    let target = library.join("New.pro").to_string_lossy().into_owned();
    let sha = t.create_file(&sample(), &target).unwrap();
    assert_eq!(sha.len(), 64);
    assert_eq!(fs::read(&target).unwrap(), fs::read(sample()).unwrap());
    let again = t.create_file(&sample(), &target).unwrap_err();
    assert_eq!(again.kind, Kind::PresentationWrite);
    assert_eq!(again.message, "New.pro already exists; nothing was changed");
    // no half-written temp file is left behind
    let left: Vec<String> = fs::read_dir(&library)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, vec!["New.pro".to_string()]);
}

#[test]
fn an_install_needs_the_file_to_be_as_it_was_read() {
    let root = tempfile::tempdir().unwrap();
    let library = root.path().join("Libraries/Verses");
    fs::create_dir_all(&library).unwrap();
    let t = transport(root.path());
    let target = library.join("Sample.pro");
    fs::copy(sample(), &target).unwrap();
    let remote = target.to_string_lossy().into_owned();
    let read = root.path().join("read.pro");
    let sha = t.fetch_file(&remote, &read).unwrap();
    let other = root.path().join("other.pro");
    fs::write(&other, b"not a presentation").unwrap();
    let wrong = t.install_file(&other, &remote, "0000").unwrap_err();
    assert_eq!(
        wrong.message,
        "The presentation changed after VerseVision read it; no files changed"
    );
    assert_eq!(fs::read(&target).unwrap(), fs::read(sample()).unwrap());
    let installed = t.install_file(&other, &remote, &sha).unwrap();
    assert_eq!(
        installed,
        versevision_pp::transport::sha256_file(&other).unwrap()
    );
    assert_eq!(fs::read(&target).unwrap(), b"not a presentation");
}

#[test]
fn deleting_what_was_created_checks_what_is_there() {
    let root = tempfile::tempdir().unwrap();
    let library = root.path().join("Libraries/Verses");
    fs::create_dir_all(&library).unwrap();
    let t = transport(root.path());
    let target = library.join("Made.pro").to_string_lossy().into_owned();
    assert_eq!(
        t.delete_created_file(&target, 4).unwrap(),
        "The presentation file was already gone"
    );
    t.create_file(&sample(), &target).unwrap();
    let edited = t.delete_created_file(&target, 99).unwrap_err();
    assert!(
        edited.message.starts_with("Made.pro has "),
        "{}",
        edited.message
    );
    assert!(edited.message.ends_with("slides now, not the 99 VerseVision added, so it was edited. It was not deleted; remove it in ProPresenter if you no longer want it"));
    assert!(Path::new(&target).exists());
}

#[test]
fn a_missing_themes_folder_is_said_so() {
    let root = tempfile::tempdir().unwrap();
    let t = transport(root.path());
    let error = t.fetch_themes(&root.path().join("dest")).unwrap_err();
    assert_eq!(error.kind, Kind::ProPresenterConnection);
    assert!(
        error.message.starts_with("No Themes folder at "),
        "{}",
        error.message
    );
}

#[test]
fn an_unreachable_propresenter_is_a_connection_error() {
    let root = tempfile::tempdir().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut config = AppConfig::default();
    config.propresenter.host = "127.0.0.1".into();
    config.propresenter.port = port;
    config.propresenter.timeout_seconds = 2.0;
    let t = LocalTransport::new(
        &config.propresenter,
        &config.node,
        Arc::new(VirtualClock::new(1_784_419_571, "t")),
        Arc::new(FixedProbe::new(false)),
        root.path().to_path_buf(),
    );
    let error = t.api_get("/version").unwrap_err();
    assert_eq!(error.kind, Kind::ProPresenterConnection);
    assert!(
        error.message.starts_with(&format!(
            "Could not reach ProPresenter at 127.0.0.1:{port}: "
        )),
        "{}",
        error.message
    );
    let put = t
        .api_put("/v1/playlist/x", &serde_json::json!([]))
        .unwrap_err();
    assert!(
        put.message
            .starts_with("ProPresenter did not accept the change to /v1/playlist/x: "),
        "{}",
        put.message
    );
}
