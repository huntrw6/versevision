//! Not a test: `VV_PREVIEW_PORT=8890 cargo test -p versevision-web --test preview -- --ignored` serves the page on that port, against the scripted
//! ProPresenter, with one import already in the history and one Bible key saved (not with VV_PREVIEW_NO_KEY=1), so a browser (Playwright) can take its screenshots.

mod common;
use common::*;

use serde_json::json;
use versevision_bible::{KeyStore, Secret};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "serves a page for screenshots until VV_PREVIEW_SECS (default 600) have passed"]
async fn preview_server() {
    let port: u16 = std::env::var("VV_PREVIEW_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8890);
    let seconds: u64 = std::env::var("VV_PREVIEW_SECS")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(600);
    let h = Harness::new("themes-look");
    let router = h.router();
    // VV_PREVIEW_NO_KEY=1 leaves the key out: the page then shows its "Add a Bible key first" card.
    if std::env::var("VV_PREVIEW_NO_KEY").is_err() {
        h.keys
            .set_key("ESV_API_KEY", Secret::new("preview-key-not-real"))
            .unwrap();
    }
    for (name, notes) in [
        ("sermon-1.txt", NOTES),
        (
            "sermon-2.txt",
            "Title\n1) Hope\nJohn 3:17 NIV\nPsalm 23:1 NIV\n",
        ),
    ] {
        let planned = upload(&router, "?version=NIV", name, notes.as_bytes()).await;
        let id = planned.json()["job"].as_str().unwrap().to_string();
        finished(&router, &id).await;
        let applied = post_json(
            &router,
            "/api/apply",
            &json!({"job": id, "accept_flags": true}),
        )
        .await;
        finished(&router, applied.json()["job"].as_str().unwrap()).await;
    }
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .unwrap();
    let serving = tokio::spawn(versevision_web::serve(listener, h.state.clone()));
    tokio::time::sleep(std::time::Duration::from_secs(seconds)).await;
    serving.abort();
}
