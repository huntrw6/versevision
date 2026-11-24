//! The StagePilot Bible service client against a local stand-in for the control plane (no real network): the request, every
//! row of the contract, the retry rule, the splitting, the text cleanup and the credential canary.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::service::*;
use common::span_of;
use serde_json::{json, Value};
use versevision_bible::{InstallationAuth, StagePilotProvider};
use versevision_core::bibles::BibleProvider;

fn provider(origin: &str) -> StagePilotProvider {
    StagePilotProvider::new("stagepilot", Some(TestAuth::enrolled(origin)))
}

fn spans(raw: &[Value]) -> Vec<versevision_core::references::VerseSpan> {
    raw.iter().map(span_of).collect()
}

#[tokio::test]
async fn a_request_has_the_shape_of_the_contract() {
    let (origin, service) = serve_service(|seen, _| contract_success(seen)).await;
    let verses = provider(&origin)
        .fetch_passage("nkjv", "JHN", &spans(&[json!([3, 16, 3, 17])]))
        .await
        .unwrap();
    let seen = service.requests();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, "GET");
    assert_eq!(
        seen[0].path,
        format!("/v1/installations/{INSTALLATION}/bible/passage")
    );
    assert_eq!(seen[0].query["version"], "NKJV");
    assert_eq!(seen[0].query["ref"], "JHN 3:16-17");
    assert_eq!(
        seen[0].authorization.as_deref(),
        Some(format!("Bearer {CREDENTIAL}").as_str())
    );
    let agent = seen[0].user_agent.as_deref().unwrap();
    assert!(agent.starts_with("VerseVision/"), "{agent}");
    assert_eq!(verses.len(), 2);
    assert_eq!(
        (verses[0].chapter, verses[0].verse, verses[1].verse),
        (3, 16, 17)
    );
    assert_eq!(verses[0].book_id, "JHN");
    assert_eq!(verses[0].text, "Synthetic NKJV text JHN 3:16.");
    assert_eq!(verses[0].version, "NKJV");
    assert_eq!(
        verses[0].copyright.as_deref(),
        Some("Synthetic NKJV copyright")
    );
    assert_eq!(
        verses[0].attribution.as_deref(),
        Some("Synthetic NKJV description")
    );
}

#[tokio::test]
async fn the_contract_answer_is_parsed_exactly() {
    let (origin, _service) = serve_service(|seen, _| contract_success(seen)).await;
    let passage = provider(&origin)
        .fetch_reference("esv", "PSA 23:1 & 4")
        .await
        .unwrap();
    assert_eq!(passage.version, "ESV");
    assert_eq!(passage.provider, "esv");
    assert!(!passage.cached);
    assert_eq!(passage.attribution, None);
    assert_eq!(
        passage.verses.iter().map(|v| v.verse).collect::<Vec<_>>(),
        [1, 4]
    );
}

#[tokio::test]
async fn a_long_reference_goes_in_a_post_body() {
    let refuse = |_: &Seen, _: usize| Reply::json(400, json!({"error": "invalid_reference"}));
    let (origin, service) = serve_service(refuse).await;
    let long: String = format!(
        "PSA 119:{}",
        (1..=40)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(" & ")
    );
    let padded = format!("{long}{}", " ".repeat(1600));
    assert!(padded.chars().count() > 1500);
    let error = provider(&origin).fetch_reference("NIV", &padded).await;
    let seen = service.requests();
    assert_eq!(seen[0].method, "POST");
    assert_eq!(
        seen[0].path,
        format!("/v1/installations/{INSTALLATION}/bible/passage")
    );
    assert_eq!(seen[0].content_type.as_deref(), Some("application/json"));
    assert!(seen[0].query.is_empty());
    let body: Value = serde_json::from_str(&seen[0].body).unwrap();
    assert_eq!(body["version"], "NIV");
    assert_eq!(body["ref"], padded);
    assert_eq!(
        seen[0].authorization.as_deref(),
        Some(format!("Bearer {CREDENTIAL}").as_str())
    );
    assert!(
        error.is_err(),
        "the stand-in refuses the padded text; the request itself was a POST"
    );
    // a reference of exactly 1,500 characters is still a GET
    let (origin, service) = serve_service(refuse).await;
    let edge = format!("JHN 3:16{}", " ".repeat(1500 - "JHN 3:16".len()));
    assert_eq!(edge.chars().count(), 1500);
    let _ = provider(&origin).fetch_reference("NIV", &edge).await;
    assert_eq!(service.requests()[0].method, "GET");
}

/// The live KJV answer starts with API.Bible's paragraph mark; Python's API.Bible path never shows it.
#[tokio::test]
async fn the_pilcrow_of_the_live_answer_is_removed_and_nothing_else() {
    let (origin, _service) = serve_service(|seen, _| {
        let mut reply = contract_success(seen);
        let mut body: Value = serde_json::from_str(&reply.body).unwrap();
        body["verses"][0]["text"] = json!("\u{b6} For God so loved the world \u{b6} that he gave.");
        reply.body = body.to_string();
        reply
    })
    .await;
    let verses = provider(&origin)
        .fetch_passage("KJV", "JHN", &spans(&[json!([3, 16])]))
        .await
        .unwrap();
    // only the leading mark and its space go; a mark inside the verse stays
    assert_eq!(
        verses[0].text,
        "For God so loved the world \u{b6} that he gave."
    );
}

#[tokio::test]
async fn a_passage_is_split_per_chapter_and_per_forty_verses_and_joined_in_order() {
    let (origin, service) = serve_service(|seen, _| contract_success(seen)).await;
    let p = provider(&origin);
    // across a chapter boundary: John 3 has 36 verses
    let verses = p
        .fetch_passage("NIV", "JHN", &spans(&[json!([3, 16, 4, 2])]))
        .await
        .unwrap();
    let refs: Vec<String> = service.requests().iter().map(|s| s.asked().1).collect();
    assert_eq!(refs, ["JHN 3:16-36", "JHN 4:1-2"]);
    let got: Vec<(u64, u64)> = verses.iter().map(|v| (v.chapter, v.verse)).collect();
    let mut want: Vec<(u64, u64)> = (16..=36).map(|v| (3, v)).collect();
    want.extend([(4, 1), (4, 2)]);
    assert_eq!(got, want);

    // 176 verses: 40 per request, in order
    let (origin, service) = serve_service(|seen, _| contract_success(seen)).await;
    let verses = provider(&origin)
        .fetch_passage("KJV", "PSA", &spans(&[json!([119, 1, 119, 176])]))
        .await
        .unwrap();
    assert_eq!(verses.len(), 176);
    assert!(verses
        .iter()
        .enumerate()
        .all(|(i, v)| v.chapter == 119 && v.verse == i as u64 + 1));
    let refs: Vec<String> = service.requests().iter().map(|s| s.asked().1).collect();
    assert_eq!(
        refs,
        [
            "PSA 119:1-40",
            "PSA 119:41-80",
            "PSA 119:81-120",
            "PSA 119:121-160",
            "PSA 119:161-176"
        ]
    );
    // no request asks for more than 40 verses or for a range across chapters
    for seen in service.requests() {
        assert!(verses_of(&seen.asked().1).len() <= 40);
    }

    // several spans of one chapter share one request
    let (origin, service) = serve_service(|seen, _| contract_success(seen)).await;
    provider(&origin)
        .fetch_passage(
            "NIV",
            "JHN",
            &spans(&[json!([3, 16, 3, 18]), json!([3, 20])]),
        )
        .await
        .unwrap();
    assert_eq!(service.requests().len(), 1);
    assert_eq!(service.requests()[0].asked().1, "JHN 3:16-18 & 20");
}

async fn error_for(status: u16, body: Value, headers: &[(&str, &str)]) -> (String, usize) {
    let headers: Vec<(String, String)> = headers
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
    let (origin, service) = serve_service(move |_, _| {
        let mut reply = Reply::json(status, body.clone());
        reply.headers = headers.clone();
        reply
    })
    .await;
    let error = provider(&origin)
        .fetch_passage("NKJV", "JHN", &spans(&[json!([3, 16])]))
        .await
        .unwrap_err();
    (error.message().to_string(), service.count())
}

#[tokio::test]
async fn every_error_row_of_the_contract_has_its_own_text() {
    let ends = "; no presentation files changed";
    type Row = (u16, Value, Vec<(&'static str, &'static str)>, &'static str);
    let rows: Vec<Row> = vec![
        (401, json!({"error": "unauthorized"}), vec![],
         "StagePilot Bible service request failed: this installation's credential was rejected (restart StagePilot to sign in again, or add your own Bible key)"),
        (429, json!({"error": "rate_limited", "scope": "installation_hourly", "message": "x"}), vec![("retry-after", "1800")],
         "StagePilot Bible service request failed: rate limit exceeded (hourly limit for this installation, try again in 1800 seconds); you can add your own Bible key"),
        (429, json!({"error": "rate_limited", "scope": "installation_daily"}), vec![("retry-after", "7200")],
         "StagePilot Bible service request failed: rate limit exceeded (daily limit for this installation, try again in 7200 seconds); you can add your own Bible key"),
        (429, json!({"error": "rate_limited", "scope": "global_daily"}), vec![],
         "StagePilot Bible service request failed: rate limit exceeded (daily limit of the service); you can add your own Bible key"),
        (503, json!({"error": "provider_not_configured", "message": "no key"}), vec![("retry-after", "300")],
         "StagePilot Bible service request failed: the service has no key for NKJV yet (add your own Bible key)"),
        (503, json!({"error": "upstream_busy"}), vec![("retry-after", "60")],
         "StagePilot Bible service request failed: the Bible provider is busy, try again in 60 seconds"),
        (400, json!({"error": "invalid_version", "message": "unknown"}), vec![],
         "StagePilot Bible service request failed: the service does not offer NKJV"),
        (400, json!({"error": "invalid_reference", "message": "Unknown book."}), vec![],
         "StagePilot Bible service request failed: the service refused the request (invalid_reference): Unknown book."),
        (400, json!({"error": "unsupported_reference", "message": "Ask per chapter."}), vec![],
         "StagePilot Bible service request failed: the service refused the request (unsupported_reference): Ask per chapter."),
        (400, json!({"error": "too_many_verses", "message": "Max 40."}), vec![],
         "StagePilot Bible service request failed: the service refused the request (too_many_verses): Max 40."),
        (400, json!({"error": "invalid_request"}), vec![],
         "StagePilot Bible service request failed: the service refused the request (invalid_request)"),
        (405, json!({"error": "method_not_allowed"}), vec![],
         "StagePilot Bible service request failed: HTTP 405 (method_not_allowed)"),
        (413, json!({"error": "request_too_large"}), vec![],
         "StagePilot Bible service request failed: HTTP 413 (request_too_large)"),
        (502, json!({"error": "upstream_error"}), vec![],
         "StagePilot Bible service request failed: HTTP 502 (upstream_error)"),
        (504, json!({"error": "upstream_timeout"}), vec![],
         "StagePilot Bible service request failed: the Bible provider did not answer in time (HTTP 504)"),
        (500, json!("not an object"), vec![],
         "StagePilot Bible service request failed: HTTP 500"),
    ];
    for (status, body, headers, want) in rows {
        let (message, requests) = error_for(status, body, &headers).await;
        assert_eq!(message, format!("{want}{ends}"), "HTTP {status}");
        assert_eq!(requests, 1, "HTTP {status}: no retry");
    }
}

#[tokio::test]
async fn a_429_with_a_short_retry_after_is_retried_once_and_only_once() {
    // Retry-After 1: one retry, which succeeds
    let (origin, service) = serve_service(|seen, index| {
        if index == 0 {
            Reply::json(
                429,
                json!({"error": "rate_limited", "scope": "installation_hourly"}),
            )
            .with("retry-after", "1")
        } else {
            contract_success(seen)
        }
    })
    .await;
    let started = Instant::now();
    let verses = provider(&origin)
        .fetch_passage("NIV", "JHN", &spans(&[json!([3, 16])]))
        .await
        .unwrap();
    assert_eq!(verses.len(), 1);
    assert_eq!(service.count(), 2);
    assert!(started.elapsed() >= Duration::from_millis(900), "it waited");

    // 429 again after the retry: no second retry
    let (origin, service) = serve_service(|_, _| {
        Reply::json(
            429,
            json!({"error": "rate_limited", "scope": "installation_hourly"}),
        )
        .with("retry-after", "1")
    })
    .await;
    let error = provider(&origin)
        .fetch_passage("NIV", "JHN", &spans(&[json!([3, 16])]))
        .await
        .unwrap_err();
    assert!(error.message().contains("rate limit exceeded"));
    assert_eq!(service.count(), 2, "one retry, not a storm");

    // Retry-After above 5 s, or none, or not a number: no retry at all
    for header in [Some("6"), Some("60"), Some("soon"), None] {
        let (origin, service) = serve_service(move |_, _| {
            let reply = Reply::json(429, json!({"error": "rate_limited"}));
            match header {
                Some(value) => reply.with("retry-after", value),
                None => reply,
            }
        })
        .await;
        let started = Instant::now();
        assert!(provider(&origin)
            .fetch_passage("NIV", "JHN", &spans(&[json!([3, 16])]))
            .await
            .is_err());
        assert_eq!(service.count(), 1, "Retry-After {header:?}");
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}

#[tokio::test]
async fn a_bad_answer_is_an_error_not_a_guess() {
    let shapes: Vec<Value> = vec![
        json!([]),
        json!({"version": "NIV"}),
        json!({"version":"NIV","reference":"JHN 3:16","verses":"x","copyright":null,"attribution":null,"provider":"youversion","cached":false}),
        json!({"version":"NIV","reference":"JHN 3:16","verses":[{"book_id":"JHN","chapter":"3","verse":16,"text":"t"}],"copyright":null,"attribution":null,"provider":"youversion","cached":false}),
        json!({"version":"NIV","reference":"JHN 3:16","verses":[{"book_id":"JHN","chapter":3,"verse":16,"text":"t"}],"copyright":null,"attribution":null,"provider":"somebody","cached":false}),
        json!({"version":"NIV","reference":"JHN 3:16","verses":[{"book_id":"JHN","chapter":3,"verse":16,"text":"t"}],"copyright":5,"attribution":null,"provider":"youversion","cached":false}),
        json!({"version":"NIV","reference":"JHN 3:16","verses":[{"book_id":"JHN","chapter":3,"verse":16,"text":"t"}],"copyright":null,"attribution":null,"provider":"youversion"}),
    ];
    for shape in shapes {
        let (origin, _s) = serve_service(move |_, _| Reply::json(200, shape.clone())).await;
        let error = provider(&origin)
            .fetch_passage("NIV", "JHN", &spans(&[json!([3, 16])]))
            .await
            .unwrap_err();
        assert_eq!(
            error.message(),
            "StagePilot Bible service response failed: unexpected response shape; no presentation files changed"
        );
    }
    // not JSON at all
    let (origin, _s) = serve_service(|_, _| Reply {
        status: 200,
        headers: vec![],
        body: "<html>Cloudflare</html>".to_string(),
        delay: None,
    })
    .await;
    let error = provider(&origin)
        .fetch_passage("NIV", "JHN", &spans(&[json!([3, 16])]))
        .await
        .unwrap_err();
    assert!(error.message().contains("response failed"));
    // verses that are not the ones asked for
    let (origin, _s) = serve_service(|seen, _| {
        let mut reply = contract_success(seen);
        let mut body: Value = serde_json::from_str(&reply.body).unwrap();
        body["verses"][0]["verse"] = json!(99);
        reply.body = body.to_string();
        reply
    })
    .await;
    let error = provider(&origin)
        .fetch_passage("NIV", "JHN", &spans(&[json!([3, 16])]))
        .await
        .unwrap_err();
    assert!(error.message().contains("not the ones asked for"));
    // an empty verse
    let (origin, _s) = serve_service(|seen, _| {
        let mut reply = contract_success(seen);
        let mut body: Value = serde_json::from_str(&reply.body).unwrap();
        body["verses"][0]["text"] = json!("\u{b6} ");
        reply.body = body.to_string();
        reply
    })
    .await;
    let error = provider(&origin)
        .fetch_passage("NIV", "JHN", &spans(&[json!([3, 16])]))
        .await
        .unwrap_err();
    assert_eq!(
        error.message(),
        "StagePilot Bible service returned no text for JHN 3:16"
    );
}

#[tokio::test]
async fn a_slow_service_times_out_and_a_dead_one_is_a_connection_error() {
    let (origin, _s) = serve_service(|seen, _| {
        let mut reply = contract_success(seen);
        reply.delay = Some(Duration::from_secs(3));
        reply
    })
    .await;
    let error = provider(&origin)
        .timeout(0.3)
        .fetch_passage("NIV", "JHN", &spans(&[json!([3, 16])]))
        .await
        .unwrap_err();
    assert_eq!(
        error.message(),
        "StagePilot Bible service request timed out; no presentation files changed"
    );
    let dead = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };
    let error = provider(&dead)
        .fetch_passage("NIV", "JHN", &spans(&[json!([3, 16])]))
        .await
        .unwrap_err();
    assert_eq!(
        error.message(),
        "StagePilot Bible service response failed: All connection attempts failed; no presentation files changed"
    );
}

#[tokio::test]
async fn without_an_installation_the_provider_is_unavailable_and_asks_nobody() {
    let (origin, service) = serve_service(|seen, _| contract_success(seen)).await;
    let cases: Vec<Option<Arc<dyn InstallationAuth>>> = vec![
        None,
        Some(Arc::new(TestAuth {
            origin: origin.clone(),
            id: None,
            credential: Some(CREDENTIAL.to_string()),
        })),
        Some(Arc::new(TestAuth {
            origin: origin.clone(),
            id: Some(INSTALLATION.to_string()),
            credential: None,
        })),
        Some(Arc::new(TestAuth {
            origin: origin.clone(),
            id: Some(String::new()),
            credential: Some(CREDENTIAL.to_string()),
        })),
    ];
    for auth in cases {
        let p = StagePilotProvider::new("stagepilot", auth);
        assert!(!p.is_available());
        let error = p
            .fetch_passage("NIV", "JHN", &spans(&[json!([3, 16])]))
            .await
            .unwrap_err();
        assert!(
            error
                .message()
                .contains("not connected to a StagePilot installation"),
            "{error}"
        );
        let health = p.health_check().await;
        assert!(!health.healthy);
        assert!(health.message.contains("not available"));
        // the versions are still listed: that needs no network
        assert_eq!(p.list_versions().await.unwrap().len(), 13);
    }
    assert_eq!(service.count(), 0);
}

#[tokio::test]
async fn each_scope_serves_only_its_versions() {
    let (origin, service) = serve_service(|seen, _| contract_success(seen)).await;
    let stored = provider(&origin);
    let esv = StagePilotProvider::for_esv("stagepilot-esv", Some(TestAuth::enrolled(&origin)));
    let names = |versions: Vec<versevision_core::versions::BibleVersion>| {
        versions
            .into_iter()
            .map(|v| v.abbreviation)
            .collect::<Vec<_>>()
    };
    let stored_versions = names(stored.list_versions().await.unwrap());
    assert_eq!(stored_versions.len(), 13);
    assert!(!stored_versions.contains(&"ESV".to_string()));
    for label in [
        "NIV", "NKJV", "NLT", "KJV", "MSG", "WEB", "AMP", "BSB", "LSV", "NIRV", "NASB1995",
        "NASB2020", "TPT",
    ] {
        assert!(stored_versions.contains(&label.to_string()), "{label}");
    }
    assert_eq!(names(esv.list_versions().await.unwrap()), ["ESV"]);
    assert!(!esv.cacheable() && stored.cacheable());
    for (p, version) in [
        (&stored, "ESV"),
        (&esv, "NIV"),
        (&stored, "XYZ"),
        (&esv, "YLT"),
    ] {
        let error = p
            .fetch_passage(version, "JHN", &spans(&[json!([3, 16])]))
            .await
            .unwrap_err();
        assert_eq!(
            error.message(),
            format!("StagePilot Bible service does not offer {version}")
        );
    }
    assert_eq!(service.count(), 0);
    // the stored scope never hands out ESV text, even when the service answers a version with it
    let (origin, _s) = serve_service(|seen, _| {
        let mut reply = contract_success(seen);
        let mut body: Value = serde_json::from_str(&reply.body).unwrap();
        body["provider"] = json!("esv");
        reply.body = body.to_string();
        reply
    })
    .await;
    let error = provider(&origin)
        .fetch_passage("NIV", "JHN", &spans(&[json!([3, 16])]))
        .await
        .unwrap_err();
    assert!(error.message().contains("ESV text, which is never stored"));
}
