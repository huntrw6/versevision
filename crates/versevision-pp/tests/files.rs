//! The folder work: backup, atomic replace, lock, a new file that never overwrites. The bytes are the oracle's (`oracle/pro/cases/*`).

use std::fs;
use std::path::{Path, PathBuf};

use versevision_core::config::PresentationTemplateConfig;
use versevision_core::plan::SlidePlan;
use versevision_pp::backup::{create_backup, restore_backup};
use versevision_pp::files::{append_plan_in_place, derive_presentation_file, write_new_file};
use versevision_pp::{PresentationWriter, SeededUuids};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn case(name: &str) -> (SlidePlan, PresentationTemplateConfig, toml::Value, PathBuf) {
    let dir = repo().join("oracle/pro/cases").join(name);
    let options: toml::Value =
        toml::from_str(&fs::read_to_string(dir.join("options.toml")).unwrap()).unwrap();
    let plan = serde_json::from_str(&fs::read_to_string(dir.join("plan.json")).unwrap()).unwrap();
    let definition = options["definition"].clone().try_into().unwrap();
    (plan, definition, options, dir)
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vv-pp-{tag}-{}", uuid::Uuid::new_v4().simple()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn append_in_place_makes_a_backup_and_writes_the_oracle_bytes() {
    let (plan, definition, options, dir) = case("append-sample");
    let work = scratch("append");
    let target = work.join("Sample.pro");
    fs::copy(
        repo().join("tests/fixtures/sample/Sample.pro"),
        &target,
    )
    .unwrap();
    let original = fs::read(&target).unwrap();
    let mut writer =
        PresentationWriter::new(SeededUuids::new(options["uuid_seed"].as_str().unwrap()));
    let done = append_plan_in_place(
        &mut writer,
        &target,
        &plan,
        &definition,
        None,
        None,
        None,
        1_784_419_571,
    )
    .unwrap();
    assert_eq!(
        fs::read(&target).unwrap(),
        fs::read(dir.join("expected.pro")).unwrap()
    );
    assert_eq!(
        fs::read(&done.backup_path).unwrap(),
        original,
        "the backup is the file as it was"
    );
    assert_eq!(
        done.backup_path.file_name().unwrap(),
        "Sample.pro.versevision-backup-20260719-000611"
    );
    assert_eq!(done.result.cue_count_after, 7);
    let leftovers: Vec<_> = fs::read_dir(&work)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "no temporary file is left: {leftovers:?}"
    );
    // put the original back from the backup
    restore_backup(&target, &done.backup_path).unwrap();
    assert_eq!(fs::read(&target).unwrap(), original);
    fs::remove_dir_all(work).unwrap();
}

#[test]
fn a_failed_write_changes_nothing() {
    let (plan, definition, _, _) = case("append-sample");
    let work = scratch("fail");
    let target = work.join("Wrong.pro");
    fs::copy(repo().join("oracle/pro/files/wrong_version.pro"), &target).unwrap();
    let before = fs::read(&target).unwrap();
    let mut writer = PresentationWriter::default();
    let error = append_plan_in_place(
        &mut writer,
        &target,
        &plan,
        &definition,
        None,
        None,
        None,
        0,
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "Unsupported ProPresenter version 19.1.1");
    assert_eq!(fs::read(&target).unwrap(), before);
    let names: Vec<_> = fs::read_dir(&work)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        names
            .iter()
            .all(|n| !n.contains("backup") && !n.ends_with(".tmp")),
        "no backup and no temporary file for a refused write: {names:?}"
    );
    fs::remove_dir_all(work).unwrap();
}

#[test]
fn a_new_presentation_never_overwrites_a_file() {
    let (plan, definition, options, dir) = case("derive-replace-keep-id");
    let work = scratch("derive");
    let source = work.join("Sunday.pro");
    fs::copy(repo().join("oracle/pro/files/service.pro"), &source).unwrap();
    let output = work.join("Sunday 2.pro");
    let mut writer =
        PresentationWriter::new(SeededUuids::new(options["uuid_seed"].as_str().unwrap()));
    let name = options["name"].as_str().unwrap();
    let done = derive_presentation_file(
        &mut writer,
        &source,
        &output,
        &plan,
        &definition,
        None,
        None,
        name,
        true,
        true,
        1_784_419_571,
    )
    .unwrap();
    assert_eq!(
        fs::read(&output).unwrap(),
        fs::read(dir.join("expected.pro")).unwrap()
    );
    assert_eq!(done.path.file_name().unwrap(), "Sunday 2.pro");
    let again = derive_presentation_file(
        &mut writer,
        &source,
        &output,
        &plan,
        &definition,
        None,
        None,
        name,
        true,
        true,
        1_784_419_571,
    )
    .unwrap_err();
    assert_eq!(
        again.to_string(),
        "Sunday 2.pro already exists; a new presentation never overwrites a file"
    );
    let direct = write_new_file(&output, b"x").unwrap_err();
    assert!(direct.to_string().contains("already exists"));
    assert!(fs::read_dir(&work).unwrap().all(|e| !e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .ends_with(".tmp")));
    fs::remove_dir_all(work).unwrap();
}

#[test]
fn backups_are_named_after_the_file_and_never_replace_one() {
    let work = scratch("backup");
    let file = work.join("A.pro");
    fs::write(&file, b"1").unwrap();
    let first = create_backup(&file, 0).unwrap();
    let second = create_backup(&file, 0).unwrap();
    assert_eq!(
        first.file_name().unwrap(),
        "A.pro.versevision-backup-19700101-000000"
    );
    assert_eq!(
        second.file_name().unwrap(),
        "A.pro.versevision-backup-19700101-000000-1"
    );
    fs::remove_dir_all(work).unwrap();
}
