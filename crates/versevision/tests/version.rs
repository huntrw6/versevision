use std::process::Command;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_versevision"))
        .args(args)
        .env_remove("COLUMNS")
        .output()
        .expect("the binary runs")
}

#[test]
fn version_flag_prints_the_name_and_the_workspace_version() {
    let out = run(&["--version"]);
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("VerseVision {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn no_arguments_show_the_help_and_exit_with_2_like_the_python_app() {
    let out = run(&[]);
    assert_eq!(out.status.code(), Some(2));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("Usage: versevision [OPTIONS] COMMAND [ARGS]..."),
        "{text}"
    );
    assert!(text.contains("─ Commands ─"), "{text}");
}

#[test]
fn a_misspelt_command_is_a_usage_error_with_a_suggestion() {
    let out = run(&["histry"]);
    assert_eq!(out.status.code(), Some(2));
    let text = String::from_utf8_lossy(&out.stderr);
    assert!(
        text.contains("No such command 'histry'. Did you mean 'history'?"),
        "{text}"
    );
}

#[test]
fn colour_only_when_the_output_is_a_terminal_or_forced() {
    let plain = run(&["versions", "list"]);
    assert!(!String::from_utf8_lossy(&plain.stdout).contains('\u{1b}'));
    let forced = Command::new(env!("CARGO_BIN_EXE_versevision"))
        .args(["versions", "list"])
        .env("FORCE_COLOR", "1")
        .env_remove("NO_COLOR")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&forced.stdout).contains('\u{1b}'));
    let off = Command::new(env!("CARGO_BIN_EXE_versevision"))
        .args(["versions", "list"])
        .env("FORCE_COLOR", "1")
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    assert!(!String::from_utf8_lossy(&off.stdout).contains('\u{1b}'));
}
