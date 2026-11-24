//! The ProPresenter side against the oracle (`oracle/pipeline/`): every scenario is run through the Rust port against the scripted
//! ProPresenter (`tests/fake_pp`) with virtual time and seeded ids, and what comes out must be what the Python app did: the answers of each
//! step, every HTTP request (virtual time, method, path, body, status), every file operation, the files left behind, `imports.json`, the audit
//! log and every file of every run folder.
//!
//! `VV_ORACLE_BLESS_DIFF=1` prints every difference instead of stopping at the first scenario that has one.

mod fake_pp;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use fake_pp::FakePp;
use serde_json::{json, Map, Value};
use versevision_core::bibles::InMemoryBible;
use versevision_core::config::AppConfig;
use versevision_core::notes::title::document_title;
use versevision_core::plan::ReviewChoices;
use versevision_pp::api_client::ProPresenterApiClient;
use versevision_pp::clock::{Clock, VirtualClock};
use versevision_pp::dirs::Dirs;
use versevision_pp::errors::{VvError, VvResult};
use versevision_pp::explain::{explain_data, render_explain, render_replay, replay_data};
use versevision_pp::history::records_json;
use versevision_pp::pipeline::{
    plan_for_transport, ApplyResult, Ctx, Planned, SharedUuids, Target,
};
use versevision_pp::process::FixedProbe;
use versevision_pp::run::{
    apply_reviewed, history_list, report_value, run_import, run_undo, ReviewedImport, RunOutcome,
    RunRequest,
};
use versevision_pp::runlog::{list_runs, RunLog};
use versevision_pp::selection::select_unique_presentation;
use versevision_pp::transport::{sha256_file, LocalTransport, Transport};
use versevision_pp::SeededUuids;

const BASE_EPOCH: i64 = 1784419571;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// The path with `/` as the separator.
fn slashed(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

struct Norm {
    root: String,
    port: u16,
}

impl Norm {
    fn text(&self, text: &str) -> String {
        let mut text = text.to_string();
        if text.replace('\\', "/").contains(&self.root) {
            text = text.replace('\\', "/");
        }
        text.replace(&self.root, "<ROOT>")
            .replace(&format!("127.0.0.1:{}", self.port), "127.0.0.1:<PORT>")
    }
    fn value(&self, value: &Value) -> Value {
        match value {
            Value::String(s) => Value::String(self.text(s)),
            Value::Array(a) => Value::Array(a.iter().map(|v| self.value(v)).collect()),
            Value::Object(o) => {
                Value::Object(o.iter().map(|(k, v)| (k.clone(), self.value(v))).collect())
            }
            other => other.clone(),
        }
    }
}

/// `Proxy` of the Python oracle: the real local transport, recording every file operation and telling the fake ProPresenter what landed.
struct Recorder {
    real: LocalTransport,
    fake: Arc<FakePp>,
    clock: Arc<VirtualClock>,
    root: String,
    hooks: Mutex<Vec<(String, String, bool)>>,
    ops: Mutex<Vec<Value>>,
}

impl Recorder {
    fn rel(&self, text: &str) -> String {
        let text = text.replace('\\', "/");
        match text.strip_prefix(&self.root) {
            Some(rest) => format!("<ROOT>{rest}"),
            None => format!("<local>/{}", text.rsplit('/').next().unwrap_or("")),
        }
    }

    fn note(&self, op: &str, args: Vec<Value>, result: Option<Value>, error: Option<&VvError>) {
        let mut entry = json!({"t": round3(self.clock.elapsed()), "op": op, "args": args});
        if let Some(e) = error {
            entry["error"] = json!(e.python_form());
        } else if let Some(r) = result {
            entry["result"] = r;
        }
        self.ops.lock().unwrap().push(entry);
    }

    fn call<T>(
        &self,
        op: &str,
        args: Vec<Value>,
        result: VvResult<T>,
        show: impl Fn(&T) -> Option<Value>,
    ) -> VvResult<T> {
        match &result {
            Ok(value) => self.note(op, args, show(value), None),
            Err(e) => self.note(op, args, None, Some(e)),
        }
        result
    }

    fn hook(&self, op: &str, path: &str) {
        let mut hooks = self.hooks.lock().unwrap();
        for (after, source, done) in hooks.iter_mut() {
            if after == op && !*done {
                *done = true;
                let rel = source.split_once(':').unwrap().1;
                let data = fs::read(repo().join("python/tests/fixtures").join(rel)).unwrap();
                fs::write(path, data).unwrap();
                self.ops.lock().unwrap().push(json!({
                    "t": round3(self.clock.elapsed()), "op": "hook:overwrite", "args": [self.rel(path), source]
                }));
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
}

impl Transport for Recorder {
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
        let r = self.real.is_running();
        self.note("is_running", vec![], Some(json!(r)), None);
        r
    }
    fn quit_app(&self) -> VvResult<()> {
        self.call("quit_app", vec![], self.real.quit_app(), |_| None)
    }
    fn launch_app(&self) -> VvResult<()> {
        self.call("launch_app", vec![], self.real.launch_app(), |_| None)
    }
    fn fetch_file(&self, remote_path: &str, local_path: &Path) -> VvResult<String> {
        self.call(
            "fetch_file",
            vec![json!(self.rel(remote_path))],
            self.real.fetch_file(remote_path, local_path),
            |s| Some(json!(s)),
        )
    }
    fn install_file(
        &self,
        local_path: &Path,
        remote_path: &str,
        expected_remote_sha: &str,
    ) -> VvResult<String> {
        self.call(
            "install_file",
            vec![json!(self.rel(remote_path))],
            self.real
                .install_file(local_path, remote_path, expected_remote_sha),
            |s| Some(json!(s)),
        )
    }
    fn find_presentation(&self, name: &str, library: Option<&str>) -> Vec<String> {
        let found = self.real.find_presentation(name, library);
        let shown: Vec<String> = found.iter().map(|f| self.rel(f)).collect();
        self.note(
            "find_presentation",
            vec![json!(name), json!(library)],
            Some(json!(shown)),
            None,
        );
        found
    }
    fn library_folder(&self, library: &str) -> String {
        self.real.library_folder(library)
    }
    fn fetch_themes(&self, dest: &Path) -> VvResult<()> {
        self.call("fetch_themes", vec![], self.real.fetch_themes(dest), |_| {
            None
        })
    }
    fn create_file(&self, local_path: &Path, remote_path: &str) -> VvResult<String> {
        let result = self.real.create_file(local_path, remote_path);
        if result.is_ok() {
            let (lib, name) = Self::lib_name(remote_path);
            self.fake.on_landed(&lib, &name, "create");
            self.hook("create_file", remote_path);
        }
        self.call(
            "create_file",
            vec![json!(self.rel(remote_path))],
            result,
            |s| Some(json!(s)),
        )
    }
    fn delete_created_file(&self, remote_path: &str, expected_cues: usize) -> VvResult<String> {
        let result = self.real.delete_created_file(remote_path, expected_cues);
        if result.is_ok() {
            let (lib, name) = Self::lib_name(remote_path);
            self.fake.on_removed(&lib, &name);
        }
        self.call(
            "delete_created_file",
            vec![json!(self.rel(remote_path)), json!(expected_cues)],
            result,
            |s| Some(json!(s)),
        )
    }
    fn rename_file(&self, old_path: &str, new_path: &str) -> VvResult<()> {
        let result = self.real.rename_file(old_path, new_path);
        if result.is_ok() {
            let (lib, old) = Self::lib_name(old_path);
            let (_, new) = Self::lib_name(new_path);
            self.fake.on_removed(&lib, &old);
            self.fake.on_landed(&lib, &new, "rename");
            self.hook("rename_file", new_path);
        }
        self.call(
            "rename_file",
            vec![json!(self.rel(old_path)), json!(self.rel(new_path))],
            result,
            |_| None,
        )
    }
    fn delete_file(&self, remote_path: &str, expected_sha: &str) -> VvResult<String> {
        let result = self.real.delete_file(remote_path, expected_sha);
        if result.is_ok() {
            let (lib, name) = Self::lib_name(remote_path);
            self.fake.on_removed(&lib, &name);
        }
        self.call(
            "delete_file",
            vec![json!(self.rel(remote_path))],
            result,
            |s| Some(json!(s)),
        )
    }
    fn backup_file(&self, remote_path: &str) -> VvResult<String> {
        self.call(
            "backup_file",
            vec![json!(self.rel(remote_path))],
            self.real.backup_file(remote_path),
            |s| Some(json!(self.rel(s))),
        )
    }
    fn restore_backup(&self, backup_path: &str, remote_path: &str) -> VvResult<()> {
        self.call(
            "restore_backup",
            vec![json!(self.rel(backup_path)), json!(self.rel(remote_path))],
            self.real.restore_backup(backup_path, remote_path),
            |_| None,
        )
    }
}

fn str_list(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn opt(step: &Value, key: &str) -> Option<String> {
    step.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn flag(step: &Value, key: &str) -> bool {
    step.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn failure(error: &VvError) -> Value {
    json!({"ok": false, "error": error.message, "exit_code": error.exit_code() as u8})
}

/// What `--output json` printed.
fn stdout_of(outcome: &RunOutcome) -> Value {
    if let Some(error) = &outcome.error {
        return failure(error);
    }
    outcome.report.clone().unwrap_or(Value::Null)
}

/// What the review page holds between the plan and the apply.
struct WebReview {
    plan: Planned,
    target: Option<Target>,
    report: Value,
    text: String,
    name: String,
    title: String,
    mode: String,
    run_id: String,
}

struct Scenario {
    spec: Value,
    root: PathBuf,
    norm: Norm,
    clock: Arc<VirtualClock>,
    probe: Arc<FixedProbe>,
    fake: Arc<FakePp>,
    recorder: Arc<Recorder>,
    config: AppConfig,
    ctx: Ctx,
    bible: InMemoryBible,
    web: Option<WebReview>,
}

fn build_library(spec: &Value, root: &Path) {
    let fixtures = repo().join("python/tests/fixtures");
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

impl Scenario {
    fn new(dir: &Path, scratch: &Path) -> Scenario {
        let spec: Value =
            serde_json::from_str(&fs::read_to_string(dir.join("scenario.json")).unwrap()).unwrap();
        let seed = dir.file_name().unwrap().to_string_lossy().into_owned();
        let seed = spec
            .get("seed")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or(seed);
        let root = scratch.to_path_buf();
        let root_text = slashed(&root);
        build_library(&spec, &root);
        let clock = Arc::new(VirtualClock::new(BASE_EPOCH, &seed));
        let fake = FakePp::start(&spec, &root, clock.clone());
        let expected = dir.join("expected");
        let config_text = fs::read_to_string(expected.join("config.toml"))
            .unwrap()
            .replace("<ROOT>", &root_text)
            .replace("port = <PORT>", &format!("port = {}", fake.port));
        let config = AppConfig::from_toml_str(&config_text)
            .unwrap_or_else(|e| panic!("config.toml of {}: {e}", dir.display()));
        let probe = Arc::new(FixedProbe::new(
            spec.get("running").and_then(Value::as_bool).unwrap_or(true),
        ));
        let real = LocalTransport::new(
            &config.propresenter,
            &config.node,
            clock.clone(),
            probe.clone(),
            root.clone(),
        );
        let hooks = spec
            .get("hooks")
            .and_then(Value::as_array)
            .map(|h| {
                h.iter()
                    .map(|x| {
                        (
                            x["after"].as_str().unwrap().to_string(),
                            x["overwrite_with"].as_str().unwrap().to_string(),
                            false,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let recorder = Arc::new(Recorder {
            real,
            fake: fake.clone(),
            clock: clock.clone(),
            root: root_text.clone(),
            hooks: Mutex::new(hooks),
            ops: Mutex::new(Vec::new()),
        });
        let dirs = Dirs {
            config: root.clone(),
            data: root.join("data"),
            logs: root.join("logs"),
            keys: root.clone(),
        };
        let mut ctx = Ctx::new(
            clock.clone(),
            SharedUuids::new(SeededUuids::new(&seed)),
            dirs,
            "0.38.1",
        );
        if let Some(timing) = spec.get("timing").and_then(Value::as_object) {
            for (name, value) in timing {
                let v = value.as_f64().unwrap();
                match name.as_str() {
                    "MIN_GAP_SECONDS" => ctx.timing.min_gap = v,
                    "QUIET_SECONDS" => ctx.timing.quiet = v,
                    "PLAYLIST_CONVERGE_SECONDS" => ctx.timing.playlist_converge = v,
                    "LOAD_GRACE_SECONDS" => ctx.timing.load_grace = v,
                    "STABLE_SECONDS" => ctx.timing.stable = v,
                    "SETTLE_SECONDS" => ctx.timing.settle = v,
                    "SWAP_WAIT_SECONDS" => ctx.timing.swap_wait = v,
                    other => panic!("unknown timing {other}"),
                }
            }
        }
        let bible =
            InMemoryBible::from_json(&fs::read_to_string(expected.join("bible.json")).unwrap())
                .unwrap();
        let norm = Norm {
            root: root_text,
            port: fake.port,
        };
        Scenario {
            spec,
            root,
            norm,
            clock,
            probe,
            fake,
            recorder,
            config,
            ctx,
            bible,
            web: None,
        }
    }

    fn runs_sorted(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.root.join("logs/runs"))
            .map(|r| {
                r.flatten()
                    .filter(|e| e.path().is_dir())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    fn run_step(&mut self, step: &Value, index: usize, answers: &[Value]) -> Value {
        let op = step["op"].as_str().unwrap();
        let mut answer = json!({"op": op});
        match op {
            "advance" => self.clock.sleep(step["seconds"].as_f64().unwrap()),
            "running" => self.probe.set(step["value"].as_bool().unwrap()),
            "server" => self.fake.set_server(step["set"].as_object().unwrap()),
            "write_file" => {
                let rel = step["source"].as_str().unwrap().split_once(':').unwrap().1;
                let to = self
                    .root
                    .join("Libraries")
                    .join(step["library"].as_str().unwrap())
                    .join(format!("{}.pro", step["name"].as_str().unwrap()));
                fs::copy(repo().join("python/tests/fixtures").join(rel), to).unwrap();
            }
            "seed_runs" => {
                let folder = self.root.join("logs/runs");
                for n in 0..step["count"].as_u64().unwrap() {
                    fs::create_dir_all(folder.join(format!(
                        "20200101-{:02}{:02}{:02}-aaaaaa",
                        n / 3600,
                        n / 60 % 60,
                        n % 60
                    )))
                    .unwrap();
                }
            }
            "run" => {
                let file = step
                    .get("file")
                    .and_then(Value::as_str)
                    .unwrap_or("notes.txt")
                    .to_string();
                let text = step["text"].as_str().unwrap();
                fs::write(self.root.join(&file), text).unwrap();
                let (source, info) =
                    versevision_docs::extract_text(text.as_bytes(), &file).unwrap();
                let extract = match info.to_json() {
                    Value::Object(m) => Some(m),
                    _ => None,
                };
                let request = RunRequest {
                    source,
                    source_name: file,
                    extract,
                    presentation: opt(step, "presentation"),
                    presentation_uuid: opt(step, "presentation_uuid"),
                    title: opt(step, "title"),
                    library: opt(step, "library"),
                    replace: flag(step, "replace"),
                    append: flag(step, "append"),
                    default_version: opt(step, "default_version"),
                    dry_run: flag(step, "dry_run"),
                    yes: flag(step, "yes"),
                    no_playlist: flag(step, "no_playlist"),
                    as_copy: flag(step, "as_copy"),
                    in_place: flag(step, "in_place"),
                    no_close: flag(step, "no_close"),
                    no_reopen: flag(step, "no_reopen"),
                    profile: opt(step, "profile"),
                    add_line: step
                        .get("add_line")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().map(|n| n.as_u64().unwrap() as usize).collect())
                        .unwrap_or_default(),
                    set_options: str_list(step, "set"),
                };
                let outcome = pollster::block_on(run_import(
                    &mut self.ctx,
                    &request,
                    &self.config,
                    self.recorder.as_ref(),
                    Some(&self.bible),
                ));
                answer["exit_code"] = json!(outcome.exit_code);
                answer["stdout"] = stdout_of(&outcome);
            }
            "undo" => {
                let id = match step.get("import_id").and_then(Value::as_str) {
                    Some(id) => id.to_string(),
                    None => answers[step["from_step"].as_u64().unwrap() as usize]["stdout"]
                        ["applied"]["import_id"]
                        .as_str()
                        .unwrap()
                        .to_string(),
                };
                let outcome = run_undo(
                    &mut self.ctx,
                    &id,
                    &self.config,
                    self.recorder.as_ref(),
                    flag(step, "no_close"),
                );
                answer["exit_code"] = json!(outcome.exit_code);
                answer["stdout"] = match (&outcome.error, &outcome.applied) {
                    (Some(e), _) => failure(e),
                    (None, Some(applied)) => {
                        let mut v = json!({"ok": true});
                        v.as_object_mut()
                            .unwrap()
                            .extend(applied.to_value().as_object().unwrap().clone());
                        v
                    }
                    _ => Value::Null,
                };
            }
            "history" => {
                answer["exit_code"] = json!(0);
                answer["stdout"] = records_json(&history_list(&self.ctx));
            }
            "log_list" => {
                answer["exit_code"] = json!(0);
                answer["stdout"] = Value::Array(list_runs(
                    &self.ctx.dirs.runs_dir(),
                    step.get("count").and_then(Value::as_u64).unwrap_or(20) as usize,
                ));
            }
            "explain" | "replay" => {
                let names = self.runs_sorted();
                let rid = match step.get("run_index").and_then(Value::as_u64) {
                    Some(i) => names[i as usize].clone(),
                    None => step
                        .get("run")
                        .and_then(Value::as_str)
                        .unwrap_or("latest")
                        .to_string(),
                };
                let runs = self.ctx.dirs.runs_dir();
                let line = step.get("line").and_then(Value::as_i64);
                let tier = opt(step, "tier");
                if op == "explain" {
                    let result = if flag(step, "json") {
                        explain_data(&runs, &rid, line, tier.as_deref())
                    } else {
                        render_explain(&runs, &rid, line, tier.as_deref(), 150)
                            .map(|t| json!({"text": format!("{t}\n")}))
                    };
                    match result {
                        Ok(v) => {
                            answer["exit_code"] = json!(0);
                            answer["stdout"] = v;
                        }
                        Err(_) => {
                            answer["exit_code"] = json!(1);
                            answer["stdout"] = json!({"text": ""});
                        }
                    }
                } else {
                    match replay_data(&runs, &self.config, &rid, !flag(step, "todays_options")) {
                        Ok(d) => {
                            answer["exit_code"] = json!(0);
                            answer["stdout"] = json!({"text": format!("{}\n", render_replay(&d))});
                        }
                        Err(e) => {
                            answer["exit_code"] = json!(1);
                            answer["stdout"] = json!({"text": format!("{rid}: {}\n", e.message)});
                        }
                    }
                }
            }
            "client" => {
                let pp = &self.config.propresenter;
                let client = ProPresenterApiClient::new(&pp.host, pp.port, pp.timeout_seconds);
                let mut out = Map::new();
                for call in step["calls"].as_array().unwrap() {
                    let call = call.as_str().unwrap();
                    let value = (|| -> VvResult<Value> {
                        let dump = |v: &dyn erased::Dump| v.dump();
                        Ok(match call {
                            "version" => dump(&client.version()?),
                            "libraries" => dump(&client.libraries()?),
                            "presentations" => dump(&client.presentations()?),
                            "playlists" => dump(&client.playlists()?),
                            c if c.starts_with("presentation:") => {
                                dump(&client.presentation(&c["presentation:".len()..])?)
                            }
                            c if c.starts_with("playlist:") => {
                                dump(&client.playlist(&c["playlist:".len()..])?)
                            }
                            c if c.starts_with("select:") => dump(&select_unique_presentation(
                                &client.presentations()?,
                                None,
                                Some(&c["select:".len()..]),
                                None,
                            )?),
                            _ => Value::Null,
                        })
                    })();
                    out.insert(
                        call.to_string(),
                        value.unwrap_or_else(|e| json!({"error": e.python_form()})),
                    );
                }
                answer["calls"] = Value::Object(out);
            }
            "web_plan" => self.web_plan(step, &mut answer),
            "web_apply" => self.web_apply(step, &mut answer),
            other => panic!("unknown step {other} (step {index})"),
        }
        answer["t"] = json!(round3(self.clock.elapsed()));
        answer
    }

    fn web_plan(&mut self, step: &Value, answer: &mut Value) {
        let name = step
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("notes.txt")
            .to_string();
        let text = step["text"].as_str().unwrap().to_string();
        let mode = step["mode"].as_str().unwrap().to_string();
        let default_version =
            opt(step, "default_version").or_else(|| self.config.bible.default_version.clone());
        let mut meta = Map::new();
        meta.insert("source".into(), json!(name));
        meta.insert("mode".into(), json!(mode));
        meta.insert("title".into(), Value::Null);
        meta.insert("library".into(), Value::Null);
        meta.insert("target".into(), Value::Null);
        meta.insert(
            "node".into(),
            json!(self
                .config
                .node
                .name
                .clone()
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| "this computer".into())),
        );
        meta.insert("default_version".into(), json!(default_version));
        let mut run =
            RunLog::start(&self.ctx.dirs, self.clock.clone(), "0.38.1", "web", meta).unwrap();
        run.save_source(&text, &name);
        let run_id = run.id.clone();
        self.ctx.run = Some(run);
        let target_uuid = opt(step, "target");
        let (plan, target) = pollster::block_on(plan_for_transport(
            &mut self.ctx,
            &text,
            &self.config,
            self.recorder.as_ref(),
            opt(step, "default_version").as_deref(),
            target_uuid.as_deref(),
            None,
            None,
            false,
            Some(&self.bible),
        ))
        .unwrap();
        let title = document_title(&text, &name).unwrap();
        let mut report = report_value(&plan, &name);
        report["mode"] = json!(mode);
        report["title"] = json!(title);
        report["library"] = json!(self.config.output.library);
        report["target"] = match &target {
            Some(t) => json!({"name": t.name, "library": t.library, "uuid": t.uuid}),
            None => Value::Null,
        };
        report["node"] = json!(self.recorder.description());
        report["replaces"] = Value::Null;
        report["run_id"] = json!(run_id);
        let mut run = self.ctx.run.take().unwrap();
        run.save_report(&report);
        run.finish("reviewed, not imported yet", Some(&report), None, None);
        answer["run_id"] = json!(run_id);
        answer["report_summary"] = report["summary"].clone();
        answer["decisions"] = Value::Array(
            report["decisions"]
                .as_array()
                .unwrap()
                .iter()
                .map(|d| json!([d["id"], d["line"], d["kind"], d["tier"]]))
                .collect(),
        );
        self.web = Some(WebReview {
            plan,
            target,
            report,
            text,
            name,
            title,
            mode,
            run_id,
        });
    }

    fn web_apply(&mut self, step: &Value, answer: &mut Value) {
        let WebReview {
            plan,
            target,
            report,
            text,
            name,
            title,
            mode,
            run_id,
        } = self.web.take().expect("web_apply needs a web_plan");
        let ids = |key: &str| -> std::collections::BTreeSet<usize> {
            step.get(key)
                .and_then(Value::as_array)
                .map(|a| a.iter().map(|n| n.as_u64().unwrap() as usize).collect())
                .unwrap_or_default()
        };
        let choices = ReviewChoices {
            skip: ids("skip"),
            include: ids("include"),
            reject_fixes: ids("reject_fixes"),
        };
        let accept = step
            .get("accept_flags")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let review = ReviewedImport {
            text: &text,
            source_name: &name,
            mode: &mode,
            title: &title,
            library: None,
            plan: &plan,
            target: target.as_ref(),
            report: &report,
            run_id: Some(&run_id),
        };
        let outcome = apply_reviewed(
            &mut self.ctx,
            &review,
            accept,
            &choices,
            &self.config,
            self.recorder.as_ref(),
        );
        answer["outcome"] = json!(if outcome.error.is_none() {
            "ok"
        } else {
            "failed"
        });
        answer["error"] = json!(outcome.error.as_ref().map(|e| e.message.clone()));
        answer["applied"] = outcome
            .applied
            .as_ref()
            .map(ApplyResult::to_value)
            .unwrap_or(Value::Null);
    }

    fn files(&self) -> Value {
        let base = self.root.join("Libraries");
        let mut out = BTreeMap::new();
        fn walk(dir: &Path, base: &Path, out: &mut BTreeMap<String, String>) {
            for entry in fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, base, out);
                } else {
                    let name = path.file_name().unwrap().to_string_lossy().into_owned();
                    if name.ends_with(".lock") || name.ends_with(".tmp") {
                        continue;
                    }
                    let rel = slashed(path.strip_prefix(base).unwrap());
                    out.insert(
                        format!("<ROOT>/Libraries/{rel}"),
                        sha256_file(&path).unwrap(),
                    );
                }
            }
        }
        walk(&base, &base, &mut out);
        json!(out)
    }
}

mod erased {
    use serde::Serialize;
    use serde_json::Value;

    pub trait Dump {
        fn dump(&self) -> Value;
    }
    impl<T: Serialize> Dump for T {
        fn dump(&self) -> Value {
            serde_json::to_value(self).unwrap()
        }
    }
}

// ------------------------------------------------------------------------------------------------------------ comparing

fn diff(path: &str, expected: &Value, got: &Value, out: &mut Vec<String>) {
    if out.len() >= 12 || expected == got {
        return;
    }
    match (expected, got) {
        (Value::Object(a), Value::Object(b)) => {
            for key in a.keys().chain(b.keys().filter(|k| !a.contains_key(*k))) {
                diff(
                    &format!("{path}.{key}"),
                    a.get(key).unwrap_or(&Value::Null),
                    b.get(key).unwrap_or(&Value::Null),
                    out,
                );
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            if a.len() != b.len() {
                out.push(format!(
                    "{path}: {} items expected, {} got",
                    a.len(),
                    b.len()
                ));
            }
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                diff(&format!("{path}[{i}]"), x, y, out);
            }
        }
        _ => out.push(format!(
            "{path}:\n   expected {}\n   got      {}",
            short(expected),
            short(got)
        )),
    }
}

fn short(v: &Value) -> String {
    let s = v.to_string();
    if s.len() > 400 {
        format!("{}...", &s[..400])
    } else {
        s
    }
}

fn text_diff(label: &str, expected: &str, got: &str, out: &mut Vec<String>) {
    if expected == got {
        return;
    }
    let (a, b): (Vec<&str>, Vec<&str>) = (expected.lines().collect(), got.lines().collect());
    for i in 0..a.len().max(b.len()) {
        if a.get(i) != b.get(i) {
            out.push(format!(
                "{label} line {}:\n   expected {:?}\n   got      {:?}",
                i + 1,
                a.get(i),
                b.get(i)
            ));
            return;
        }
    }
}

fn jsonl(text: &str) -> Vec<Value> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or(Value::String(l.to_string())))
        .collect()
}

/// A traceback has the machine's paths in it; Rust records the exception's name and message instead of a stack. The oracle keeps that
/// there was one.
fn without_stacks(mut events: Vec<Value>) -> Vec<Value> {
    for event in &mut events {
        if event.get("traceback").is_some_and(Value::is_string) {
            event["traceback"] = json!("<traceback>");
        }
    }
    events
}

/// (steps, requests, file operations, run folders) compared in one scenario.
static COUNTS: [std::sync::atomic::AtomicUsize; 4] = [
    std::sync::atomic::AtomicUsize::new(0),
    std::sync::atomic::AtomicUsize::new(0),
    std::sync::atomic::AtomicUsize::new(0),
    std::sync::atomic::AtomicUsize::new(0),
];

fn run_scenario(dir: &Path) -> Vec<String> {
    let scratch = tempfile::tempdir().unwrap();
    let mut sc = Scenario::new(dir, scratch.path());
    let steps = sc.spec["steps"].as_array().unwrap().clone();
    let mut answers: Vec<Value> = Vec::new();
    for (i, step) in steps.iter().enumerate() {
        let answer = sc.run_step(step, i, &answers);
        answers.push(answer);
    }
    let expected = dir.join("expected");
    let norm = &sc.norm;
    let result: Value =
        serde_json::from_str(&fs::read_to_string(expected.join("result.json")).unwrap()).unwrap();
    let got = json!({
        "steps": norm.value(&Value::Array(answers.clone())),
        "requests": norm.value(&Value::Array(sc.fake.log())),
        "file_ops": norm.value(&Value::Array(sc.recorder.ops.lock().unwrap().clone())),
        "files": norm.value(&sc.files()),
        "running": sc.probe.0.load(std::sync::atomic::Ordering::SeqCst),
        "playlist": sc.fake.playlist_items(),
    });
    for (n, key) in ["steps", "requests", "file_ops"].iter().enumerate() {
        COUNTS[n].fetch_add(
            got[*key].as_array().map_or(0, Vec::len),
            std::sync::atomic::Ordering::SeqCst,
        );
    }
    COUNTS[3].fetch_add(sc.runs_sorted().len(), std::sync::atomic::Ordering::SeqCst);
    let mut problems = Vec::new();
    for key in [
        "steps", "requests", "file_ops", "files", "running", "playlist",
    ] {
        diff(key, &result[key], &got[key], &mut problems);
    }
    // imports.json (compared as text where the system keeps the separators: it must be byte for byte what Python writes)
    let history = fs::read_to_string(sc.root.join("data/imports.json")).unwrap_or_default();
    let want = fs::read_to_string(expected.join("history.json")).unwrap();
    let got_history = norm.text(&history);
    if cfg!(windows) {
        diff(
            "history.json",
            &json!(jsonl_one(&want)),
            &norm.value(&jsonl_one(&history)),
            &mut problems,
        );
    } else {
        text_diff("history.json", &want, &got_history, &mut problems);
    }
    let audit = fs::read_to_string(sc.root.join("logs/audit.jsonl")).unwrap_or_default();
    diff(
        "audit.jsonl",
        &Value::Array(jsonl(
            &fs::read_to_string(expected.join("audit.jsonl")).unwrap(),
        )),
        &norm.value(&Value::Array(jsonl(&audit))),
        &mut problems,
    );
    let runs = sc.runs_sorted();
    let want_runs: Vec<String> = fs::read_dir(expected.join("runs"))
        .map(|r| {
            r.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    let mut want_runs = want_runs;
    want_runs.sort();
    if want_runs.len() != runs.len() {
        problems.push(format!(
            "runs: {} folders expected, {} made",
            want_runs.len(),
            runs.len()
        ));
    }
    for (number, folder) in want_runs.iter().zip(&runs) {
        let want_dir = expected.join("runs").join(number);
        let got_dir = sc.root.join("logs/runs").join(folder);
        text_diff(
            &format!("runs/{number}/name"),
            &fs::read_to_string(want_dir.join("name")).unwrap(),
            folder,
            &mut problems,
        );
        let mut files: Vec<String> = fs::read_dir(&want_dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "name")
            .collect();
        files.sort();
        let mut have: Vec<String> = fs::read_dir(&got_dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        have.sort();
        if files != have {
            problems.push(format!(
                "runs/{number}: files expected {files:?}, got {have:?}"
            ));
        }
        for file in &files {
            let want = fs::read_to_string(want_dir.join(file)).unwrap();
            let got = fs::read_to_string(got_dir.join(file)).unwrap_or_default();
            let label = format!("runs/{number}/{file}");
            if file.ends_with(".jsonl") {
                diff(
                    &label,
                    &Value::Array(without_stacks(jsonl(&want))),
                    &norm.value(&Value::Array(without_stacks(jsonl(&got)))),
                    &mut problems,
                );
            } else if file.ends_with(".json") {
                let w: Value = serde_json::from_str(&want).unwrap();
                let g: Value = serde_json::from_str(&got).unwrap_or(Value::Null);
                diff(&label, &w, &norm.value(&g), &mut problems);
            } else {
                text_diff(&label, &want, &norm.text(&got), &mut problems);
            }
        }
    }
    problems
}

fn jsonl_one(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or(Value::Null)
}

fn scenarios() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(repo().join("oracle/pipeline/scenarios"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("scenario.json").is_file())
        .collect();
    dirs.sort();
    dirs
}

#[test]
fn every_scenario_matches_what_python_did() {
    let mut failures = Vec::new();
    let all = scenarios();
    assert!(all.len() >= 25, "the scenarios are missing");
    for dir in &all {
        let problems = run_scenario(dir);
        if !problems.is_empty() {
            failures.push(format!(
                "== {}\n{}",
                dir.file_name().unwrap().to_string_lossy(),
                problems.join("\n")
            ));
        }
    }
    let n = |i: usize| COUNTS[i].load(std::sync::atomic::Ordering::SeqCst);
    println!(
        "pipeline oracle: {} scenarios, {} steps, {} HTTP requests, {} file operations, {} run folders match what Python did",
        all.len(),
        n(0),
        n(1),
        n(2),
        n(3)
    );
    assert!(failures.is_empty(), "\n{}", failures.join("\n\n"));
}
