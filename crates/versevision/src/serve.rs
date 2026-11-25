//! `versevision serve`: start the local web page (`cli_run.py` `serve`, `web/server.py` `serve`).
//!
//! The page and its API are `versevision-web`'s axum router; this module owns what the command line decides: the address, the access
//! token a network address needs, the key store, the cache and the folders, the start-at-login switch, the opening of a browser and the
//! way the server stops (Ctrl-C, and SIGTERM on Unix: `Stopped.`, exit code 0).

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use versevision_bible::{FileKeyStore, ScriptureCache};
use versevision_core::config::AppConfig;
use versevision_pp::errors::VvError;
use versevision_web::{Autostart, Dirs, WebState};

use crate::app::{err, Rt, VERSION};
use crate::autostart::SystemRunner;
use crate::def::ServeArgs;
use crate::paths::Paths;

/// The addresses that keep the page on this computer (Python's `{"127.0.0.1", "localhost", "::1"}`); any other host is open to the
/// network and gets an access token.
pub fn is_loopback_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

/// `secrets.token_urlsafe(16)`: 16 random bytes (a version 4 UUID's) as 22 characters of the URL-safe alphabet.
pub fn new_token() -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let bytes = *uuid::Uuid::new_v4().as_bytes();
    let mut out = String::new();
    let mut bits = 0u32;
    let mut count = 0;
    for byte in bytes {
        bits = (bits << 8) | u32::from(byte);
        count += 8;
        while count >= 6 {
            count -= 6;
            out.push(ALPHABET[((bits >> count) & 63) as usize] as char);
        }
        bits &= (1 << count) - 1;
    }
    if count > 0 {
        out.push(ALPHABET[((bits << (6 - count)) & 63) as usize] as char);
    }
    out
}

/// The token for `host`: none on this computer, a new one for a network address.
pub fn token_for(host: &str) -> Option<String> {
    (!is_loopback_host(host)).then(new_token)
}

/// The start-at-login switch of the page, backed by the same code as `versevision autostart` (a launch agent, a systemd user unit, a
/// scheduled task). The page is already running, so enabling only sets up the login item and disabling leaves this page alone.
pub struct HostAutostart {
    paths: Paths,
}

impl HostAutostart {
    pub fn new(paths: Paths) -> Self {
        Self { paths }
    }
}

impl Autostart for HostAutostart {
    fn status(&self) -> Result<Value, String> {
        let runner = SystemRunner;
        Ok(crate::cmds::autostart_setup_for(&self.paths, &runner)
            .status()
            .to_value())
    }

    fn enable(&self, host: &str, port: u16) -> Result<Value, String> {
        let runner = SystemRunner;
        crate::cmds::autostart_setup_for(&self.paths, &runner)
            .enable(host, i64::from(port), false)
            .map(|s| s.to_value())
    }

    fn disable(&self) -> Result<Value, String> {
        let runner = SystemRunner;
        crate::cmds::autostart_setup_for(&self.paths, &runner)
            .disable(true)
            .map(|s| s.to_value())
    }
}

/// The web app's state for this command line: the folders, the key store and the scripture cache the other commands use, the version
/// constant, and the autostart above. `token` is what a page not on this computer must present.
pub fn web_state(paths: &Paths, config: AppConfig, token: Option<String>) -> WebState {
    let dirs: Dirs = paths.pipeline_dirs();
    WebState::builder(dirs, config)
        .token(token)
        .app_version(VERSION)
        .home(paths.home.clone())
        .keys(Arc::new(FileKeyStore::new(paths.keys_file.clone())))
        .cache(Arc::new(ScriptureCache::new(
            paths.cache_dir.join("scripture"),
        )))
        .autostart(Arc::new(HostAutostart::new(paths.clone())))
        .build()
}

/// Serve `state` on `listener` until `shutdown` is ready. A network address without a token is refused (`versevision-web` checks).
pub async fn run_server(
    listener: tokio::net::TcpListener,
    state: WebState,
    shutdown: impl Future<Output = ()>,
) -> std::io::Result<()> {
    tokio::select! {
        result = versevision_web::serve(listener, state) => result,
        () = shutdown => Ok(()),
    }
}

/// Ctrl-C (and SIGTERM where there is one).
pub async fn stop_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut term) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

/// `webbrowser.open(link)`: best effort, silent when there is no browser.
fn open_browser(link: &str) {
    #[cfg(target_os = "macos")]
    let command = std::process::Command::new("open").arg(link).spawn();
    #[cfg(target_os = "windows")]
    let command = std::process::Command::new("cmd")
        .args(["/C", "start", "", link])
        .spawn();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let command = std::process::Command::new("xdg-open").arg(link).spawn();
    let _ = command;
}

fn failure(rt: &Rt, message: String) -> i32 {
    crate::cmds::fail_run(rt, &VvError::general(message), false)
}

pub fn serve(rt: &Rt, a: ServeArgs) -> i32 {
    let config = match crate::runcmds::config_with_node(rt, a.node.as_deref()) {
        Ok(c) => c,
        Err(e) => return crate::cmds::fail_run(rt, &e, false),
    };
    let Ok(port) = u16::try_from(a.port) else {
        return failure(rt, "bind(): port must be 0-65535.".to_string());
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => return failure(rt, format!("Cannot start the web server: {e}")),
    };
    // bind first: the address in use is reported before anything says the page is running
    let listener = match runtime.block_on(tokio::net::TcpListener::bind((a.host.as_str(), port))) {
        Ok(listener) => listener,
        Err(e) => return failure(rt, format!("Cannot listen on {}:{port}: {e}", a.host)),
    };
    let bound: SocketAddr = match listener.local_addr() {
        Ok(addr) => addr,
        Err(e) => return failure(rt, format!("Cannot listen on {}:{port}: {e}", a.host)),
    };
    let token = token_for(&a.host);
    let shown = if matches!(a.host.as_str(), "0.0.0.0" | "::") {
        "127.0.0.1"
    } else {
        a.host.as_str()
    };
    let link = format!(
        "http://{shown}:{}/{}",
        bound.port(),
        token
            .as_ref()
            .map(|t| format!("?token={t}"))
            .unwrap_or_default()
    );
    let console = rt.term.err();
    let mut text = console.print(&format!("[green]VerseVision is running[/green]  {link}"));
    if token.is_some() {
        text.push_str(&console.print(
            "This address is open to your network, so the link above includes a private access token.",
        ));
    }
    err(&text);
    if a.open && !a.no_open {
        open_browser(&link);
    }
    let state = web_state(&rt.paths, config, token);
    let result = runtime.block_on(run_server(listener, state, stop_signal()));
    // a job that is still running gets a moment, not forever
    runtime.shutdown_timeout(Duration::from_secs(2));
    match result {
        Ok(()) => {
            err(&console.print("Stopped."));
            0
        }
        Err(e) => failure(rt, e.to_string()),
    }
}
