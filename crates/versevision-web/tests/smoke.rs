mod common;
use common::*;

#[tokio::test]
async fn the_page_and_status_answer() {
    let h = Harness::new("new");
    let router = h.router();
    let page = get(&router, "/").await;
    assert_eq!(page.status, 200);
    assert!(page.text().contains("VerseVision"));
    let status = get(&router, "/api/status").await.json();
    assert_eq!(status["template_ready"], true);
    assert_eq!(status["version"], "0.0.0-test");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_plan_runs_and_is_reviewable() {
    let h = Harness::new("new");
    let router = h.router();
    let answer = upload(&router, "?version=NIV", "n.txt", NOTES.as_bytes()).await;
    assert_eq!(answer.status, 202, "{}", answer.text());
    let id = answer.json()["job"].as_str().unwrap().to_string();
    let job = finished(&router, &id).await;
    assert_eq!(job["state"], "done", "{job}");
    let summary = &job["result"]["summary"];
    println!("{summary}");
    assert_eq!(summary["errors"], 1);
}
