//! The page, its icons, the manifest and the service worker: what `test_web_review.py` and `test_config_page.py` check of them, and that
//! every URL in them is relative so the router works under a prefix.

mod common;
use common::*;

use axum::Router;
use serde_json::{json, Value};

fn router() -> (Harness, Router) {
    let h = Harness::new("new");
    let r = h.router();
    (h, r)
}

#[tokio::test]
async fn the_icon_is_served_and_only_the_action_has_a_description() {
    let (_h, router) = router();
    let icon = get(&router, "/icon.svg").await;
    assert_eq!(icon.headers["content-type"], "image/svg+xml");
    assert!(icon.text().contains("<svg"));
    assert_eq!(
        get(&router, "/favicon.ico").await.headers["content-type"],
        "image/svg+xml"
    );
    let page = get(&router, "/").await.text();
    assert!(page.contains("rel=\"icon\"") && page.contains("id=\"modeHelp\""));
    assert_eq!(page.matches("class=\"help\"").count(), 1);
}

#[tokio::test]
async fn the_page_is_an_installable_app_with_relative_urls() {
    let (_h, router) = router();
    let manifest: Value = get(&router, "/manifest.webmanifest").await.json();
    assert_eq!(
        (manifest["display"].as_str(), manifest["name"].as_str()),
        (Some("standalone"), Some("VerseVision"))
    );
    assert_eq!(
        (
            manifest["start_url"].as_str(),
            manifest["scope"].as_str(),
            manifest["id"].as_str()
        ),
        (Some("./"), Some("./"), Some("./"))
    );
    for icon in manifest["icons"].as_array().unwrap() {
        let src = icon["src"].as_str().unwrap();
        assert!(
            !src.starts_with('/') && !src.contains("://"),
            "{src} must be relative"
        );
        let answer = get(&router, &format!("/{src}")).await;
        assert_eq!(
            answer.headers["content-type"],
            icon["type"].as_str().unwrap()
        );
        assert!(answer.body.len() > 200);
    }
    let sizes: Vec<&str> = manifest["icons"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["sizes"].as_str().unwrap())
        .collect();
    assert!(sizes.contains(&"192x192") && sizes.contains(&"512x512"));
    assert!(manifest["icons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["purpose"] == "maskable"));
    let sw = get(&router, "/sw.js").await;
    assert_eq!(sw.headers["content-type"], "text/javascript");
    assert!(!sw.text().contains("serviceWorker"));
    let page = get(&router, "/").await.text();
    assert!(page.contains("rel=\"manifest\"") && page.contains("rel=\"apple-touch-icon\""));
    assert!(page.contains("serviceWorker.register(\"sw.js\", {scope:\"./\"})"));
}

/// Every place a URL can be written in the page, the service worker and the manifest.
fn urls(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for marker in [
        "href=\"",
        "src=\"",
        "url(",
        "fetch(",
        "api('",
        "api(\"",
        "register(\"",
        "register('",
        "'/",
        "\"/",
    ] {
        let mut rest = text;
        while let Some(at) = rest.find(marker) {
            let after = &rest[at + marker.len()..];
            let end = after.find(['"', '\'', ')', '`']).unwrap_or(after.len());
            let found = if marker.ends_with('/') {
                format!("/{}", &after[..end])
            } else {
                after[..end].to_string()
            };
            out.push(found);
            rest = &rest[at + marker.len()..];
        }
    }
    out
}

#[tokio::test]
async fn the_page_has_no_absolute_url() {
    let (_h, router) = router();
    for (name, text) in [
        ("index", get(&router, "/").await.text()),
        ("sw.js", get(&router, "/sw.js").await.text()),
        (
            "manifest",
            get(&router, "/manifest.webmanifest").await.text(),
        ),
    ] {
        for found in urls(&text) {
            // a path that starts with a slash is absolute; so is anything with a scheme (but the page's own `location.origin` text is not a URL)
            let absolute_path = found.starts_with('/')
                && found[1..].starts_with(|c: char| c.is_ascii_alphabetic() || c == '.');
            assert!(!absolute_path, "{name}: absolute URL {found:?}");
            assert!(
                !found.starts_with("http://") && !found.starts_with("https://"),
                "{name}: {found:?}"
            );
        }
        assert!(
            !text.contains("\"/api") && !text.contains("'/api") && !text.contains("(/ico"),
            "{name} has an absolute /api or icon URL"
        );
    }
    // and every call of the API in the page is the relative `api/...`
    let page = get(&router, "/").await.text();
    assert!(page.contains("api('api/status')") && page.contains("'api/themethumb?theme='"));
}

#[tokio::test]
async fn the_toolbar_has_a_config_button_with_its_own_icon_and_the_bible_keys_section() {
    let (_h, router) = router();
    let page = get(&router, "/").await.text();
    assert!(
        page.contains("id=\"tab-config\"")
            && page.contains(">Config<")
            && page.contains("id=\"viewConfig\"")
    );
    assert!(get(&router, "/ico-config.svg")
        .await
        .text()
        .starts_with("<svg"));
    assert!(
        page.contains("Bible keys")
            && page.contains("api/bible/keys/test")
            && page.contains("api/bible/source")
    );
    // a key field is write-only: a password field that is cleared once saved
    assert!(page.contains("input.type='password'") && page.contains("input.value=''"));
}

#[tokio::test]
async fn the_sidebar_and_the_titles_use_the_pixel_icons() {
    let (_h, router) = router();
    for name in ["ico-library", "ico-playlist", "ico-presentation"] {
        let body = get(&router, &format!("/{name}.svg")).await.text();
        assert!(body.starts_with("<svg") && body.contains("<rect"), "{name}");
    }
    for name in ["new", "replace", "append", "history"] {
        let body = get(&router, &format!("/ico-{name}.svg")).await.text();
        assert!(
            body.starts_with("<svg") && body.contains("viewBox=\"0 0 8 8\""),
            "{name}"
        );
    }
    let page = get(&router, "/").await.text();
    assert!(page.contains("pi lib") && page.contains("pi pl") && page.contains("pi pres"));
    assert!(
        page.contains("id=\"ttlIcon\"")
            && page.contains("titleIcon(m)")
            && page.contains("titleIcon('history')")
    );
}

#[tokio::test]
async fn the_sidebar_marks_what_is_chosen_in_orange_and_blue_without_a_hint_line() {
    // Python's test of the same name: New mode = Library and Playlists titles orange and their selected row blue; Replace and Append
    // (`aside[data-pick]`) = the Presentations title orange and the picked presentation blue; bigger titles; no hint line.
    let (_h, router) = router();
    let page = get(&router, "/").await.text();
    assert!(
        page.contains("class=\"sec s-nav\">Library<")
            && page.contains("class=\"sec s-nav\" id=\"plSec\">Playlists<")
    );
    assert!(page.contains("class=\"sec s-pres\">Presentations<"));
    assert!(page
        .contains("aside:not([data-pick]) .s-nav,aside[data-pick] .s-pres{color:var(--orange)}"));
    assert!(page.contains("aside:not([data-pick]) #libs .row.sel,aside:not([data-pick]) #plists .row.sel,aside[data-pick] #pres .row.sel{background:var(--sel-blue)"));
    assert!(page.contains("document.querySelector('aside').toggleAttribute('data-pick', picking)"));
    assert!(page.contains("aside .sec{font-size:17px"));
    assert!(!page.contains("Select a Library or Presentation") && !page.contains("side-note"));
}

#[tokio::test]
async fn the_drop_box_is_closed_until_there_is_a_way_to_look_verses_up() {
    // The behaviour (card instead of the drop box, a button to the Config page with the Bible keys section open, drag and file refusals)
    // was driven in a real browser against `tests/preview.rs` with VV_PREVIEW_NO_KEY=1; here the page's wiring is pinned.
    let (_h, router) = router();
    let page = get(&router, "/").await.text();
    assert!(
        page.contains("id=\"needKey\"")
            && page.contains("id=\"needKeyBtn\"")
            && page.contains("Add a Bible key first")
    );
    assert!(
        page.contains("function keysNeeded(){ return !!(bk.data && bk.data.ready === false); }")
    );
    assert!(page.contains("function gotoBibleKeys(){ cfg.focusKeys = true; setView('config'); }"));
    assert!(page.contains("function lockedNow(){ return keysNeeded() ||"));
    assert!(page.contains("if(keysNeeded()){ setView('import'); flash('Add a Bible key first.');"));
    assert!(page.contains("showBibleKeys(); }"));
}

#[tokio::test]
async fn the_review_panel_has_its_tiers_the_use_the_fix_box_and_add_as_slide() {
    let (_h, router) = router();
    let page = get(&router, "/").await.text();
    for text in [
        "Use the fix",
        "Add as slide",
        "reject_fixes",
        "state.reject",
        "error",
        "warning",
        "good",
    ] {
        assert!(page.contains(text), "{text}");
    }
}

#[tokio::test]
async fn the_config_page_reads_and_saves_and_the_running_app_uses_the_new_limits() {
    let h = Harness::new("new");
    let router = h.router();
    let got = get(&router, "/api/config").await.json();
    let ids: Vec<&str> = got["sections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert_eq!(&ids[..2], ["verse_slides", "points"]);
    assert_eq!(got["path"], h.dirs.config_file().to_string_lossy().as_ref());
    let after = post_json(
        &router,
        "/api/config",
        &json!({"values": {"points.max_lines_per_slide": 2}}),
    )
    .await;
    assert_eq!(after.status, 200);
    let value = |page: &Value, key: &str| -> Value {
        page["sections"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|s| s["fields"].as_array().unwrap().clone())
            .find(|f| f["key"] == key)
            .unwrap()["value"]
            .clone()
    };
    assert_eq!(value(&after.json(), "points.max_lines_per_slide"), 2);
    assert_eq!(h.state.config().points.max_lines_per_slide, 2);
    let saved = versevision_core::config::AppConfig::from_toml_str(
        &std::fs::read_to_string(h.dirs.config_file()).unwrap(),
    )
    .unwrap();
    assert_eq!(saved.points.max_lines_per_slide, 2);
    let bad = post_json(
        &router,
        "/api/config",
        &json!({"values": {"slides.max_body_lines": 0}}),
    )
    .await;
    assert_eq!(bad.status, 400);
    assert!(bad.json()["error"]
        .as_str()
        .unwrap()
        .contains("slides.max_body_lines"));
    // nothing was saved by the refusal
    assert_eq!(h.state.config().slides.max_body_lines, 5);
    let hidden = post_json(
        &router,
        "/api/config",
        &json!({"values": {"propresenter.host": "evil.example"}}),
    )
    .await;
    assert_eq!(
        hidden.json()["error"],
        "propresenter.host cannot be changed here"
    );
}

#[tokio::test]
async fn the_config_page_answers_for_the_limits_and_kinds_the_tests_of_python_name() {
    let (_h, router) = router();
    let page = get(&router, "/api/config").await.json();
    let field = |key: &str| -> Value {
        page["sections"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|s| s["fields"].as_array().unwrap().clone())
            .find(|f| f["key"] == key)
            .unwrap()
    };
    let body = field("slides.max_body_lines");
    assert_eq!(
        (
            body["kind"].as_str(),
            body["value"].clone(),
            body["min"].clone()
        ),
        (Some("int"), json!(5), json!(1))
    );
    assert!(body["help"]
        .as_str()
        .unwrap()
        .to_lowercase()
        .contains("verse"));
    let points = field("points.max_lines_per_slide");
    assert_eq!(
        (points["value"].clone(), points["min"].clone()),
        (json!(0), json!(0))
    );
    assert!(points["help"]
        .as_str()
        .unwrap()
        .contains("continues on the next slide"));
    assert_eq!(
        field("notes.line_breaks")["choices"],
        json!(["slide", "paragraph"])
    );
    assert_eq!(field("points.strip_trailing_period")["kind"], "bool");
    assert_eq!(field("notes.extra_cue_words")["kind"], "list");
    assert_eq!(field("points.bullet_char")["label"], "Bullet mark");
    let keys: Vec<String> = versevision_web::configpage::editable_keys();
    for hidden in [
        "slides.max_characters",
        "points.theme",
        "verses.theme",
        "propresenter.host",
        "node.command",
    ] {
        assert!(!keys.iter().any(|k| k == hidden), "{hidden}");
    }
    for shown in [
        "slides.max_body_lines",
        "points.max_lines_per_slide",
        "points.fix_label_typos",
        "verses.quote_match",
        "bible.default_version",
    ] {
        assert!(keys.iter().any(|k| k == shown), "{shown}");
    }
}
