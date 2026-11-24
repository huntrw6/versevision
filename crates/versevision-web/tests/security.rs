//! Same-origin only: no CORS, an Origin and Fetch-Metadata check on everything that writes, the Host allow-list, the access token, the
//! headers on every answer, and what serving on a network address needs.

mod common;
use common::*;

use axum::http::Method;
use serde_json::json;

#[tokio::test]
async fn a_cross_origin_post_is_refused_and_a_same_origin_one_is_not() {
    let h = Harness::new("new");
    let router = h.router();
    let body = json!({"values": {"slides.max_body_lines": 4}})
        .to_string()
        .into_bytes();
    for (origin, site) in [
        ("http://evil.example", None),
        ("http://localhost:9999", None),
        ("null", None),
        ("http://localhost:8777", Some("cross-site")),
        ("http://localhost:8777", Some("same-site")),
    ] {
        let mut headers = vec![("host", "localhost:8777"), ("origin", origin)];
        if let Some(site) = site {
            headers.push(("sec-fetch-site", site));
        }
        let answer = send(&router, Method::POST, "/api/config", &headers, body.clone()).await;
        assert_eq!(
            (answer.status, answer.json()),
            (403, json!({"error": "Not allowed"})),
            "{origin} {site:?}"
        );
    }
    assert_eq!(
        h.state.config().slides.max_body_lines,
        5,
        "nothing was saved"
    );
    // the page's own requests
    for headers in [
        vec![
            ("host", "localhost:8777"),
            ("origin", "http://localhost:8777"),
            ("sec-fetch-site", "same-origin"),
        ],
        vec![("host", "localhost:8777")],
        vec![("host", "localhost:8777"), ("sec-fetch-site", "none")],
    ] {
        let answer = send(&router, Method::POST, "/api/config", &headers, body.clone()).await;
        assert_eq!(answer.status, 200, "{headers:?}: {}", answer.text());
    }
}

#[tokio::test]
async fn no_answer_opens_the_page_to_other_origins() {
    let h = Harness::new("new");
    let router = h.router();
    for path in ["/", "/api/status", "/api/config", "/nothing", "/icon.svg"] {
        let answer = get(&router, path).await;
        assert!(
            !answer
                .headers
                .keys()
                .any(|k| k.as_str().starts_with("access-control-")),
            "{path}"
        );
    }
    // a preflight is not answered with permission
    let preflight = send(
        &router,
        Method::OPTIONS,
        "/api/config",
        &[
            ("host", "localhost:8777"),
            ("origin", "http://evil.example"),
            ("access-control-request-method", "POST"),
        ],
        Vec::new(),
    )
    .await;
    assert!(
        preflight.status >= 400
            && !preflight
                .headers
                .contains_key("access-control-allow-origin")
    );
}

#[tokio::test]
async fn every_answer_carries_the_headers_python_sends() {
    let h = Harness::new("new");
    let router = h.router();
    for path in ["/", "/api/status", "/nothing", "/sw.js"] {
        let answer = get(&router, path).await;
        assert_eq!(answer.headers["cache-control"], "no-store", "{path}");
        assert_eq!(answer.headers["x-content-type-options"], "nosniff");
        assert_eq!(answer.headers["referrer-policy"], "no-referrer");
        assert_eq!(
            answer.headers["content-security-policy"],
            "default-src 'self'; style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline'"
        );
    }
    assert_eq!(
        get(&router, "/nothing").await.json(),
        json!({"error": "Not found"})
    );
}

#[tokio::test]
async fn the_access_token_guards_the_api_but_not_the_app_shell() {
    let h = Harness::with("new", |b| b.token(Some("s3cret".into())));
    let router = h.router();
    assert_eq!(get(&router, "/").await.status, 200);
    assert_eq!(get(&router, "/icon.svg").await.status, 200);
    let answer = get(&router, "/api/status").await;
    assert_eq!(
        (answer.status, answer.json()),
        (401, json!({"error": "Access token required"}))
    );
    assert_eq!(get(&router, "/api/status?token=wrong").await.status, 401);
    assert_eq!(get(&router, "/api/status?token=s3cret").await.status, 200);
    let header = send(
        &router,
        Method::GET,
        "/api/status",
        &[("x-versevision-token", "s3cret")],
        Vec::new(),
    )
    .await;
    assert_eq!(header.status, 200);
    let post = post_json(&router, "/api/config", &json!({})).await;
    assert_eq!(
        (post.status, post.json()),
        (403, json!({"error": "Not allowed"}))
    );
    let ok = send(
        &router,
        Method::POST,
        "/api/config",
        &[("host", "localhost"), ("x-versevision-token", "s3cret")],
        b"{}".to_vec(),
    )
    .await;
    assert_eq!(ok.status, 200);
}

#[tokio::test]
async fn the_host_allow_list_stops_dns_rebinding() {
    let h = Harness::with("new", |b| {
        b.allowed_hosts(vec!["localhost".into(), "127.0.0.1".into(), "::1".into()])
    });
    let router = h.router();
    for host in ["localhost:8777", "127.0.0.1:1", "[::1]:8777", "LOCALHOST"] {
        let answer = send(
            &router,
            Method::GET,
            "/api/status",
            &[("host", host)],
            Vec::new(),
        )
        .await;
        assert_eq!(answer.status, 200, "{host}");
    }
    for host in [
        "evil.example:8777",
        "evil.example",
        "localhost.evil.example",
    ] {
        let answer = send(
            &router,
            Method::GET,
            "/api/status",
            &[("host", host)],
            Vec::new(),
        )
        .await;
        assert_eq!(
            (answer.status, answer.json()),
            (403, json!({"error": "Not allowed"})),
            "{host}"
        );
    }
}

#[tokio::test]
async fn a_bad_body_is_a_400_that_does_not_echo_it() {
    let h = Harness::new("new");
    let router = h.router();
    let answer = send(
        &router,
        Method::POST,
        "/api/config",
        &[("host", "localhost")],
        b"{\"values\": SECRET-CANARY".to_vec(),
    )
    .await;
    assert_eq!(answer.status, 400);
    assert!(!answer.text().contains("SECRET-CANARY"));
    let list = send(
        &router,
        Method::POST,
        "/api/apply",
        &[("host", "localhost")],
        b"[1,2]".to_vec(),
    )
    .await;
    assert_eq!(list.status, 400);
}

async fn raw_request(addr: std::net::SocketAddr, request: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    stream.read_to_end(&mut out).await.unwrap();
    String::from_utf8_lossy(&out).into_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serving_binds_where_the_host_says_and_a_network_address_needs_a_token() {
    let h = Harness::new("new");
    // a network address with no token is refused outright
    let any = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let error = versevision_web::serve(any, h.state.clone())
        .await
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    // loopback: answers its own names only
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = h.state.clone();
    let server = tokio::spawn(async move { versevision_web::serve(listener, state).await });
    let own = raw_request(
        addr,
        &format!(
            "GET /api/status HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
            addr.port()
        ),
    )
    .await;
    assert!(own.starts_with("HTTP/1.1 200"), "{own}");
    let rebound = raw_request(
        addr,
        &format!(
            "GET /api/status HTTP/1.1\r\nHost: evil.example:{}\r\nConnection: close\r\n\r\n",
            addr.port()
        ),
    )
    .await;
    assert!(rebound.starts_with("HTTP/1.1 403"), "{rebound}");
    server.abort();
    assert_eq!(versevision_web::DEFAULT_ADDR.to_string(), "127.0.0.1:8777");
}
