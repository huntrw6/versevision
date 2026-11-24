//! The review: plan -> grouped decisions -> fix a line and recheck -> switch a slide off -> import (`tests/test_web_review.py`), through the
//! router, against the scripted ProPresenter.

mod common;
use common::*;

use serde_json::{json, Value};
use versevision_pp::runlog::read_trace;

async fn plan(router: &axum::Router, query: &str, notes: &str) -> Value {
    let answer = upload(router, query, "n.txt", notes.as_bytes()).await;
    assert_eq!(answer.status, 202, "{}", answer.text());
    let job = finished(router, answer.json()["job"].as_str().unwrap()).await;
    assert_eq!(job["state"], "done", "{job}");
    let mut job = job;
    job["id"] = answer.json()["job"].clone();
    job
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_plan_is_graded_and_every_line_can_be_edited_and_rechecked() {
    let h = Harness::new("new");
    let router = h.router();
    let job = plan(&router, "?version=NIV", NOTES).await;
    let r = &job["result"];
    assert_eq!(r["summary"]["errors"], 1);
    assert!(r["summary"]["warnings"].as_u64().unwrap() >= 1);
    assert!(r["summary"]["good"].as_u64().unwrap() >= 2);
    let error = &r["review"]["error"][0];
    assert!(error["title"].as_str().unwrap().contains("John 3:99"));
    let line = error["line"].as_u64().unwrap().to_string();
    assert!(r["lines"][&line].as_str().unwrap().starts_with("John 3:99"));
    // fix the bad verse; the whole document is checked again
    let answer = post_json(
        &router,
        "/api/replan",
        &json!({"job": job["id"], "edits": {line.clone(): "John 3:17 NIV"}}),
    )
    .await;
    assert_eq!(answer.status, 202, "{}", answer.text());
    let redo = finished(&router, answer.json()["job"].as_str().unwrap()).await;
    assert_eq!(redo["state"], "done", "{redo}");
    assert_eq!(redo["result"]["summary"]["errors"], 0);
    assert!(redo["result"]["review"]["good"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["title"].as_str().unwrap().contains("John 3:17")));
    // the recheck says so in its run
    let run = redo["result"]["run_id"].as_str().unwrap();
    let trace = read_trace(&h.dirs.runs_dir(), run);
    assert!(trace
        .iter()
        .any(|r| r["stage"] == "review_edit" && r["changes"][0]["after"] == "John 3:17 NIV"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_decision_can_be_switched_off_before_importing() {
    let h = Harness::new("new");
    let router = h.router();
    let job = plan(&router, "?version=NIV", NOTES).await;
    let r = &job["result"];
    let warn = r["review"]["warning"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| !d["slides"].as_array().unwrap().is_empty())
        .unwrap();
    let kept =
        r["summary"]["slides"].as_u64().unwrap() - warn["slides"].as_array().unwrap().len() as u64;
    let answer = post_json(
        &router,
        "/api/apply",
        &json!({"job": job["id"], "accept_flags": true, "skip": [warn["id"]]}),
    )
    .await;
    assert_eq!(answer.status, 202, "{}", answer.text());
    let done = finished(&router, answer.json()["job"].as_str().unwrap()).await;
    assert_eq!(done["state"], "done", "{done}");
    assert_eq!(done["result"]["slide_count"].as_u64().unwrap(), kept);
    assert_eq!(done["message"], "Done");
    // the history lists it, and it can be undone
    let history = get(&router, "/api/history").await.json();
    assert_eq!(history[0]["mode"], "new");
    assert_eq!(history[0]["slides"].as_u64().unwrap(), kept);
    assert!(history[0]["when"].as_str().unwrap().ends_with("+00:00"));
    let undo = post_json(&router, "/api/undo", &json!({"id": history[0]["id"]})).await;
    assert_eq!(undo.status, 202, "{}", undo.text());
    let undone = finished(&router, undo.json()["job"].as_str().unwrap()).await;
    assert_eq!(undone["state"], "done", "{undone}");
    assert_eq!(
        get(&router, "/api/history").await.json()[0]["status"],
        "undone"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_left_out_line_ticked_in_the_review_becomes_a_slide_when_importing() {
    let h = Harness::new("new");
    let router = h.router();
    let job = plan(&router, "?version=NIV", NOTES).await;
    let r = &job["result"];
    let title = r["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["kind"] == "left_out" && d["title"].as_str().unwrap().starts_with("Title 11"))
        .unwrap();
    assert_eq!(title["addable"], true);
    assert_eq!(title["add_text"], "\u{2022} Title 11/8/26");
    assert_eq!(title["slides"], json!([]));
    let before = r["summary"]["slides"].as_u64().unwrap();
    let answer = post_json(
        &router,
        "/api/apply",
        &json!({"job": job["id"], "accept_flags": true, "include": [title["id"]]}),
    )
    .await;
    let done = finished(&router, answer.json()["job"].as_str().unwrap()).await;
    assert_eq!(done["state"], "done", "{done}");
    assert_eq!(done["result"]["slide_count"].as_u64().unwrap(), before + 1);
    let trace = read_trace(&h.dirs.runs_dir(), r["run_id"].as_str().unwrap());
    assert!(trace
        .iter()
        .any(|t| t["stage"] == "review_included"
            && t["added"][0]["text"] == "\u{2022} Title 11/8/26"));
}

const FIXES: &str = "Title 11/8/26\n\nKEY#1) ONE\nKEY #2) TWO\nKEY #3) THREE\nKEY #4) FOUR\n(pause)\nSome stray line of prose to maybe add.\nJohn 3:16 NIV\n";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_automatic_fix_can_be_refused_with_use_the_fix_unticked() {
    let h = Harness::new("reject-fixes");
    let router = h.router();
    let job = plan(&router, "", FIXES).await;
    let fix = job["result"]["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| !d["correction"].is_null())
        .expect("a decision carries an automatic fix");
    let kept_text = fix["correction"]["from"].as_str().unwrap().to_string();
    let answer = post_json(
        &router,
        "/api/apply",
        &json!({"job": job["id"], "accept_flags": true, "reject_fixes": [fix["id"]]}),
    )
    .await;
    let done = finished(&router, answer.json()["job"].as_str().unwrap()).await;
    assert_eq!(done["state"], "done", "{done}");
    let trace = read_trace(
        &h.dirs.runs_dir(),
        job["result"]["run_id"].as_str().unwrap(),
    );
    assert!(
        trace
            .iter()
            .any(|t| t["stage"] == "review_fix_rejected" && t["kept"] == kept_text.as_str()),
        "{trace:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_refusals_have_pythons_words() {
    let h = Harness::with_config("new", |c| c.presentation_template = None);
    let router = h.router();
    // no template: the plan job fails with the setup hint
    let answer = upload(&router, "", "n.txt", b"x").await;
    let job = finished(&router, answer.json()["job"].as_str().unwrap()).await;
    assert_eq!(job["state"], "error");
    assert_eq!(
        job["error"],
        "No template is set up yet. Run: versevision template setup"
    );
    assert_eq!(job["exit_code"], 1);
    // a file type that is not accepted
    let answer = upload(&router, "", "n.exe", b"x").await;
    assert_eq!(
        (answer.status, answer.json()),
        (
            400,
            json!({"error": "Unsupported file type .exe. Use PDF, Word (.docx), text, Markdown or HTML."})
        )
    );
    let answer = upload(&router, "", "noext", b"x").await;
    assert_eq!(
        answer.json()["error"],
        "Unsupported file type (none). Use PDF, Word (.docx), text, Markdown or HTML."
    );
    // a path in the name is cut to the file name
    let answer = upload(&router, "", "..\\..\\etc\\n.txt", b"x").await;
    assert_eq!(answer.status, 202);
    // an unknown mode
    let answer = upload(&router, "?mode=sideways", "n.txt", b"x").await;
    let job = finished(&router, answer.json()["job"].as_str().unwrap()).await;
    assert_eq!(job["error"], "Unknown mode");
    // replace without a target
    let h2 = Harness::new("new");
    let router2 = h2.router();
    let answer = upload(&router2, "?mode=replace", "n.txt", NOTES.as_bytes()).await;
    let job = finished(&router2, answer.json()["job"].as_str().unwrap()).await;
    assert_eq!(job["error"], "Choose which presentation to change");
    // a job and a run that do not exist
    assert_eq!(get(&router, "/api/jobs/nope").await.status, 404);
    assert_eq!(
        get(&router, "/api/run/nope").await.json(),
        json!({"error": "No such run"})
    );
    // apply with nothing to apply
    let answer = post_json(&router, "/api/apply", &json!({"job": "nope"})).await;
    assert_eq!(
        answer.json()["error"],
        "That review is no longer available; drop the file again"
    );
    // the replan of a line that is not there
    let h3 = Harness::new("new");
    let router3 = h3.router();
    let job = plan(&router3, "?version=NIV", NOTES).await;
    let answer = post_json(
        &router3,
        "/api/replan",
        &json!({"job": job["id"], "edits": {"99": "x"}}),
    )
    .await;
    assert_eq!(
        (answer.status, answer.json()["error"].as_str().unwrap()),
        (400, "There is no line 99")
    );
    let answer = post_json(
        &router3,
        "/api/replan",
        &json!({"job": job["id"], "edits": {"x": "y"}}),
    )
    .await;
    assert_eq!(
        answer.json()["error"],
        "Bad request: invalid literal for int() with base 10: 'x'"
    );
    // everything switched off, and flagged lines not accepted
    let answer = post_json(&router3, "/api/apply", &json!({"job": job["id"]})).await;
    assert_eq!(
        answer.json()["error"],
        "Some lines are flagged; confirm you want to import the rest"
    );
    let all: Vec<u64> = job["result"]["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["id"].as_u64().unwrap())
        .collect();
    let answer = post_json(
        &router3,
        "/api/apply",
        &json!({"job": job["id"], "accept_flags": true, "skip": all}),
    )
    .await;
    assert_eq!(
        answer.json()["error"],
        "Every slide was switched off; nothing to import"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_one_import_runs_at_a_time_and_settings_wait_for_it() {
    let h = Harness::new("new");
    let router = h.router();
    let job = plan(&router, "?version=NIV", NOTES).await;
    h.gate.set(false); // the import will stand still before it writes its file
    let first = post_json(
        &router,
        "/api/apply",
        &json!({"job": job["id"], "accept_flags": true}),
    )
    .await;
    assert_eq!(first.status, 202);
    let id = first.json()["job"].as_str().unwrap().to_string();
    for _ in 0..100 {
        if h.state.busy() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let second = post_json(
        &router,
        "/api/apply",
        &json!({"job": job["id"], "accept_flags": true}),
    )
    .await;
    assert_eq!(
        (second.status, second.json()["error"].as_str().unwrap()),
        (400, "Another import is already running")
    );
    let undo = post_json(&router, "/api/undo", &json!({"id": "x"})).await;
    assert_eq!(undo.json()["error"], "Another import is already running");
    let config = post_json(
        &router,
        "/api/config",
        &json!({"values": {"slides.max_body_lines": 4}}),
    )
    .await;
    assert_eq!(
        (config.status, config.json()["error"].as_str().unwrap()),
        (
            400,
            "An import is running; change settings when it finishes"
        )
    );
    assert!(
        !h.dirs.config_file().exists()
            || !std::fs::read_to_string(h.dirs.config_file())
                .unwrap()
                .contains("max_body_lines = 4")
    );
    assert_eq!(
        get(&router, &format!("/api/jobs/{id}")).await.json()["state"],
        "running"
    );
    h.gate.set(true);
    let done = finished(&router, &id).await;
    assert_eq!(done["state"], "done", "{done}");
    assert!(!h.state.busy());
    let config = post_json(
        &router,
        "/api/config",
        &json!({"values": {"slides.max_body_lines": 4}}),
    )
    .await;
    assert_eq!(config.status, 200);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_over_the_limit_is_refused_with_the_limit_in_the_message() {
    let h = Harness::new("new");
    let router = h.router();
    let big = vec![b'a'; 25 * 1024 * 1024 + 1];
    let answer = upload(&router, "", "big.txt", &big).await;
    assert_eq!(
        (answer.status, answer.json()),
        (
            400,
            json!({"error": "That file is too large (limit 25 MB)"})
        )
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn playlists_list_their_presentations_under_their_section_headers() {
    let h = Harness::new("new");
    let router = h.router();
    let lists = get(&router, "/api/playlists").await.json();
    let first = &lists["playlists"][0];
    assert_eq!(first["name"], "Sunday");
    let entry = &first["entries"][0];
    assert_eq!(
        (
            entry["section"].as_str(),
            entry["name"].as_str(),
            entry["library"].as_str()
        ),
        (Some("Sermon"), Some("Sample"), Some("Verses"))
    );
    assert_eq!(entry["uuid"], "P1");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_look_can_be_chosen_from_the_loaded_themes_and_a_recheck_keeps_it() {
    let h = Harness::new("themes-look");
    let router = h.router();
    let themes = get(&router, "/api/themes").await.json();
    let rows = themes["themes"].as_array().unwrap();
    let labels: std::collections::BTreeSet<&str> =
        rows.iter().map(|r| r["label"].as_str().unwrap()).collect();
    assert_eq!(
        labels,
        [
            "Scripture main screens \u{b7} Slide 1",
            "Sermon Points \u{b7} Theme Slide",
            "The biblke \u{b7} Slide 1"
        ]
        .into_iter()
        .collect()
    );
    let two_box = rows
        .iter()
        .find(|r| r["label"].as_str().unwrap().starts_with("The biblke"))
        .unwrap();
    assert_eq!(
        (two_box["reference_box"].clone(), two_box["key"].as_str()),
        (json!(false), Some("The biblke::1"))
    );
    assert_eq!(
        (
            two_box["group"].as_str(),
            two_box["slide"].as_str(),
            two_box["number"].as_u64()
        ),
        (Some(""), Some("Slide 1"), Some(1))
    );
    let query = format!(
        "?version=NIV&verse_look={}&point_look={}",
        "The%20biblke%3A%3A1", "Sermon%20Points%3A%3A1"
    );
    let job = plan(&router, &query, NOTES).await;
    assert_eq!(
        job["result"]["looks"],
        json!({"verse": "The biblke \u{b7} Slide 1", "point": "Sermon Points \u{b7} Theme Slide"})
    );
    let answer = post_json(
        &router,
        "/api/replan",
        &json!({"job": job["id"], "edits": {}}),
    )
    .await;
    let redo = finished(&router, answer.json()["job"].as_str().unwrap()).await;
    assert_eq!(redo["result"]["looks"], job["result"]["looks"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_thumbnail_route_answers_404_when_nothing_listens() {
    let h = Harness::with_config("new", |c| c.propresenter.port = 9);
    let router = h.router();
    let answer = get(&router, "/api/themethumb?theme=Nope&slide=1").await;
    assert_eq!(answer.status, 404);
    assert!(answer.body.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_job_that_panics_still_ends_with_an_error_and_lets_go_of_the_import_flag() {
    let h = Harness::with("new", |b| {
        b.transport(std::sync::Arc::new(|_| panic!("the transport broke")))
    });
    let router = h.router();
    let answer = upload(&router, "?version=NIV", "n.txt", NOTES.as_bytes()).await;
    let job = finished(&router, answer.json()["job"].as_str().unwrap()).await;
    assert_eq!(job["state"], "error");
    assert!(
        job["error"]
            .as_str()
            .unwrap()
            .starts_with("Unexpected problem: "),
        "{job}"
    );
    let undo = post_json(&router, "/api/undo", &json!({"id": "x"})).await;
    let job = finished(&router, undo.json()["job"].as_str().unwrap()).await;
    assert_eq!(job["state"], "error");
    assert!(!h.state.busy(), "the flag was let go");
}
