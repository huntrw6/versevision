//! `versevision serve`, on real sockets: the page answers, a write from another origin is refused, a network address needs (and gets) an
//! access token, a busy port and a bad port fail with exit code 1, and Ctrl-C stops it with `Stopped.` and exit code 0.

use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::Value;
use versevision_cli::paths::Paths;
use versevision_cli::serve::{new_token, run_server, token_for, web_state};
use versevision_core::config::AppConfig;
use versevision_core::VERSEVISION_VERSION;
use versevision_pp::dirs::{DirsEnv, Os};

/// One HTTP/1.1 request with a closed connection; returns `(status, body)`.
fn http(
    addr: SocketAddr,
    request_line: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, String) {
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut text = format!("{request_line} HTTP/1.1\r\nconnection: close\r\n");
    if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("host")) {
        text.push_str(&format!("host: 127.0.0.1:{}\r\n", addr.port()));
    }
    for (name, value) in headers {
        text.push_str(&format!("{name}: {value}\r\n"));
    }
    text.push_str(&format!("content-length: {}\r\n\r\n{body}", body.len()));
    stream.write_all(text.as_bytes()).unwrap();
    let mut answer = String::new();
    let _ = stream.read_to_string(&mut answer);
    let status = answer
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body = answer.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    (status, body)
}

fn paths_in(root: &Path) -> Paths {
    let home = root.join("home");
    fs::create_dir_all(&home).unwrap();
    let env = DirsEnv {
        home: home.clone(),
        xdg_data_home: Some(root.join("data")),
        xdg_config_home: Some(root.join("config")),
        xdg_state_home: Some(root.join("state")),
        local_app_data: Some(root.join("local")),
    };
    Paths::with(Os::current(), &env, &|_| None)
}

#[test]
fn tokens_are_22_url_safe_characters_and_only_a_network_address_gets_one() {
    let a = new_token();
    assert_eq!(a.len(), 22);
    assert!(a
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    assert_ne!(a, new_token());
    for host in ["127.0.0.1", "localhost", "::1"] {
        assert_eq!(token_for(host), None, "{host}");
    }
    for host in ["0.0.0.0", "::", "192.168.1.5", "example.local"] {
        assert_eq!(token_for(host).map(|t| t.len()), Some(22), "{host}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_page_answers_a_write_from_another_origin_is_refused_and_shutdown_is_clean() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths_in(root.path());
    let state = web_state(&paths, AppConfig::default(), None);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(run_server(listener, state, async {
        let _ = stopped.await;
    }));
    let (status, wrong, own, rebound) = tokio::task::spawn_blocking(move || {
        let status = http(addr, "GET /api/status", &[], "");
        let wrong = http(
            addr,
            "POST /api/autostart",
            &[
                ("origin", "http://evil.example"),
                ("content-type", "application/json"),
            ],
            "{\"enabled\": false}",
        );
        let own = http(
            addr,
            "POST /api/autostart",
            &[
                ("origin", &format!("http://127.0.0.1:{}", addr.port())),
                ("content-type", "application/json"),
            ],
            "{\"enabled\": false}",
        );
        let rebound = http(addr, "GET /api/status", &[("host", "evil.example")], "");
        (status, wrong, own, rebound)
    })
    .await
    .unwrap();
    assert_eq!(status.0, 200, "{}", status.1);
    assert_eq!(wrong.0, 403, "a POST with another Origin: {}", wrong.1);
    assert_ne!(own.0, 403, "the page's own Origin is allowed: {}", own.1);
    assert_eq!(rebound.0, 403, "a Host that is not this computer");
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_reports_the_one_version_constant() {
    let root = tempfile::tempdir().unwrap();
    let state = web_state(&paths_in(root.path()), AppConfig::default(), None);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(run_server(listener, state, async {
        let _ = stopped.await;
    }));
    let (code, body) = tokio::task::spawn_blocking(move || http(addr, "GET /api/status", &[], ""))
        .await
        .unwrap();
    assert_eq!(code, 200, "{body}");
    let json: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["version"], VERSEVISION_VERSION);
    assert_eq!(VERSEVISION_VERSION, "1.0.0");
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_network_address_without_a_token_is_refused_and_with_one_the_token_is_required() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths_in(root.path());
    // refused outright: the server does not start
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", 0)).await.unwrap();
    let refused = run_server(
        listener,
        web_state(&paths, AppConfig::default(), None),
        std::future::pending(),
    )
    .await
    .unwrap_err();
    assert_eq!(
        refused.kind(),
        std::io::ErrorKind::InvalidInput,
        "{refused}"
    );
    assert!(refused.to_string().contains("access token"), "{refused}");

    // with the token the command line makes: the API wants it
    let token = token_for("0.0.0.0").unwrap();
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let state = web_state(&paths, AppConfig::default(), Some(token.clone()));
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(run_server(listener, state, async {
        let _ = stopped.await;
    }));
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let (without, with) = tokio::task::spawn_blocking(move || {
        (
            http(addr, "GET /api/status", &[], "").0,
            http(addr, &format!("GET /api/status?token={token}"), &[], "").0,
        )
    })
    .await
    .unwrap();
    assert_eq!(without, 401);
    assert_eq!(with, 200);
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}

// ----------------------------------------------------------------------------------------------- the binary itself

fn binary(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_versevision"));
    let home = root.join("home");
    fs::create_dir_all(&home).unwrap();
    for var in ["COLUMNS", "FORCE_COLOR", "NO_COLOR"] {
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

#[cfg(unix)]
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[cfg(unix)]
fn wait_for(port: u16) {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let started = std::time::Instant::now();
    while TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_err() {
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the server did not start"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn finish(mut child: Child) -> (i32, String) {
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    let code = child.wait().unwrap().code().unwrap_or(-1);
    (code, stderr)
}

fn serve_process(root: &Path, args: &[&str]) -> Child {
    binary(root)
        .arg("serve")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

#[cfg(unix)]
fn interrupt(child: &Child) {
    let status = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
}

#[cfg(unix)]
#[test]
fn ctrl_c_stops_the_server_with_stopped_and_exit_code_0() {
    let root = tempfile::tempdir().unwrap();
    let port = free_port();
    let child = serve_process(root.path(), &["--port", &port.to_string(), "--no-open"]);
    wait_for(port);
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let (code, body) = http(addr, "GET /api/status", &[], "");
    assert_eq!(code, 200, "{body}");
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["version"],
        VERSEVISION_VERSION
    );
    interrupt(&child);
    let (code, stderr) = finish(child);
    assert_eq!(code, 0, "{stderr}");
    assert!(
        stderr.contains(&format!(
            "VerseVision is running  http://127.0.0.1:{port}/\n"
        )),
        "{stderr}"
    );
    assert!(stderr.trim_end().ends_with("Stopped."), "{stderr}");
    assert!(!stderr.contains("token"), "{stderr}");
}

#[cfg(unix)]
#[test]
fn a_network_host_prints_a_link_with_its_token_and_needs_it() {
    let root = tempfile::tempdir().unwrap();
    let port = free_port();
    let mut child = serve_process(
        root.path(),
        &["--host", "0.0.0.0", "--port", &port.to_string()],
    );
    wait_for(port);
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    assert_eq!(http(addr, "GET /api/status", &[], "").0, 401);
    // the link is on the first stderr line; read it once the server is up, then stop it
    interrupt(&child);
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert_eq!(child.wait().unwrap().code(), Some(0), "{stderr}");
    let token = stderr
        .split("?token=")
        .nth(1)
        .and_then(|t| t.split_whitespace().next())
        .unwrap_or_else(|| panic!("no token in {stderr}"))
        .to_string();
    assert_eq!(token.len(), 22, "{stderr}");
    assert!(
        stderr.contains(&format!("http://127.0.0.1:{port}/?token=")),
        "{stderr}"
    );
    assert!(
        stderr.split_whitespace().collect::<Vec<_>>().join(" ").contains("This address is open to your network, so the link above includes a private access token."),
        "{stderr}"
    );
    // a second run: the token that run printed is the one it needs
    let mut child = serve_process(
        root.path(),
        &["--host", "0.0.0.0", "--port", &port.to_string()],
    );
    wait_for(port);
    interrupt(&child);
    let mut again = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut again)
        .unwrap();
    child.wait().unwrap();
    assert_ne!(
        again
            .split("?token=")
            .nth(1)
            .and_then(|t| t.split_whitespace().next()),
        Some(token.as_str()),
        "a new token every start"
    );
}

#[test]
fn a_port_in_use_and_a_port_out_of_range_fail_with_exit_code_1() {
    let root = tempfile::tempdir().unwrap();
    let busy = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = busy.local_addr().unwrap().port();
    let (code, stderr) = finish(serve_process(root.path(), &["--port", &port.to_string()]));
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("Cannot listen on 127.0.0.1"), "{stderr}");
    assert!(!stderr.contains("is running"), "{stderr}");
    let (code, stderr) = finish(serve_process(root.path(), &["--port", "70000"]));
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("port must be 0-65535"), "{stderr}");
}

#[test]
fn a_remote_node_is_refused_before_anything_starts() {
    let root = tempfile::tempdir().unwrap();
    let (code, stderr) = finish(serve_process(root.path(), &["--node", "other"]));
    assert_ne!(code, 0, "{stderr}");
    assert!(!stderr.contains("is running"), "{stderr}");
}
