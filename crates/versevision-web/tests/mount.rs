//! The router works standalone at `/` and nested under a prefix inside a host's server (`/versevision/`), because every URL is relative.

mod common;
use common::*;

use axum::http::Method;
use axum::Router;
use serde_json::json;

fn mounted(h: &Harness, prefix: &str) -> Router {
    Router::new()
        .route("/outside", axum::routing::get(|| async { "host" }))
        .merge(versevision_web::mount(prefix, h.state.clone()))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn under_a_prefix_the_page_the_api_and_the_app_files_are_all_there() {
    let h = Harness::new("new");
    let host = mounted(&h, "/versevision");
    assert_eq!(get(&host, "/outside").await.text(), "host");
    let page = get(&host, "/versevision/").await;
    assert_eq!(page.status, 200, "{}", page.text());
    assert!(page.text().contains("VerseVision"));
    assert_eq!(
        get(&host, "/versevision/api/status").await.json()["template_ready"],
        true
    );
    // the files the page and the manifest name, resolved the way a browser resolves them (relative to the mount)
    let manifest = get(&host, "/versevision/manifest.webmanifest").await.json();
    for icon in manifest["icons"].as_array().unwrap() {
        let src = icon["src"].as_str().unwrap();
        let answer = get(&host, &format!("/versevision/{src}")).await;
        assert_eq!(
            (
                answer.status,
                answer.headers["content-type"].to_str().unwrap().to_string()
            ),
            (200, icon["type"].as_str().unwrap().to_string()),
            "{src}"
        );
    }
    for file in [
        "sw.js",
        "icon.svg",
        "ico-new.svg",
        "ico-config.svg",
        "ico-library.svg",
    ] {
        assert_eq!(
            get(&host, &format!("/versevision/{file}")).await.status,
            200,
            "{file}"
        );
    }
    // what the host's own root does not hold is not found there (the page is not leaking out of its prefix)
    assert_eq!(get(&host, "/api/status").await.status, 404);
    assert_eq!(get(&host, "/icon.svg").await.status, 404);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_whole_review_and_import_works_through_the_prefix() {
    let h = Harness::new("new");
    let host = mounted(&h, "/versevision");
    let answer = send(
        &host,
        Method::POST,
        "/versevision/api/plan?version=NIV",
        &[
            ("host", "localhost:3000"),
            ("x-file-name", "n.txt"),
            ("origin", "http://localhost:3000"),
        ],
        NOTES.as_bytes().to_vec(),
    )
    .await;
    assert_eq!(answer.status, 202, "{}", answer.text());
    let id = answer.json()["job"].as_str().unwrap().to_string();
    let job = loop {
        let state = get(&host, &format!("/versevision/api/jobs/{id}"))
            .await
            .json();
        if state["state"] != "running" {
            break state;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    };
    assert_eq!(job["state"], "done", "{job}");
    let apply = send(
        &host,
        Method::POST,
        "/versevision/api/apply",
        &[
            ("host", "localhost:3000"),
            ("origin", "http://localhost:3000"),
        ],
        json!({"job": id, "accept_flags": true})
            .to_string()
            .into_bytes(),
    )
    .await;
    assert_eq!(apply.status, 202, "{}", apply.text());
    let done = finished_under(&host, "/versevision", apply.json()["job"].as_str().unwrap()).await;
    assert_eq!(done["state"], "done", "{done}");
    assert_eq!(
        get(&host, "/versevision/api/history").await.json()[0]["mode"],
        "new"
    );
}

#[tokio::test]
async fn a_deeper_prefix_and_the_page_opened_without_the_trailing_slash() {
    let h = Harness::new("new");
    let host = Router::new().merge(versevision_web::mount("/tools/vv", h.state.clone()));
    assert_eq!(get(&host, "/tools/vv/api/status").await.status, 200);
    assert_eq!(get(&host, "/tools/vv/").await.status, 200);
    // without the slash the page is still served (its first script sets the base so the relative URLs resolve inside the prefix)
    let bare = get(&host, "/tools/vv").await;
    assert_eq!(bare.status, 200);
    assert!(bare
        .text()
        .contains("document.write('<base href=\"'+location.pathname+'/\">')"));
}

#[tokio::test]
async fn standalone_at_the_root() {
    let h = Harness::new("new");
    let router = h.router();
    assert_eq!(get(&router, "/").await.status, 200);
    assert_eq!(get(&router, "/api/status").await.status, 200);
}

#[tokio::test]
async fn plain_nest_serves_the_api_and_the_page_without_the_slash() {
    let h = Harness::new("new");
    let host = Router::new().nest("/versevision", h.router());
    assert_eq!(get(&host, "/versevision/api/status").await.status, 200);
    assert_eq!(
        get(&host, "/versevision/manifest.webmanifest").await.status,
        200
    );
    assert_eq!(get(&host, "/versevision").await.status, 200);
}

#[tokio::test]
async fn the_mounted_page_keeps_the_query_so_a_token_link_works() {
    let h = Harness::with("new", |b| b.token(Some("t0k".into())));
    let host = mounted(&h, "/versevision");
    assert_eq!(get(&host, "/versevision/?token=t0k").await.status, 200);
    assert_eq!(get(&host, "/versevision/api/status").await.status, 401);
    assert_eq!(
        get(&host, "/versevision/api/status?token=t0k").await.status,
        200
    );
}
