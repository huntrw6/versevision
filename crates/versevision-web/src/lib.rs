//! VerseVision's web app as an `axum` router: the page, its JSON API, the Config page with the Bible keys section.
//!
//! [`router`] takes a [`WebState`] and returns a [`Router`] that works standalone (served at `/`, see [`serve`]) and nested under a
//! prefix inside a host's server (`Router::nest("/versevision", router(state))`): every URL in the page, the manifest and the service
//! worker is relative, so nothing needs to know the prefix. A host that nests it should send `/versevision` to `/versevision/` (the page
//! also repairs its own base when it is opened without the slash).
//!
//! The web app is Python's `web/` (`server.py`, `configpage.py`, `index.html`) ported: same paths, JSON shapes, status codes and error
//! texts. What is new is the Bible keys section, the Origin and Host checks, and that the state is injected rather than global.

mod assets;
mod biblekeys;
pub mod configpage;
mod jobs;
mod routes;
mod state;

use std::net::SocketAddr;

pub use axum::Router;
pub use state::{
    Autostart, BibleFactory, BibleHandle, TransportFactory, WebState, WebStateBuilder,
};
pub use versevision_pp::dirs::Dirs;

/// The standalone address: this computer only.
pub const DEFAULT_ADDR: SocketAddr = SocketAddr::new(
    std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)),
    8777,
);

/// The page and its API, to be served at `/` or nested (`Router::nest("/versevision", router(state))`).
///
/// axum's `nest` answers `/versevision` and `/versevision/api/...` but leaves `/versevision/` (with the slash, the address people use) to the
/// host; [`mount`] does the whole job.
pub fn router(state: WebState) -> Router {
    routes::build(state)
}

/// A router that serves the page under `prefix` (`"/versevision"`): the page at `/versevision/` and `/versevision`, the API and the app
/// files below it. Merge it into the host's router (`host.merge(mount("/versevision", state))`) behind the host's own login.
pub fn mount(prefix: &str, state: WebState) -> Router {
    use axum::extract::Request;
    use axum::routing::any;
    use tower::ServiceExt;
    let prefix = format!("/{}", prefix.trim_matches('/'));
    let inner = router(state);
    let page = inner.clone();
    let with_slash = format!("{prefix}/");
    Router::new()
        .route(
            &with_slash,
            any(move |mut request: Request| {
                let inner = page.clone();
                async move {
                    let query = request
                        .uri()
                        .query()
                        .map(|q| format!("?{q}"))
                        .unwrap_or_default();
                    if let Ok(uri) = format!("/{query}").parse() {
                        *request.uri_mut() = uri;
                    }
                    inner
                        .oneshot(request)
                        .await
                        .unwrap_or_else(|never| match never {})
                }
            }),
        )
        .nest(&prefix, inner)
}

/// `save_config(config, overwrite=True)`: the config file written next to a temporary file and renamed over the old one.
pub(crate) fn save_config(
    dirs: &Dirs,
    config: &versevision_core::config::AppConfig,
) -> Result<(), String> {
    let path = dirs.config_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let mut temporary = path.clone().into_os_string();
    temporary.push(".tmp");
    let temporary = std::path::PathBuf::from(temporary);
    std::fs::write(&temporary, config.to_toml_string())
        .and_then(|()| std::fs::rename(&temporary, &path))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Serve the page on `listener` until the future is dropped. Binding is the host's choice; a address that is not loopback needs an
/// access token (`WebStateBuilder::token`), as in Python. On a loopback address only the loopback host names are answered (against DNS
/// rebinding), unless the host set its own list.
pub async fn serve(listener: tokio::net::TcpListener, state: WebState) -> std::io::Result<()> {
    let addr = listener.local_addr()?;
    if !addr.ip().is_loopback() && !state.has_token() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "serving on a network address needs an access token (WebStateBuilder::token)",
        ));
    }
    state.set_listen_addr(addr);
    if addr.ip().is_loopback()
        && state
            .inner
            .allowed_hosts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_none()
    {
        state.set_allowed_hosts(Some(vec![
            "127.0.0.1".into(),
            "localhost".into(),
            "::1".into(),
        ]));
    }
    axum::serve(listener, router(state)).await
}
