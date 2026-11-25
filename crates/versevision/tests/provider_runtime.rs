//! A command that reaches a Bible provider must not panic. The providers use `reqwest`, which needs a Tokio runtime around the future;
//! the CLI once polled them with a plain executor and every real `run` / `import` died with "there is no reactor running" (the offline
//! tests use in-memory providers and could not see it). This runs the real binary with keys set for every provider, so a provider is
//! really called: it comes back as a clean error (no network, or a rejected key), never a panic.

use std::process::Command;

fn run(home: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_versevision"))
        .args(args)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("APPDATA", home)
        .env("LOCALAPPDATA", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("API_BIBLE_API_KEY", "test-key-not-real")
        .env("YOUVERSION_API_KEY", "test-key-not-real")
        .env("ESV_API_KEY", "test-key-not-real")
        .output()
        .expect("the binary runs")
}

fn assert_no_panic(label: &str, out: &std::process::Output) {
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_ne!(out.status.code(), Some(101), "{label} panicked: {text}");
    assert!(
        !text.contains("panicked") && !text.contains("no reactor"),
        "{label}: {text}"
    );
}

#[test]
fn import_and_run_reach_the_providers_without_panicking() {
    let home = std::env::temp_dir().join(format!("versevision-runtime-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    let notes = home.join("notes.txt");
    std::fs::write(&notes, "Title\n1) Faith\nJohn 3:16 NIV\nRomans 8:28 ESV\n").unwrap();
    let notes = notes.to_string_lossy().into_owned();
    assert_no_panic(
        "import --text",
        &run(
            &home,
            &["import", "--text", "John 3:16 NIV", "--dry-run", "--yes"],
        ),
    );
    assert_no_panic(
        "run --dry-run",
        &run(
            &home,
            &["run", &notes, "--dry-run", "--yes", "--output", "json"],
        ),
    );
    assert_no_panic("providers test", &run(&home, &["providers", "test"]));
    let _ = std::fs::remove_dir_all(&home);
}
