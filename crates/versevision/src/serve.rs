//! `versevision serve`: start the local web page. The page itself is S7a's (`versevision-web`, an axum router); this crate does not
//! write a web server. Until that crate is on `main`, [`Serve`] is the one entry point it plugs into, and without an implementation
//! `serve` says so and exits with 1 (Python would start its own server here).

use crate::app::{err, out, Rt};
use crate::def::ServeArgs;
use versevision_pp::errors::VvError;

/// What the web page is started with.
pub struct ServeState {
    pub config: versevision_core::config::AppConfig,
    /// The private access token a non-local address needs (`None` for 127.0.0.1, localhost, ::1).
    pub token: Option<String>,
    pub paths: crate::paths::Paths,
}

/// `serve(addr, state)`: blocks until the server stops. Implemented by the web crate.
pub trait Serve {
    fn serve(&self, host: &str, port: u16, state: ServeState) -> Result<(), String>;
}

/// The web page, when this build has one. Nothing is registered until S7a lands.
pub fn registered() -> Option<&'static dyn Serve> {
    None
}

fn token() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    // 16 random-looking bytes from the clock and the process, enough to keep a LAN guest out; not a cryptographic secret
    let mut x = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos())
        ^ (u128::from(std::process::id()) << 64);
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for _ in 0..22 {
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        out.push(alphabet[((x >> 33) & 63) as usize] as char);
    }
    out
}

pub fn serve(rt: &Rt, a: ServeArgs) -> i32 {
    let config = match crate::runcmds::config_with_node(rt, a.node.as_deref()) {
        Ok(c) => c,
        Err(e) => return crate::cmds::fail_run(rt, &e, false),
    };
    let Some(server) = registered() else {
        let error = VvError::general(
            "The web page is not part of this build yet (it arrives with versevision-web). Nothing was started.",
        );
        return crate::cmds::fail_run(rt, &error, false);
    };
    let loopback = matches!(a.host.as_str(), "127.0.0.1" | "localhost" | "::1");
    let token = if loopback { None } else { Some(token()) };
    let shown = if matches!(a.host.as_str(), "0.0.0.0" | "::") {
        "127.0.0.1"
    } else {
        a.host.as_str()
    };
    let link = format!(
        "http://{shown}:{}/{}",
        a.port,
        token
            .as_ref()
            .map(|t| format!("?token={t}"))
            .unwrap_or_default()
    );
    let console = rt.term.err();
    let mut text = console.print(&format!("[green]VerseVision is running[/green]  {link}"));
    if token.is_some() {
        text.push_str(&console.print("This address is open to your network, so the link above includes a private access token."));
    }
    err(&text);
    let state = ServeState {
        config,
        token,
        paths: rt.paths.clone(),
    };
    let port = u16::try_from(a.port).unwrap_or(8777);
    match server.serve(&a.host, port, state) {
        Ok(()) => {
            err(&console.print("Stopped."));
            0
        }
        Err(message) => {
            out("");
            crate::cmds::fail_run(rt, &VvError::general(message), false)
        }
    }
}
