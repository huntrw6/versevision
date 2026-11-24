//! `versevision keys`: write-only, and nothing ever prints a key. The canary test puts a recognisable key through every way in
//! (`keys set` from standard input, the environment) and looks for it in everything the program wrote: standard output, standard
//! error, the run folders, the history, the config and the audit log.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const CANARY: &str = "CANARY-9f3c1e7a5b-do-not-print";

fn binary(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_versevision"));
    let home = root.join("home");
    fs::create_dir_all(&home).unwrap();
    for var in [
        "API_BIBLE_API_KEY",
        "YOUVERSION_API_KEY",
        "ESV_API_KEY",
        "COLUMNS",
        "FORCE_COLOR",
        "NO_COLOR",
    ] {
        command.env_remove(var);
    }
    command
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("VERSEVISION_CONFIG", root.join("config.toml"))
        .env("VERSEVISION_HISTORY", root.join("imports.json"))
        .env("VERSEVISION_LOGS", root.join("logs"))
        .env("VERSEVISION_KEYS", root.join("keys.env"))
        .current_dir(root);
    command
}

fn run(
    root: &Path,
    args: &[&str],
    stdin: &str,
    extra_env: &[(&str, &str)],
) -> (i32, String, String) {
    let mut command = binary(root);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in extra_env {
        command.env(k, v);
    }
    let mut child = command.spawn().unwrap();
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).ok();
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files_under(&path, out);
            } else {
                out.push(path);
            }
        }
    }
}

#[test]
fn a_key_is_stored_from_standard_input_and_status_says_set_or_not_set() {
    let root = tempfile::tempdir().unwrap();
    let (code, out, err) = run(root.path(), &["keys", "status"], "", &[]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("not set") && !out.contains("│ set"), "{out}");
    let (code, out, err) = run(
        root.path(),
        &["keys", "set", "esv"],
        &format!("{CANARY}\n"),
        &[],
    );
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.trim(), "Stored the key for esv.");
    let (_, out, _) = run(
        root.path(),
        &["keys", "status", "--output", "json"],
        "",
        &[],
    );
    let rows: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(rows[2], serde_json::json!({"provider": "esv", "set": true}));
    assert_eq!(
        rows[0],
        serde_json::json!({"provider": "api_bible", "set": false})
    );
    // the key is in the keys file (owner-only on Unix) and nowhere else
    let keys_file = root.path().join("keys.env");
    assert!(fs::read_to_string(&keys_file).unwrap().contains(CANARY));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&keys_file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let (code, out, _) = run(root.path(), &["keys", "remove", "esv"], "", &[]);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "Removed the stored key for esv.");
    assert!(!fs::read_to_string(&keys_file)
        .unwrap_or_default()
        .contains(CANARY));
}

#[test]
fn a_key_can_never_be_an_argument() {
    let root = tempfile::tempdir().unwrap();
    let (code, _, err) = run(root.path(), &["keys", "set", "esv", CANARY], "", &[]);
    assert_eq!(code, 2);
    assert!(
        !err.contains(CANARY) || err.contains("unexpected extra argument"),
        "{err}"
    );
    assert!(!root.path().join("keys.env").exists());
    // nothing on standard input is an error, not a hang
    let (code, _, err) = run(root.path(), &["keys", "set", "esv"], "", &[]);
    assert_eq!(code, 2, "{err}");
    assert!(!root.path().join("keys.env").exists());
}

#[test]
fn unknown_providers_are_refused() {
    let root = tempfile::tempdir().unwrap();
    let (code, out, _) = run(root.path(), &["keys", "set", "bibleworks"], "x\n", &[]);
    assert_eq!(code, 2);
    assert!(out.contains("Unknown provider 'bibleworks'"));
}

#[test]
fn the_canary_is_in_nothing_the_program_prints_or_writes() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("notes.txt"),
        "Title 11/8/26\n\n1) ONE\nJohn 3:16 NIV\n",
    )
    .unwrap();
    let mut seen: Vec<(String, String)> = Vec::new();
    let mut note = |label: &str, (code, out, err): (i32, String, String)| {
        seen.push((format!("{label} stdout"), out));
        seen.push((format!("{label} stderr"), err));
        code
    };
    let env = [
        ("ESV_API_KEY", CANARY),
        ("API_BIBLE_API_KEY", CANARY),
        ("YOUVERSION_API_KEY", CANARY),
    ];
    note(
        "keys set",
        run(
            root.path(),
            &["keys", "set", "esv"],
            &format!("{CANARY}\n"),
            &[],
        ),
    );
    note(
        "keys status",
        run(root.path(), &["keys", "status"], "", &env),
    );
    note(
        "keys status json",
        run(
            root.path(),
            &["keys", "status", "--output", "json"],
            "",
            &env,
        ),
    );
    note(
        "config init",
        run(root.path(), &["config", "init"], "", &env),
    );
    note(
        "config show",
        run(root.path(), &["config", "show"], "", &env),
    );
    note(
        "config show json",
        run(
            root.path(),
            &["config", "show", "--output", "json"],
            "",
            &env,
        ),
    );
    note(
        "providers test",
        run(root.path(), &["providers", "test"], "", &env),
    );
    note(
        "run dry",
        run(root.path(), &["run", "notes.txt", "--dry-run"], "", &env),
    );
    note(
        "run dry json",
        run(
            root.path(),
            &["run", "notes.txt", "--dry-run", "--output", "json"],
            "",
            &env,
        ),
    );
    note(
        "run no file",
        run(root.path(), &["run", "missing.txt"], "", &env),
    );
    note("history", run(root.path(), &["history"], "", &env));
    note("log list", run(root.path(), &["log", "list"], "", &env));
    note("explain", run(root.path(), &["explain"], "", &env));
    note("help", run(root.path(), &["keys", "--help"], "", &env));
    for (label, text) in &seen {
        assert!(
            !text.contains(CANARY),
            "the key was printed by {label}:\n{text}"
        );
    }
    let mut files = Vec::new();
    files_under(root.path(), &mut files);
    for file in files {
        if file.file_name().is_some_and(|n| n == "keys.env") {
            continue; // the one place a key lives
        }
        let bytes = fs::read(&file).unwrap();
        assert!(
            !String::from_utf8_lossy(&bytes).contains(CANARY),
            "the key is in {}",
            file.display()
        );
    }
}

#[test]
fn keys_help_is_stable() {
    let root = tempfile::tempdir().unwrap();
    for (name, args) in [
        ("keys", vec!["keys", "--help"]),
        ("keys-set", vec!["keys", "set", "--help"]),
        ("keys-remove", vec!["keys", "remove", "--help"]),
        ("keys-status", vec!["keys", "status", "--help"]),
    ] {
        let (code, out, _) = run(root.path(), &args, "", &[]);
        assert_eq!(code, 0);
        let file = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/snapshots")
            .join(format!("help-{name}.txt"));
        if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(&file, &out).unwrap();
        }
        let want = fs::read_to_string(&file).unwrap_or_else(|_| {
            panic!(
                "no snapshot {}; run with UPDATE_SNAPSHOTS=1",
                file.display()
            )
        });
        // a Windows checkout may turn the snapshot's line ends into CRLF
        assert_eq!(out, want.replace("\r\n", "\n"), "help of {name}");
    }
}
