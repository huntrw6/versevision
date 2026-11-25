//! Shared by the web tests: a scripted ProPresenter (the S6b oracle harness's `FakePp`, included from `versevision-pp`'s tests), a library on
//! disk made from one of the `oracle/pipeline/scenarios`, a virtual clock (so a 90 second playlist wait takes no time) and calls into the
//! router without a socket.
#![allow(dead_code)]

#[path = "../../../versevision-pp/tests/fake_pp/mod.rs"]
pub mod fake_pp;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderMap, Method, Request};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use versevision_bible::{KeyStore, MemoryKeyStore};
use versevision_core::bibles::InMemoryBible;
use versevision_core::config::AppConfig;
use versevision_pp::clock::VirtualClock;
use versevision_pp::dirs::Dirs;
use versevision_pp::errors::{VvError, VvResult};
use versevision_pp::pipeline::SharedUuids;
use versevision_pp::process::FixedProbe;
use versevision_pp::transport::{LocalTransport, Transport};
use versevision_pp::SeededUuids;
use versevision_web::{BibleHandle, WebState, WebStateBuilder};

use fake_pp::FakePp;

pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub const BASE_EPOCH: i64 = 1_795_000_000;

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// Make the libraries and themes of a scenario under `root`.
fn build_library(spec: &Value, root: &Path) {
    let fixtures = repo().join("tests/fixtures");
    for name in spec["libraries"].as_array().unwrap() {
        fs::create_dir_all(root.join("Libraries").join(name.as_str().unwrap())).unwrap();
    }
    for item in spec["files"].as_array().unwrap() {
        let (kind, rel) = item["source"].as_str().unwrap().split_once(':').unwrap();
        let from = if kind == "fixture" {
            fixtures.join(rel)
        } else {
            repo().join("oracle/pro/files").join(rel)
        };
        let to = root
            .join("Libraries")
            .join(item["library"].as_str().unwrap())
            .join(format!("{}.pro", item["name"].as_str().unwrap()));
        fs::copy(from, to).unwrap();
    }
    match spec.get("themes").and_then(Value::as_str) {
        Some(themes) => copy_dir(
            &fixtures.join(themes.split_once(':').unwrap().1),
            &root.join("Themes"),
        ),
        None => fs::create_dir_all(root.join("Themes")).unwrap(),
    }
}

/// The real local transport, telling the fake ProPresenter what landed in its libraries (the oracle's `Recorder` without the recording).
pub struct Hooked {
    real: LocalTransport,
    fake: Arc<FakePp>,
    gate: Arc<Gate>,
}

/// A door the test can hold shut: a file is not created in the library until it is open (an import that is still running).
pub struct Gate {
    open: std::sync::Mutex<bool>,
    changed: std::sync::Condvar,
}

impl Gate {
    pub fn new() -> Arc<Gate> {
        Arc::new(Gate {
            open: std::sync::Mutex::new(true),
            changed: std::sync::Condvar::new(),
        })
    }
    pub fn set(&self, open: bool) {
        *self.open.lock().unwrap() = open;
        self.changed.notify_all();
    }
    fn pass(&self) {
        let mut open = self.open.lock().unwrap();
        while !*open {
            open = self.changed.wait(open).unwrap();
        }
    }
}

fn lib_name(remote: &str) -> (String, String) {
    let p = Path::new(remote);
    (
        p.parent()
            .and_then(|x| x.file_name())
            .map(|x| x.to_string_lossy().into_owned())
            .unwrap_or_default(),
        p.file_stem()
            .map(|x| x.to_string_lossy().into_owned())
            .unwrap_or_default(),
    )
}

impl Transport for Hooked {
    fn description(&self) -> String {
        self.real.description()
    }
    fn api_get(&self, route: &str) -> VvResult<Value> {
        self.real.api_get(route)
    }
    fn api_put(&self, route: &str, body: &Value) -> VvResult<()> {
        self.real.api_put(route, body)
    }
    fn is_running(&self) -> bool {
        self.real.is_running()
    }
    fn quit_app(&self) -> VvResult<()> {
        self.real.quit_app()
    }
    fn launch_app(&self) -> VvResult<()> {
        self.real.launch_app()
    }
    fn fetch_file(&self, remote_path: &str, local_path: &Path) -> VvResult<String> {
        self.real.fetch_file(remote_path, local_path)
    }
    fn install_file(&self, local_path: &Path, remote_path: &str, sha: &str) -> VvResult<String> {
        self.real.install_file(local_path, remote_path, sha)
    }
    fn find_presentation(&self, name: &str, library: Option<&str>) -> Vec<String> {
        self.real.find_presentation(name, library)
    }
    fn library_folder(&self, library: &str) -> String {
        self.real.library_folder(library)
    }
    fn fetch_themes(&self, dest: &Path) -> VvResult<()> {
        self.real.fetch_themes(dest)
    }
    fn create_file(&self, local_path: &Path, remote_path: &str) -> VvResult<String> {
        self.gate.pass();
        let result = self.real.create_file(local_path, remote_path)?;
        let (lib, name) = lib_name(remote_path);
        self.fake.on_landed(&lib, &name, "create");
        Ok(result)
    }
    fn delete_created_file(&self, remote_path: &str, expected_cues: usize) -> VvResult<String> {
        let result = self.real.delete_created_file(remote_path, expected_cues)?;
        let (lib, name) = lib_name(remote_path);
        self.fake.on_removed(&lib, &name);
        Ok(result)
    }
    fn rename_file(&self, old_path: &str, new_path: &str) -> VvResult<()> {
        self.real.rename_file(old_path, new_path)?;
        let (lib, old) = lib_name(old_path);
        let (_, new) = lib_name(new_path);
        self.fake.on_removed(&lib, &old);
        self.fake.on_landed(&lib, &new, "rename");
        Ok(())
    }
    fn delete_file(&self, remote_path: &str, expected_sha: &str) -> VvResult<String> {
        let result = self.real.delete_file(remote_path, expected_sha)?;
        let (lib, name) = lib_name(remote_path);
        self.fake.on_removed(&lib, &name);
        Ok(result)
    }
    fn backup_file(&self, remote_path: &str) -> VvResult<String> {
        self.real.backup_file(remote_path)
    }
    fn restore_backup(&self, backup_path: &str, remote_path: &str) -> VvResult<()> {
        self.real.restore_backup(backup_path, remote_path)
    }
}

/// Notes the Python web tests use, with verse text for the references that exist.
pub const NOTES: &str =
    "Title 11/8/26\n1) Faith\nJohn 3:16 NIV\nRomans 8:28\nOpen with prayer\nJohn 3:99 NIV\n";

pub fn bible_text() -> InMemoryBible {
    InMemoryBible::from_json(
        &json!({
            "NIV John 3:16": "For God so loved the world that he gave his one and only Son, that whoever believes in him shall not perish but have eternal life.",
            "NIV John 3:17": "For God did not send his Son into the world to condemn the world, but to save the world through him.",
            "NIV Romans 8:28": "And we know that in all things God works for the good of those who love him, who have been called according to his purpose.",
            "NIV Psalms 23:1": "The LORD is my shepherd, I lack nothing.",
        })
        .to_string(),
    )
    .unwrap()
}

pub struct Harness {
    pub root: tempfile::TempDir,
    pub fake: Arc<FakePp>,
    pub clock: Arc<VirtualClock>,
    pub config: AppConfig,
    pub dirs: Dirs,
    pub keys: Arc<MemoryKeyStore>,
    pub gate: Arc<Gate>,
    pub state: WebState,
}

impl Harness {
    /// A library and a fake ProPresenter made from `oracle/pipeline/scenarios/<scenario>`.
    pub fn new(scenario: &str) -> Harness {
        Harness::with(scenario, |builder| builder)
    }

    pub fn with(scenario: &str, tune: impl FnOnce(WebStateBuilder) -> WebStateBuilder) -> Harness {
        Harness::build(scenario, |_| {}, tune)
    }

    /// A key store of the test's own (a file store, to see what lands on disk), with the configuration changed.
    pub fn build_with_keys(
        scenario: &str,
        change: impl FnOnce(&mut AppConfig),
        keys: Arc<dyn KeyStore>,
    ) -> Harness {
        Harness::build(scenario, change, move |builder| builder.keys(keys))
    }

    /// The same, with the configuration changed before the app gets it.
    pub fn with_config(scenario: &str, change: impl FnOnce(&mut AppConfig)) -> Harness {
        Harness::build(scenario, change, |builder| builder)
    }

    fn build(
        scenario: &str,
        change: impl FnOnce(&mut AppConfig),
        tune: impl FnOnce(WebStateBuilder) -> WebStateBuilder,
    ) -> Harness {
        let dir = repo().join("oracle/pipeline/scenarios").join(scenario);
        let spec: Value =
            serde_json::from_str(&fs::read_to_string(dir.join("scenario.json")).unwrap()).unwrap();
        let root = tempfile::tempdir().unwrap();
        let path = root.path().to_path_buf();
        build_library(&spec, &path);
        let clock = Arc::new(VirtualClock::new(BASE_EPOCH, scenario));
        let fake = FakePp::start(&spec, &path, clock.clone());
        let slashed = path.to_string_lossy().replace('\\', "/");
        let text = fs::read_to_string(dir.join("expected/config.toml"))
            .unwrap()
            .replace("<ROOT>", &slashed)
            .replace("port = <PORT>", &format!("port = {}", fake.port));
        let mut config = AppConfig::from_toml_str(&text).unwrap();
        change(&mut config);
        let gate = Gate::new();
        let transport_gate = gate.clone();
        let dirs = Dirs {
            config: path.clone(),
            data: path.join("data"),
            logs: path.join("logs"),
            keys: path.clone(),
        };
        let keys = Arc::new(MemoryKeyStore::new());
        let probe = Arc::new(FixedProbe::new(true));
        let transport_clock = clock.clone();
        let transport_fake = fake.clone();
        let home = path.clone();
        let builder = WebState::builder(dirs.clone(), config.clone())
            .app_version("0.0.0-test")
            .clock(clock.clone())
            .uuids(SharedUuids::new(SeededUuids::new(scenario)))
            .home(home.clone())
            .keys(keys.clone() as Arc<dyn KeyStore>)
            .transport(Arc::new(move |config: &AppConfig| {
                Box::new(Hooked {
                    real: LocalTransport::new(
                        &config.propresenter,
                        &config.node,
                        transport_clock.clone(),
                        probe.clone(),
                        home.clone(),
                    ),
                    fake: transport_fake.clone(),
                    gate: transport_gate.clone(),
                })
            }))
            .bible(Arc::new(|_| BibleHandle::Memory(bible_text())));
        let state = tune(builder).build();
        Harness {
            root,
            fake,
            clock,
            config,
            dirs,
            keys,
            gate,
            state,
        }
    }

    pub fn router(&self) -> Router {
        versevision_web::router(self.state.clone())
    }
}

pub struct Answer {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl Answer {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("not JSON ({e}): {}", String::from_utf8_lossy(&self.body)))
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

pub async fn send(
    router: &Router,
    method: Method,
    uri: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> Answer {
    let mut request = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body: body.collect().await.unwrap().to_bytes().to_vec(),
    }
}

pub async fn get(router: &Router, uri: &str) -> Answer {
    send(router, Method::GET, uri, &[], Vec::new()).await
}

/// A same-origin POST of JSON, as the page makes it.
pub async fn post_json(router: &Router, uri: &str, body: &Value) -> Answer {
    send(
        router,
        Method::POST,
        uri,
        &[
            ("host", "localhost:8777"),
            ("origin", "http://localhost:8777"),
        ],
        body.to_string().into_bytes(),
    )
    .await
}

pub async fn upload(router: &Router, query: &str, name: &str, body: &[u8]) -> Answer {
    send(
        router,
        Method::POST,
        &format!("/api/plan{query}"),
        &[("host", "localhost:8777"), ("x-file-name", name)],
        body.to_vec(),
    )
    .await
}

/// Poll a job until it stops running (a real second at most 60 times: the work is on a blocking thread).
pub async fn finished(router: &Router, job: &str) -> Value {
    finished_under(router, "", job).await
}

/// The same for a router mounted under `prefix` (`"/versevision"`).
pub async fn finished_under(router: &Router, prefix: &str, job: &str) -> Value {
    for _ in 0..1200 {
        let answer = get(router, &format!("{prefix}/api/jobs/{job}")).await;
        let state = answer.json();
        if state["state"] != "running" {
            return state;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("job {job} did not finish");
}

pub fn unused(_: VvError) {}
