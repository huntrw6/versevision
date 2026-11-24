//! The scripted ProPresenter of `oracle/pipeline/` (Rust twin of `FakePP` in `python/scripts/oracle_pipeline.py`; the rules are written down
//! in `oracle/pipeline/README.md`). All times are virtual seconds read from the clock the pipeline sleeps on.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{json, Map, Value};
use versevision_pp::clock::VirtualClock;
use versevision_pp::model::{element, elements_of_action, has_text, read_presentation};
use versevision_pp::rtf::plain_rtf;

#[derive(Clone)]
struct Entry {
    library: String,
    name: String,
    ids: Vec<(f64, String)>,
    listed_from: f64,
    blank_until: f64,
    loaded_from: Option<f64>,
}

impl Entry {
    fn current(&self, t: f64) -> String {
        let mut found = String::new();
        for (start, ident) in &self.ids {
            if *start <= t {
                found = ident.clone();
            }
        }
        found
    }
}

struct State {
    cfg: Map<String, Value>,
    libraries: Vec<Value>,
    entries: Vec<((String, String), Entry)>,
    next_id: u64,
    next_item: u64,
    log: Vec<Value>,
    playlist: Option<Value>,
}

pub struct FakePp {
    state: Mutex<State>,
    clock: Arc<VirtualClock>,
    root: PathBuf,
    pub port: u16,
}

fn num(v: &Value, key: &str, default: f64) -> f64 {
    v.get(key).and_then(Value::as_f64).unwrap_or(default)
}

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

fn unquote(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&text[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn last_segment(path: &str) -> String {
    unquote(path.rsplit('/').next().unwrap_or(""))
}

impl FakePp {
    /// `scenario` is the parsed `scenario.json`.
    pub fn start(scenario: &Value, root: &Path, clock: Arc<VirtualClock>) -> Arc<FakePp> {
        let cfg = scenario
            .get("server")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let libraries: Vec<Value> = scenario["libraries"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(i, name)| json!({"uuid": format!("L{}", i + 1), "name": name, "index": i}))
            .collect();
        let mut entries = Vec::new();
        for item in scenario["files"].as_array().unwrap() {
            let library = item["library"].as_str().unwrap().to_string();
            let name = item["name"].as_str().unwrap().to_string();
            entries.push((
                (library.clone(), name.clone()),
                Entry {
                    library,
                    name,
                    ids: vec![(-1e9, item["id"].as_str().unwrap().to_string())],
                    listed_from: -1e9,
                    blank_until: -1e9,
                    loaded_from: Some(-1e9),
                },
            ));
        }
        let playlist = scenario.get("playlist").filter(|p| !p.is_null()).cloned();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the fake ProPresenter");
        let port = listener.local_addr().unwrap().port();
        let fake = Arc::new(FakePp {
            state: Mutex::new(State {
                cfg,
                libraries,
                entries,
                next_id: 0,
                next_item: 0,
                log: Vec::new(),
                playlist,
            }),
            clock,
            root: root.to_path_buf(),
            port,
        });
        let server = fake.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let fake = server.clone();
                std::thread::spawn(move || fake.handle(stream));
            }
        });
        fake
    }

    pub fn log(&self) -> Vec<Value> {
        self.state.lock().unwrap().log.clone()
    }

    pub fn playlist_items(&self) -> Value {
        match &self.state.lock().unwrap().playlist {
            Some(p) => p["items"].clone(),
            None => Value::Null,
        }
    }

    pub fn set_server(&self, values: &Map<String, Value>) {
        let mut st = self.state.lock().unwrap();
        for (k, v) in values {
            st.cfg.insert(k.clone(), v.clone());
        }
    }

    fn landing(st: &State, name: &str, kind: &str) -> Value {
        let mut spec = st.cfg.get("landing").cloned().unwrap_or_else(|| json!({}));
        let mut merge = |extra: Option<&Value>| {
            if let (Some(Value::Object(e)), Value::Object(s)) = (extra, &mut spec) {
                for (k, v) in e {
                    s.insert(k.clone(), v.clone());
                }
            }
        };
        if kind == "rename" {
            merge(st.cfg.get("rename_landing"));
        }
        merge(st.cfg.get("by_name").and_then(|b| b.get(name)));
        spec
    }

    pub fn on_landed(&self, library: &str, name: &str, kind: &str) {
        let t = self.clock.elapsed();
        let mut st = self.state.lock().unwrap();
        let spec = Self::landing(&st, name, kind);
        let prefix = spec
            .get("prefix")
            .and_then(Value::as_str)
            .unwrap_or("N")
            .to_string();
        st.next_id += 1;
        let first = format!("{prefix}{:04}", st.next_id);
        let listed_after = num(&spec, "listed_after", 0.0);
        let mut ids = vec![(t + listed_after, first)];
        if let Some(list) = spec.get("reissue_at").and_then(Value::as_array) {
            for when in list {
                st.next_id += 1;
                ids.push((
                    t + when.as_f64().unwrap_or(0.0),
                    format!("{prefix}{:04}", st.next_id),
                ));
            }
        }
        let loaded = match spec.get("loaded_after") {
            Some(Value::Null) => None,
            Some(v) => Some(t + v.as_f64().unwrap_or(0.0)),
            None => Some(t),
        };
        let listed_from = t + listed_after;
        let entry = Entry {
            library: library.to_string(),
            name: name.to_string(),
            ids,
            listed_from,
            blank_until: listed_from + num(&spec, "blank_for", 0.0),
            loaded_from: loaded,
        };
        let key = (library.to_string(), name.to_string());
        st.entries.retain(|(k, _)| *k != key);
        st.entries.push((key, entry));
    }

    pub fn on_removed(&self, library: &str, name: &str) {
        let key = (library.to_string(), name.to_string());
        self.state
            .lock()
            .unwrap()
            .entries
            .retain(|(k, _)| *k != key);
    }

    fn slides(path: &Path) -> Vec<Value> {
        let bytes = std::fs::read(path).expect("read the presentation");
        let message = read_presentation(&bytes).expect("decode the presentation");
        let mut out = Vec::new();
        for cue in message.msgs("cues") {
            for action in cue.msgs("actions") {
                if !action.has("slide") {
                    continue;
                }
                let parts: Vec<String> = elements_of_action(action)
                    .into_iter()
                    .filter(|w| has_text(w))
                    .map(|w| {
                        plain_rtf(
                            element(w)
                                .unwrap()
                                .msg("text")
                                .map(|t| t.bytes("rtf_data"))
                                .unwrap_or(&[]),
                        )
                    })
                    .collect();
                out.push(json!({"enabled": true, "notes": "", "text": parts.join(" "), "label": cue.str("name")}));
            }
        }
        out
    }

    fn get(&self, st: &State, path: &str) -> (u16, Option<Value>) {
        let t = self.clock.elapsed();
        let listed = || {
            st.entries
                .iter()
                .map(|(_, e)| e)
                .filter(|e| e.listed_from <= t)
        };
        if path == "/version" {
            return (
                200,
                Some(
                    json!({"name": "ProPresenter", "platform": "macOS", "os_version": "15.7.9", "host_description": "fake",
                            "api_version": st.cfg.get("api_version").cloned().unwrap_or_else(|| json!("v1"))}),
                ),
            );
        }
        if path == "/v1/libraries" {
            return (200, Some(Value::Array(st.libraries.clone())));
        }
        if path.starts_with("/v1/library/") {
            let uuid = last_segment(path);
            let Some(lib) = st.libraries.iter().find(|l| l["uuid"] == uuid.as_str()) else {
                return (404, Some(json!({"error": "no such library"})));
            };
            let mut items: Vec<&Entry> = listed()
                .filter(|e| lib["name"] == e.library.as_str())
                .collect();
            items.sort_by(|a, b| a.name.cmp(&b.name));
            let rows: Vec<Value> = items
                .iter()
                .enumerate()
                .map(|(i, e)| {
                    json!({"uuid": if t < e.blank_until { String::new() } else { e.current(t) }, "name": e.name, "index": i})
                })
                .collect();
            return (200, Some(json!({"update_type": "", "items": rows})));
        }
        if path == "/v1/presentation/active" {
            return (404, Some(json!({"error": "no active presentation"})));
        }
        if path.starts_with("/v1/presentation/") {
            let ident = last_segment(path);
            let blocked: Vec<Value> = st
                .cfg
                .get("presentation_404")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for e in listed() {
                if e.current(t) == ident
                    && e.loaded_from.is_some_and(|l| t >= l)
                    && t >= e.blank_until
                    && !blocked.iter().any(|b| b == ident.as_str())
                {
                    let file = self
                        .root
                        .join("Libraries")
                        .join(&e.library)
                        .join(format!("{}.pro", e.name));
                    if !file.is_file() {
                        break;
                    }
                    let mut slides = Self::slides(&file);
                    if let Some(limit) = st.cfg.get("show_slides").and_then(Value::as_u64) {
                        slides.truncate(limit as usize);
                    }
                    return (
                        200,
                        Some(
                            json!({"presentation": {"id": {"uuid": ident, "name": e.name, "index": 0},
                                                      "groups": [{"name": "", "slides": slides}],
                                                      "has_timeline": false, "presentation_path": file.to_string_lossy(),
                                                      "destination": "presentation", "has_timeline_items": false}}),
                        ),
                    );
                }
            }
            return (404, Some(json!({"error": "not found"})));
        }
        if path == "/v1/playlists" {
            return match &st.playlist {
                None => (200, Some(json!([]))),
                Some(p) => (
                    200,
                    Some(
                        json!([{"id": {"uuid": p["id"], "name": p["name"], "index": 0}, "field_type": "playlist", "children": []}]),
                    ),
                ),
            };
        }
        if path.starts_with("/v1/playlist/") {
            if let Some(p) = &st.playlist {
                if p["id"] == last_segment(path).as_str() {
                    return (
                        200,
                        Some(
                            json!({"id": {"uuid": p["id"], "name": p["name"], "index": 0}, "items": p["items"]}),
                        ),
                    );
                }
            }
        }
        (404, Some(json!({"error": "unknown route"})))
    }

    fn put(&self, st: &mut State, path: &str, body: &mut Value) -> (u16, Option<Value>) {
        let t = self.clock.elapsed();
        let known = path.starts_with("/v1/playlist/")
            && st
                .playlist
                .as_ref()
                .is_some_and(|p| p["id"] == last_segment(path).as_str());
        if !known {
            return (404, None);
        }
        let status = st
            .cfg
            .get("put_status")
            .and_then(Value::as_u64)
            .unwrap_or(204) as u16;
        if status != 204 {
            return (status, Some(json!({"error": "refused"})));
        }
        if st
            .cfg
            .get("put_needs_loaded")
            .is_some_and(|v| v.as_bool().unwrap_or(false))
        {
            let listed: Vec<&Entry> = st
                .entries
                .iter()
                .map(|(_, e)| e)
                .filter(|e| e.listed_from <= t)
                .collect();
            if let Some(items) = body.as_array() {
                for item in items {
                    let ident = item["presentation_info"]["presentation_uuid"]
                        .as_str()
                        .unwrap_or("");
                    if !ident.is_empty()
                        && !listed
                            .iter()
                            .any(|e| e.current(t) == ident && e.loaded_from.is_some_and(|l| t >= l))
                    {
                        return (
                            400,
                            Some(json!({"error": "that presentation is not loaded"})),
                        );
                    }
                }
            }
        }
        if st
            .cfg
            .get("put_new_item_ids")
            .is_some_and(|v| v.as_bool().unwrap_or(false))
        {
            if let Some(items) = body.as_array_mut() {
                for item in items {
                    st.next_item += 1;
                    item["id"]["uuid"] = json!(format!("ITEM-{:04}", st.next_item));
                }
            }
        }
        if let Some(p) = st.playlist.as_mut() {
            p["items"] = body.clone();
        }
        (204, None)
    }

    fn handle(&self, stream: TcpStream) {
        let mut reader = BufReader::new(stream.try_clone().expect("clone the stream"));
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
            return;
        }
        let mut length = 0usize;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" || line == "\n" {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                if name.eq_ignore_ascii_case("content-length") {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
        }
        let mut raw = vec![0u8; length];
        let _ = reader.read_exact(&mut raw);
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let path = parts.next().unwrap_or("").to_string();
        let (status, answer, logged_body) = {
            let mut st = self.state.lock().unwrap();
            let (status, answer, logged) = if method == "PUT" {
                let mut parsed: Value = if raw.is_empty() {
                    Value::Null
                } else {
                    serde_json::from_slice(&raw).unwrap_or(Value::Null)
                };
                let (s, a) = self.put(&mut st, &path, &mut parsed);
                (s, a, parsed)
            } else {
                let (s, a) = self.get(&st, &path);
                (s, a, Value::Null)
            };
            st.log.push(json!({"t": round3(self.clock.elapsed()), "method": method, "path": path, "body": logged, "status": status}));
            (status, answer, logged)
        };
        let _ = logged_body;
        let data = answer
            .map(|v| serde_json::to_vec(&v).unwrap())
            .unwrap_or_default();
        let mut head = format!("HTTP/1.0 {status} X\r\n");
        if !data.is_empty() {
            head.push_str("Content-Type: application/json\r\n");
        }
        head.push_str(&format!("Content-Length: {}\r\n\r\n", data.len()));
        let mut stream = stream;
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.write_all(&data);
        let _ = stream.flush();
    }
}
