use std::process::Command;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_versevision"))
        .args(args)
        .output()
        .expect("the binary runs")
}

#[test]
fn version_flag_prints_the_workspace_version() {
    let out = run(&["--version"]);
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        format!("versevision {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn anything_else_is_a_usage_error() {
    let out = run(&[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("usage: versevision --version"));
}
