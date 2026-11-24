//! The key store: the Python keys file format (differential), write-only handling, private file permissions, the environment override.

mod common;

use std::fs;

use common::*;
use serde_json::Value;
use versevision_bible::{FileKeyStore, KeyError, KeyStore, MemoryKeyStore, Secret};

#[test]
fn keys_file_reading_is_python_from_keys_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("keys.env");
    let store = FileKeyStore::new(&path);
    let mut total = 0;
    for (i, row) in jsonl("keys.jsonl").iter().enumerate() {
        fs::write(&path, row["content"].as_str().unwrap().as_bytes()).unwrap();
        for (name, expected) in row["values"].as_object().unwrap() {
            let actual = store.resolve(name);
            assert_eq!(
                actual.as_ref().map(Secret::expose),
                expected.as_str(),
                "keys.jsonl:{} name {name} content {:?}",
                i + 1,
                row["content"]
            );
            assert_eq!(store.has_key(name), !matches!(expected, Value::Null));
            total += 1;
        }
    }
    println!("oracle/providers/keys.jsonl: {total}/{total} identical");
}

#[test]
fn set_has_remove_round_trip_keeps_other_lines() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("nested").join("keys.env");
    let store = FileKeyStore::new(&path);
    assert!(!store.has_key("VV_RT_ONE"));
    store.set_key("VV_RT_ONE", Secret::new("first")).unwrap();
    store.set_key("VV_RT_TWO", Secret::new("second")).unwrap();
    assert!(store.has_key("VV_RT_ONE") && store.has_key("VV_RT_TWO"));
    // replaced, not duplicated
    store.set_key("VV_RT_ONE", Secret::new("changed")).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert_eq!(text, "VV_RT_ONE=changed\nVV_RT_TWO=second\n");
    assert_eq!(store.resolve("VV_RT_ONE").unwrap().expose(), "changed");
    // an existing file's comments and other lines stay
    fs::write(
        &path,
        "# my keys\nOTHER=keep\nVV_RT_ONE=old\n\nVV_RT_ONE=older\n",
    )
    .unwrap();
    store.set_key("VV_RT_ONE", Secret::new("new")).unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "# my keys\nOTHER=keep\nVV_RT_ONE=new\n\n"
    );
    assert!(store.remove_key("VV_RT_ONE").unwrap());
    assert!(!store.remove_key("VV_RT_ONE").unwrap());
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "# my keys\nOTHER=keep\n\n"
    );
    assert!(!store.has_key("VV_RT_ONE"));
    assert!(store.has_key("OTHER"));
    // no temporary file is left behind
    let names: Vec<_> = fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(names, vec!["keys.env".to_string()]);
}

#[test]
fn bad_names_and_values_are_refused_without_echoing_the_value() {
    let directory = tempfile::tempdir().unwrap();
    let store = FileKeyStore::new(directory.path().join("keys.env"));
    for name in ["", "1ABC", "A B", "A=B", "A\nB", "naïve"] {
        let error = store.set_key(name, Secret::new("v")).unwrap_err();
        assert!(matches!(error, KeyError::InvalidName(_)), "{name:?}");
    }
    for value in [
        "",
        " lead",
        "trail ",
        "two\nlines",
        "a\rb",
        "\"quoted\"",
        "'q'",
        "nul\0",
    ] {
        let error = store
            .set_key("VV_BAD_VALUE", Secret::new(value))
            .unwrap_err();
        assert!(matches!(error, KeyError::InvalidValue), "{value:?}");
        assert!(!format!("{error} {error:?}").contains(value.trim()) || value.trim().is_empty());
    }
    assert!(!directory.path().join("keys.env").exists());
}

#[test]
fn an_environment_variable_wins_over_the_file_and_is_never_removed() {
    let directory = tempfile::tempdir().unwrap();
    let store = FileKeyStore::new(directory.path().join("keys.env"));
    store
        .set_key("VV_ENV_WINS", Secret::new("from-file"))
        .unwrap();
    assert_eq!(store.resolve("VV_ENV_WINS").unwrap().expose(), "from-file");
    std::env::set_var("VV_ENV_WINS", "from-environment");
    assert_eq!(
        store.resolve("VV_ENV_WINS").unwrap().expose(),
        "from-environment"
    );
    assert!(store.has_key("VV_ENV_WINS"));
    // removing takes the stored key out, the environment still answers
    assert!(store.remove_key("VV_ENV_WINS").unwrap());
    assert_eq!(
        store.resolve("VV_ENV_WINS").unwrap().expose(),
        "from-environment"
    );
    std::env::remove_var("VV_ENV_WINS");
    assert!(!store.has_key("VV_ENV_WINS"));
    // an empty variable is not a key (`os.getenv(name)` is falsy)
    std::env::set_var("VV_ENV_EMPTY", "");
    assert!(!store.has_key("VV_ENV_EMPTY"));
    std::env::remove_var("VV_ENV_EMPTY");
    // the in-memory store has the same rule
    let memory = MemoryKeyStore::new();
    memory.set_key("VV_ENV_MEM", Secret::new("stored")).unwrap();
    std::env::set_var("VV_ENV_MEM", "environment");
    assert_eq!(
        memory.resolve("VV_ENV_MEM").unwrap().expose(),
        "environment"
    );
    std::env::remove_var("VV_ENV_MEM");
    assert_eq!(memory.resolve("VV_ENV_MEM").unwrap().expose(), "stored");
    assert!(memory.remove_key("VV_ENV_MEM").unwrap());
    assert!(!memory.has_key("VV_ENV_MEM"));
}

#[cfg(unix)]
#[test]
fn the_keys_file_is_private_from_the_first_byte() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let dir = directory.path().join("config").join("versevision");
    let path = dir.join("keys.env");
    let store = FileKeyStore::new(&path);
    store.set_key("VV_PERM", Secret::new("v1")).unwrap();
    let mode = |p: &std::path::Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(&dir), 0o700, "a folder this code makes is owner-only");
    // an existing loose file is replaced by a private one
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    store.set_key("VV_PERM", Secret::new("v2")).unwrap();
    assert_eq!(mode(&path), 0o600);
    store.remove_key("VV_PERM").unwrap();
    assert_eq!(mode(&path), 0o600);
}

#[cfg(windows)]
#[test]
fn the_keys_file_has_a_private_acl() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("keys.env");
    let store = FileKeyStore::new(&path);
    store.set_key("VV_ACL", Secret::new("v1")).unwrap();
    store.set_key("VV_ACL", Secret::new("v2")).unwrap();
    let output = std::process::Command::new("icacls")
        .arg(&path)
        .output()
        .unwrap();
    let listing = String::from_utf8_lossy(&output.stdout).to_string();
    let user = std::env::var("USERNAME").unwrap();
    assert!(listing.contains(&user), "the user is in the ACL: {listing}");
    // nothing is inherited from the folder, and nobody but the user (and the accounts every file of the user has: the system
    // and the administrators, which can take the file anyway) is in it
    assert!(!listing.contains("(I)"), "no inherited entry: {listing}");
    for other in [
        "Everyone",
        "BUILTIN\\Users",
        "Authenticated Users",
        "Domain Users",
        "CREATOR OWNER",
    ] {
        assert!(
            !listing.contains(other),
            "{other} must not be in the ACL: {listing}"
        );
    }
    let entries = listing.matches(":(").count();
    assert!(entries <= 3, "{entries} entries: {listing}");
    assert_eq!(store.resolve("VV_ACL").unwrap().expose(), "v2");
}

#[test]
fn concurrent_writers_do_not_lose_keys_or_leave_partial_files() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("keys.env");
    let store = std::sync::Arc::new(FileKeyStore::new(&path));
    let handles: Vec<_> = (0..8)
        .map(|n| {
            let store = store.clone();
            std::thread::spawn(move || {
                for round in 0..10 {
                    store
                        .set_key(
                            &format!("VV_CONC_{n}"),
                            Secret::new(format!("value-{n}-{round}")),
                        )
                        .unwrap();
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    for n in 0..8 {
        assert_eq!(
            store.resolve(&format!("VV_CONC_{n}")).unwrap().expose(),
            format!("value-{n}-9")
        );
    }
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}
