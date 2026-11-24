//! `workflow/pipeline.py`: one import pipeline used by the command line and the web page.
//!
//! * plan  -> read notes, classify, fetch Scripture, report successes and problems (writes nothing)
//! * apply -> write the result into ProPresenter's library and record it so it can be replaced or undone.
//!
//! The way in is [`publish_new`] (a new file), [`publish_swap`] (Replace / Append without restarting ProPresenter: the new file takes the
//! original's place under its name and the playlists follow it) and [`apply_import`] (`--in-place`, which closes and reopens the app).
//! [`undo_import`] puts it back. Everything waits and stamps through the [`Ctx`]'s clock, so the whole run can be replayed in virtual time.
//!
//! This is a straight port: the order of the questions asked of ProPresenter, the pauses between them and the words of every message are
//! those of the Python file (the oracle `oracle/pipeline/` compares them request by request).

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use regex::Regex;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use versevision_core::bibles::BiblePassages;
use versevision_core::config::AppConfig;
use versevision_core::notes::title::{numbered_name, safe_presentation_name, unique_name};
use versevision_core::notes::trace::{emit, NoTrace};
use versevision_core::plan::{NotesImportResult, PlanOptions, SlidePlan};
use versevision_core::references::pytext::casefold;
use versevision_core::workflow::plan_notes;

use crate::clock::{pydantic_datetime, Clock};
use crate::dirs::Dirs;
use crate::errors::{Kind, VvError, VvResult};
use crate::files::{append_plan_in_place, create_presentation_file, derive_presentation_file};
use crate::history::{HistoryStore, ImportRecord};
use crate::model::{elements_of_action, read_presentation, uuid_of};
use crate::pb::Message;
use crate::pyjson::dumps;
use crate::pyrepr::py_repr;
use crate::runlog::RunLog;
use crate::template::validate_template;
use crate::themes::{self, LoadedLooks};
use crate::transport::Transport;
use crate::writer::{generated_cue_uuids, PresentationWriter, UuidSource};

/// ProPresenter needs about 2 s after a rename before a new file may appear at the old name; writing sooner leaves it with a dead entry
/// for that name (measured on the iMac, 0.5 s fails every time).
pub const MIN_GAP_SECONDS: f64 = 4.0;
/// No questions to ProPresenter for this long after a new file is written.
pub const QUIET_SECONDS: f64 = 6.0;
/// How long to keep pointing the playlist at the id ProPresenter currently lists for the new file.
pub const PLAYLIST_CONVERGE_SECONDS: f64 = 90.0;
/// A listed file with a blank id (not loaded yet) is checked on disk after this long.
pub const LOAD_GRACE_SECONDS: f64 = 8.0;
/// The new file's id must not change for this long before it is trusted.
pub const STABLE_SECONDS: f64 = 4.0;
/// How long each swap step waits for ProPresenter to notice a file change.
pub const SETTLE_SECONDS: f64 = 15.0;
/// How long a swap waits for ProPresenter to list and show the new file.
pub const SWAP_WAIT_SECONDS: f64 = 30.0;

/// The module constants of the Python file as a value, so a test can shorten them (Python tests patch the module).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timing {
    pub min_gap: f64,
    pub quiet: f64,
    pub playlist_converge: f64,
    pub load_grace: f64,
    pub stable: f64,
    pub settle: f64,
    pub swap_wait: f64,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            min_gap: MIN_GAP_SECONDS,
            quiet: QUIET_SECONDS,
            playlist_converge: PLAYLIST_CONVERGE_SECONDS,
            load_grace: LOAD_GRACE_SECONDS,
            stable: STABLE_SECONDS,
            settle: SETTLE_SECONDS,
            swap_wait: SWAP_WAIT_SECONDS,
        }
    }
}

/// The text ProPresenterNotFound starts with after the route: ProPresenter answered 404 for the presentation.
const NOT_LOADED: &str = "has nothing at";

/// New ids, shared by the pipeline and the writer (one numbered sequence, so a seeded run repeats).
#[derive(Clone)]
pub struct SharedUuids(Arc<Mutex<Box<dyn UuidSource + Send>>>);

impl SharedUuids {
    pub fn new(source: impl UuidSource + Send + 'static) -> SharedUuids {
        SharedUuids(Arc::new(Mutex::new(Box::new(source))))
    }
    pub fn random() -> SharedUuids {
        SharedUuids::new(crate::writer::RandomUuids)
    }
}

impl UuidSource for SharedUuids {
    fn next_uuid(&mut self) -> String {
        self.0.lock().expect("uuid source").next_uuid()
    }
}

/// Where the progress messages of a run are shown.
pub type ProgressSink = Box<dyn FnMut(&str) + Send>;

/// Everything a run needs besides the config and the transport: time, ids, folders, the history, the run log in progress.
pub struct Ctx {
    pub clock: Arc<dyn Clock>,
    pub uuids: SharedUuids,
    pub dirs: Dirs,
    pub history: HistoryStore,
    /// `runlog.current()`.
    pub run: Option<RunLog>,
    /// `versevision.__version__` (the host says which build this is).
    pub app_version: String,
    pub timing: Timing,
    /// `runlog.wrap_progress`: every progress message is also an `progress` event of the run.
    pub log_progress: bool,
    pub progress_sink: Option<ProgressSink>,
}

impl Ctx {
    pub fn new(clock: Arc<dyn Clock>, uuids: SharedUuids, dirs: Dirs, app_version: &str) -> Ctx {
        let history = HistoryStore::new(dirs.history_path(), clock.clone());
        Ctx {
            clock,
            uuids,
            dirs,
            history,
            run: None,
            app_version: app_version.to_string(),
            timing: Timing::default(),
            log_progress: false,
            progress_sink: None,
        }
    }

    /// `runlog.event(step, level, **data)`: nothing happens when no run is active.
    pub fn event(&mut self, step: &str, level: &str, data: Value) {
        if let (Some(run), Value::Object(map)) = (self.run.as_mut(), data) {
            run.event(step, level, map);
        }
    }

    /// `runlog.trace(stage, **data)`.
    pub fn trace(&mut self, stage: &str, data: Value) {
        if let (Some(run), Value::Object(map)) = (self.run.as_mut(), data) {
            emit(run, stage, map);
        }
    }

    /// `progress(message)` of a run (`runlog.wrap_progress`): logged, then shown.
    pub fn progress(&mut self, message: &str) {
        if self.log_progress {
            self.event("progress", "info", json!({"message": message}));
        }
        if let Some(sink) = self.progress_sink.as_mut() {
            sink(message);
        }
    }

    pub fn mono(&self) -> f64 {
        self.clock.monotonic()
    }

    pub fn sleep(&self, seconds: f64) {
        self.clock.sleep(seconds);
    }

    /// `datetime.now(timezone.utc)` as the time stamped on records.
    pub fn now_micros(&self) -> i64 {
        self.clock.now_micros()
    }

    /// `str(uuid.uuid4())`.
    pub fn uuid(&mut self) -> String {
        self.uuids.next_uuid()
    }

    /// `uuid.uuid4().hex[:10]`.
    pub fn short_id(&mut self) -> String {
        self.uuid().replace('-', "")[..10].to_string()
    }

    fn writer(&self) -> PresentationWriter<SharedUuids> {
        PresentationWriter::new(self.uuids.clone())
    }
}

/// `ApplyResult` (field order is the model's).
#[derive(Debug, Clone, PartialEq)]
pub struct ApplyResult {
    pub mode: String,
    pub presentation_name: String,
    pub restart_needed: bool,
    pub import_id: String,
    pub target_name: String,
    pub node: String,
    pub slide_count: i64,
    pub cue_count_before: i64,
    pub cue_count_after: i64,
    pub removed_slides: i64,
    pub backup_path: Option<String>,
    pub verification: String,
    pub verified: bool,
    pub reopened: bool,
    pub steps: Vec<String>,
}

impl ApplyResult {
    fn blank(import_id: &str, target_name: &str, node: &str) -> ApplyResult {
        ApplyResult {
            mode: "new".into(),
            presentation_name: String::new(),
            restart_needed: false,
            import_id: import_id.to_string(),
            target_name: target_name.to_string(),
            node: node.to_string(),
            slide_count: 0,
            cue_count_before: 0,
            cue_count_after: 0,
            removed_slides: 0,
            backup_path: None,
            verification: String::new(),
            verified: false,
            reopened: false,
            steps: Vec::new(),
        }
    }

    /// `model_dump()`.
    pub fn to_value(&self) -> Value {
        json!({
            "mode": self.mode,
            "presentation_name": self.presentation_name,
            "restart_needed": self.restart_needed,
            "import_id": self.import_id,
            "target_name": self.target_name,
            "node": self.node,
            "slide_count": self.slide_count,
            "cue_count_before": self.cue_count_before,
            "cue_count_after": self.cue_count_after,
            "removed_slides": self.removed_slides,
            "backup_path": self.backup_path,
            "verification": self.verification,
            "verified": self.verified,
            "reopened": self.reopened,
            "steps": self.steps,
        })
    }
}

/// A presentation ProPresenter lists (`list_presentations`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Presentation {
    pub uuid: String,
    pub name: String,
    pub library: String,
    pub library_uuid: String,
}

impl Presentation {
    pub fn to_value(&self) -> Value {
        json!({"uuid": self.uuid, "name": self.name, "library": self.library, "library_uuid": self.library_uuid})
    }
}

/// `(library_name, presentation_uuid, presentation_name, path, detail)`.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub library: String,
    pub uuid: String,
    pub name: String,
    pub path: String,
    pub detail: Value,
}

/// A plan as the pipeline writes it: the planner's result and the theme slides its look is made from.
#[derive(Debug, Clone)]
pub struct Planned {
    pub result: NotesImportResult,
    pub looks: Option<LoadedLooks>,
}

// ------------------------------------------------------------------------------------------------------------ small JSON helpers

fn key_error(key: &str) -> VvError {
    VvError::new(Kind::Other("KeyError"), format!("'{key}'"))
}

fn type_error(what: &str) -> VvError {
    VvError::new(Kind::Other("TypeError"), what.to_string())
}

/// `value[key]`.
fn req<'a>(value: &'a Value, key: &str) -> VvResult<&'a Value> {
    match value {
        Value::Object(map) => map.get(key).ok_or_else(|| key_error(key)),
        _ => Err(type_error(
            "list indices must be integers or slices, not str",
        )),
    }
}

/// `value[key]` that must be text.
fn req_str(value: &Value, key: &str) -> VvResult<String> {
    match req(value, key)? {
        Value::String(s) => Ok(s.clone()),
        _ => Err(type_error("expected a string")),
    }
}

/// `value.get(key, "")` as text (`None` or a missing key is `""`).
fn get_str(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// `value.get("items", [])`.
fn items_of(value: &Value) -> VvResult<Vec<Value>> {
    match value {
        Value::Object(map) => match map.get("items") {
            None => Ok(Vec::new()),
            Some(Value::Array(a)) => Ok(a.clone()),
            Some(_) => Err(type_error("'NoneType' object is not iterable")),
        },
        _ => Err(VvError::new(
            Kind::Other("AttributeError"),
            "object has no attribute 'get'",
        )),
    }
}

fn as_list(value: &Value) -> VvResult<Vec<Value>> {
    match value {
        Value::Array(a) => Ok(a.clone()),
        _ => Err(type_error("object is not iterable")),
    }
}

fn eq_fold(a: &str, b: &str) -> bool {
    casefold(a) == casefold(b)
}

fn file_name_of(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn workdir() -> VvResult<tempfile::TempDir> {
    tempfile::Builder::new()
        .prefix("versevision-")
        .tempdir()
        .map_err(VvError::from)
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

// ------------------------------------------------------------------------------------------------------------ listing

/// `list_presentations`: every presentation ProPresenter knows about, grouped by library, for pickers.
pub fn list_presentations(transport: &dyn Transport) -> VvResult<Vec<Presentation>> {
    let mut out = Vec::new();
    for lib in as_list(&transport.api_get("/v1/libraries")?)? {
        let lib_uuid = req_str(&lib, "uuid")?;
        let contents = transport.api_get(&format!("/v1/library/{lib_uuid}"))?;
        for item in items_of(&contents)? {
            out.push(Presentation {
                uuid: req_str(&item, "uuid")?,
                name: req_str(&item, "name")?,
                library: req_str(&lib, "name")?,
                library_uuid: lib_uuid.clone(),
            });
        }
    }
    Ok(out)
}

/// One playlist with the presentations in it (`list_playlist_presentations`).
#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistListing {
    pub uuid: String,
    pub name: String,
    pub entries: Vec<PlaylistEntry>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistEntry {
    pub uuid: String,
    pub name: String,
    pub section: String,
    pub library: String,
}

impl PlaylistListing {
    pub fn to_value(&self) -> Value {
        json!({
            "uuid": self.uuid, "name": self.name,
            "entries": self.entries.iter().map(|e| json!({"uuid": e.uuid, "name": e.name, "section": e.section, "library": e.library})).collect::<Vec<_>>(),
        })
    }
}

/// `list_playlist_presentations`: every ProPresenter playlist with the presentations in it, in order, for pickers. `section` is the
/// header the entry sits under (PreService, Worship, Sermon...).
pub fn list_playlist_presentations(transport: &dyn Transport) -> VvResult<Vec<PlaylistListing>> {
    let known: Vec<Presentation> = list_presentations(transport)?
        .into_iter()
        .filter(|i| !i.uuid.is_empty())
        .collect();
    let mut out = Vec::new();
    fn walk(
        transport: &dyn Transport,
        known: &[Presentation],
        nodes: &[Value],
        trail: &[String],
        out: &mut Vec<PlaylistListing>,
    ) -> VvResult<()> {
        for node in nodes {
            let children = node
                .get("children")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let node_name = req_str(req(node, "id")?, "name")?;
            if node.get("field_type").and_then(Value::as_str) == Some("playlist")
                || children.is_empty()
            {
                let pid = req_str(req(node, "id")?, "uuid")?;
                let items = transport
                    .api_get(&format!("/v1/playlist/{pid}"))
                    .and_then(|v| items_of(&v))
                    .unwrap_or_default();
                let (mut section, mut entries) = (String::new(), Vec::new());
                for item in items {
                    match item.get("type").and_then(Value::as_str) {
                        Some("header") => {
                            section = item
                                .get("id")
                                .map(|i| get_str(i, "name"))
                                .unwrap_or_default()
                        }
                        Some("presentation") => {
                            let uid = item
                                .get("presentation_info")
                                .filter(|v| !v.is_null())
                                .map(|i| get_str(i, "presentation_uuid"))
                                .unwrap_or_default();
                            let found = known.iter().find(|k| eq_fold(&k.uuid, &uid));
                            entries.push(PlaylistEntry {
                                uuid: uid.clone(),
                                name: match found {
                                    Some(f) => f.name.clone(),
                                    None => item
                                        .get("id")
                                        .map(|i| get_str(i, "name"))
                                        .unwrap_or_default(),
                                },
                                section: section.clone(),
                                library: found.map(|f| f.library.clone()).unwrap_or_default(),
                            });
                        }
                        _ => {}
                    }
                }
                let mut full = trail.to_vec();
                full.push(node_name);
                out.push(PlaylistListing {
                    uuid: pid,
                    name: full.join(" / "),
                    entries,
                });
            } else {
                let mut next = trail.to_vec();
                next.push(node_name);
                walk(transport, known, &children, &next, out)?;
            }
        }
        Ok(())
    }
    let nodes = as_list(&transport.api_get("/v1/playlists")?)?;
    walk(transport, &known, &nodes, &[], &mut out)?;
    Ok(out)
}

/// `source_fingerprint`.
pub fn source_fingerprint(text: &str) -> String {
    sha256_hex(text.as_bytes())
}

// ------------------------------------------------------------------------------------------------------------ finding the target

/// Find the target by its file when ProPresenter is closed (the API is unavailable).
fn target_from_disk(
    ctx: &mut Ctx,
    transport: &dyn Transport,
    uuid: Option<&str>,
    name: Option<&str>,
    library: Option<&str>,
) -> VvResult<Target> {
    let mut wanted = name.filter(|n| !n.is_empty()).map(str::to_string);
    if wanted.is_none() {
        let record = ctx
            .history
            .load()
            .into_iter()
            .find(|r| uuid.is_some_and(|u| !u.is_empty() && eq_fold(&r.target_uuid, u)));
        wanted = record.map(|r| r.target_name);
    }
    let Some(wanted) = wanted.filter(|w| !w.is_empty()) else {
        return Err(VvError::configuration(
            "ProPresenter is not running, so the presentation cannot be looked up by id; give its name",
        ));
    };
    let paths = transport.find_presentation(&wanted, library);
    if paths.is_empty() {
        return Err(VvError::configuration(format!(
            "ProPresenter is not running and no file named {wanted}.pro was found in its library folder"
        )));
    }
    if paths.len() > 1 {
        return Err(VvError::configuration(format!(
            "Presentation target is ambiguous: {}",
            paths.join(", ")
        )));
    }
    let dir = workdir()?;
    let local = dir.path().join(file_name_of(&paths[0]));
    transport.fetch_file(&paths[0], &local)?;
    let message = read_presentation(&fs::read(&local)?)?;
    let parent = Path::new(&paths[0])
        .parent()
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let shown = message.str("name");
    Ok(Target {
        library: parent,
        uuid: uuid_of(&message).to_string(),
        name: if shown.is_empty() {
            wanted
        } else {
            shown.to_string()
        },
        path: paths[0].clone(),
        detail: json!({"from_disk": true}),
    })
}

/// Find the target presentation and its file path: through the ProPresenter API, or from disk when it is closed.
pub fn target_from_api(
    ctx: &mut Ctx,
    transport: &dyn Transport,
    uuid: Option<&str>,
    name: Option<&str>,
    library: Option<&str>,
) -> VvResult<Target> {
    match target_from_api_only(ctx, transport, uuid, name, library) {
        Ok(target) => Ok(target),
        Err(error) if error.is_configuration() => Err(error),
        Err(api_error) => {
            if transport.is_running() && !api_error.is_not_found() {
                return Err(api_error);
            }
            target_from_disk(ctx, transport, uuid, name, library).map_err(|disk_error| {
                if disk_error.is_configuration() {
                    VvError::configuration(format!(
                        "{} (the ProPresenter API was also unavailable: {})",
                        disk_error.message, api_error.message
                    ))
                } else {
                    disk_error
                }
            })
        }
    }
}

fn target_from_api_only(
    ctx: &mut Ctx,
    transport: &dyn Transport,
    uuid: Option<&str>,
    name: Option<&str>,
    library: Option<&str>,
) -> VvResult<Target> {
    let libraries = as_list(&transport.api_get("/v1/libraries")?)?;
    let mut found: Vec<(String, Value)> = Vec::new();
    for lib in &libraries {
        let lib_name = req_str(lib, "name")?;
        if library.is_some_and(|l| !l.is_empty() && !eq_fold(&lib_name, l)) {
            continue;
        }
        let contents = transport.api_get(&format!("/v1/library/{}", req_str(lib, "uuid")?))?;
        for item in items_of(&contents)? {
            let item_uuid = req_str(&item, "uuid")?;
            let item_name = req_str(&item, "name")?;
            let by_uuid = uuid.is_some_and(|u| !u.is_empty() && eq_fold(&item_uuid, u));
            let by_name = name.is_some_and(|n| !n.is_empty() && eq_fold(&item_name, n));
            if by_uuid || by_name {
                found.push((lib_name.clone(), item));
            }
        }
    }
    if found.is_empty() {
        let shown = match name.filter(|n| !n.is_empty()).or(uuid) {
            Some(n) => py_repr(n),
            None => "None".to_string(),
        };
        return Err(VvError::configuration(format!(
            "Presentation {shown} was not found in ProPresenter"
        )));
    }
    if found.len() > 1 {
        let list: Vec<String> = found
            .iter()
            .map(|(lib, it)| format!("{lib}/{}", get_str(it, "name")))
            .collect();
        return Err(VvError::configuration(format!(
            "Presentation target is ambiguous: {}",
            list.join(", ")
        )));
    }
    let (library_name, item) = found.remove(0);
    let item_uuid = req_str(&item, "uuid")?;
    let item_name = req_str(&item, "name")?;
    let detail: Option<Value>;
    match transport.api_get(&format!("/v1/presentation/{item_uuid}")) {
        Ok(v) => detail = Some(req(&v, "presentation")?.clone()),
        Err(first) if first.is_connection() => {
            // ProPresenter lists the presentation but will not hand it over by id (it has not loaded that file). Try the open one, then the file.
            ctx.event(
                "target_lookup",
                "warn",
                json!({"message": format!("ProPresenter would not return {item_name} by id ({first}); trying the open presentation, then the file on disk")}),
            );
            let active = (|| -> VvResult<Option<Value>> {
                let reply = transport.api_get("/v1/presentation/active")?;
                let active = req(&reply, "presentation")?.clone();
                let active_uuid = req_str(req(&active, "id")?, "uuid")?;
                let has_path = active
                    .get("presentation_path")
                    .is_some_and(|p| !p.is_null() && p != &json!(""));
                Ok((eq_fold(&active_uuid, &item_uuid) && has_path).then_some(active))
            })()
            .unwrap_or(None);
            match active {
                Some(active) => detail = Some(active),
                None => {
                    let from_disk = target_from_disk(
                        ctx,
                        transport,
                        Some(&item_uuid),
                        Some(&item_name),
                        Some(&library_name),
                    )?;
                    ctx.event(
                        "target_lookup",
                        "info",
                        json!({"message": format!("read {item_name} from its file instead: {}", from_disk.path)}),
                    );
                    ctx.trace(
                        "target",
                        json!({"how": "from the file on disk: ProPresenter would not return the presentation by its id", "name": item_name, "library": library_name, "path": from_disk.path}),
                    );
                    return Ok(Target {
                        library: library_name,
                        uuid: item_uuid,
                        name: item_name,
                        path: from_disk.path,
                        detail: from_disk.detail,
                    });
                }
            }
        }
        Err(other) => return Err(other),
    }
    let detail = detail.unwrap_or(Value::Null);
    let path = crate::pyrepr::py_str(req(&detail, "presentation_path")?);
    ctx.trace(
        "target",
        json!({"how": "from the ProPresenter API", "name": item_name, "library": library_name, "path": path}),
    );
    Ok(Target {
        library: library_name,
        uuid: item_uuid,
        name: item_name,
        path,
        detail,
    })
}

/// The presentation that holds the template slides: by its saved id, else by name (ProPresenter can re-issue ids).
fn template_target(
    ctx: &mut Ctx,
    transport: &dyn Transport,
    definition: &versevision_core::config::PresentationTemplateConfig,
) -> VvResult<Target> {
    match target_from_api(
        ctx,
        transport,
        Some(&definition.presentation_uuid),
        None,
        None,
    ) {
        Ok(target) => Ok(target),
        Err(error) if error.is_configuration() => {
            let found = target_from_api(
                ctx,
                transport,
                None,
                Some(&definition.presentation_name),
                definition.library_name.as_deref(),
            )?;
            ctx.event(
                "template_id_refreshed",
                "warn",
                json!({"message": format!("saved id was stale; found {} by name", py_repr(&found.name)), "saved": definition.presentation_uuid, "now": found.uuid}),
            );
            Ok(found)
        }
        Err(other) => Err(other),
    }
}

/// `_template_target` for callers outside this module (`template check`).
pub fn find_template_target(
    ctx: &mut Ctx,
    transport: &dyn Transport,
    definition: &versevision_core::config::PresentationTemplateConfig,
) -> VvResult<Target> {
    template_target(ctx, transport, definition)
}

// ------------------------------------------------------------------------------------------------------------ planning

/// `plan_for_transport`: plan the notes, work out the look from ProPresenter's themes, find the target presentation. Writes nothing.
///
/// `bible` is where verse text comes from (`None` plans without it, as `no_fetch` does). The transport's calls block: run this on a thread
/// that may block (`spawn_blocking`).
#[allow(clippy::too_many_arguments)]
pub async fn plan_for_transport<B: BiblePassages + ?Sized>(
    ctx: &mut Ctx,
    text: &str,
    config: &AppConfig,
    transport: &dyn Transport,
    default_version: Option<&str>,
    target_uuid: Option<&str>,
    target_name: Option<&str>,
    library: Option<&str>,
    no_fetch: bool,
    bible: Option<&B>,
) -> VvResult<(Planned, Option<Target>)> {
    let default_version = default_version.filter(|d| !d.is_empty());
    let used = default_version.map(str::to_string).or_else(|| {
        config
            .bible
            .default_version
            .clone()
            .filter(|d| !d.is_empty())
    });
    ctx.event(
        "plan_start",
        "info",
        json!({"default_version": used, "no_fetch": no_fetch, "characters": text.chars().count()}),
    );
    // `plan_notes` of Python saves the options of the run first
    if let Some(run) = ctx.run.as_ref() {
        let cfg = config.to_value();
        run.save_options(json!({
            "notes": cfg["notes"], "points": cfg["points"], "slides": cfg["slides"], "verses": cfg["verses"],
            "bible": {"default_version": cfg["bible"]["default_version"], "version_aliases": cfg["bible"]["version_aliases"], "version_routes": cfg["bible"]["version_routes"]},
            "default_version_used": used,
            "no_fetch": no_fetch,
        }));
    }
    let options = PlanOptions {
        default_version: default_version.map(str::to_string),
        no_fetch,
    };
    let result = match ctx.run.as_mut() {
        Some(run) => plan_notes(text, config, &options, bible, run).await?,
        None => plan_notes(text, config, &options, bible, &mut NoTrace).await?,
    };
    let mut planned = Planned {
        result,
        looks: None,
    };
    if !no_fetch {
        planned.looks = resolve_looks(ctx, config, transport)?;
        let describe = planned.looks.as_ref().map(|l| l.describe());
        ctx.trace(
            "look",
            json!({
                "verse": describe.as_ref().map(|d| d.verse.clone()),
                "point": describe.as_ref().map(|d| d.point.clone()),
                "note": "the ProPresenter theme slides used for the look (empty = the VerseVision template)",
            }),
        );
        if let Some(d) = &describe {
            let parts: Vec<String> = [("verse", &d.verse), ("point", &d.point)]
                .iter()
                .filter(|(_, v)| !v.is_empty())
                .map(|(k, v)| format!("{k}: {v}"))
                .collect();
            ctx.event(
                "looks",
                "info",
                json!({"message": format!("look from ProPresenter themes: {}", parts.join("; "))}),
            );
        }
        planned.result.slide_plan.looks = planned.looks.as_ref().map(loaded_to_looks);
    }
    let r = &planned.result;
    ctx.event(
        "plan_ready",
        "info",
        json!({"message": format!("{} slides, {} not added, {} to check, {} left out, {} adjustments",
            r.slide_plan.slides.len(), r.failed.len(), r.review.len(), r.skipped.len(), r.notices.len())}),
    );
    let mut target = None;
    if target_uuid.is_some_and(|u| !u.is_empty()) || target_name.is_some_and(|n| !n.is_empty()) {
        let found = target_from_api(ctx, transport, target_uuid, target_name, library)?;
        ctx.event(
            "target",
            "info",
            json!({"name": found.name, "library": found.library, "uuid": found.uuid, "path": found.path}),
        );
        target = Some(found);
    }
    Ok((planned, target))
}

fn loaded_to_looks(l: &LoadedLooks) -> versevision_core::looks::Looks {
    versevision_core::looks::Looks {
        verse: l.verse.as_ref().map(|s| s.slide.clone()),
        point: l.point.as_ref().map(|s| s.slide.clone()),
    }
}

/// `themes.resolve_looks(config, transport)`: the theme slides chosen in the config, or `None` when neither is set.
pub fn resolve_looks(
    _ctx: &mut Ctx,
    config: &AppConfig,
    transport: &dyn Transport,
) -> VvResult<Option<LoadedLooks>> {
    let (verse_theme, point_theme) = (
        versevision_core::pystr::strip(&config.verses.theme),
        versevision_core::pystr::strip(&config.points.theme),
    );
    if verse_theme.is_empty() && point_theme.is_empty() {
        return Ok(None);
    }
    let folder = tempfile::Builder::new()
        .prefix("versevision-themes-")
        .tempdir()?;
    transport.fetch_themes(folder.path())?;
    let slides = themes::read_theme_tree(folder.path());
    Ok(themes::resolve_looks(
        &slides,
        &config.verses.theme,
        &config.verses.theme_slide,
        &config.points.theme,
        &config.points.theme_slide,
    )?)
}

// ------------------------------------------------------------------------------------------------------------ verifying

fn norm(value: &str) -> String {
    casefold(&versevision_core::pystr::split_ws(value).join(" "))
}

/// What a swap checks the new file against.
pub enum Verifier<'a> {
    /// The slides of the plan are all shown (`verify_via_api(..., timeout=4.0)`).
    Plan {
        plan: &'a SlidePlan,
        expected: usize,
    },
    /// The presentation shows this many slides (the check of an undo).
    SlideCount { wanted: usize },
}

impl Verifier<'_> {
    fn check(&self, ctx: &mut Ctx, transport: &dyn Transport, uuid: &str) -> (bool, String) {
        match self {
            Verifier::Plan { plan, expected } => {
                verify_via_api(ctx, transport, uuid, plan, *expected, 4.0)
            }
            Verifier::SlideCount { wanted } => {
                let deadline = ctx.mono() + 8.0;
                let mut problem = "ProPresenter did not report the presentation".to_string();
                while ctx.mono() < deadline {
                    let attempt = (|| -> VvResult<Option<String>> {
                        let reply = transport.api_get(&format!("/v1/presentation/{uuid}"))?;
                        let detail = req(&reply, "presentation")?;
                        let mut count = 0usize;
                        for g in as_list(req(detail, "groups")?)? {
                            count += as_list(req(&g, "slides")?)?.len();
                        }
                        if count == *wanted {
                            return Ok(Some(format!("{count} slides")));
                        }
                        Err(VvError::general(format!(
                            "ProPresenter shows {count} slides; expected {wanted}"
                        )))
                    })();
                    match attempt {
                        Ok(Some(done)) => return (true, done),
                        Ok(None) => {}
                        Err(e) => problem = e.message,
                    }
                    ctx.sleep(1.0);
                }
                (false, problem)
            }
        }
    }
}

/// `verify_via_api`: the presentation shows every slide of the plan (waits up to `timeout` seconds).
pub fn verify_via_api(
    ctx: &mut Ctx,
    transport: &dyn Transport,
    presentation_uuid: &str,
    plan: &SlidePlan,
    expected_slides: usize,
    timeout: f64,
) -> (bool, String) {
    let deadline = ctx.mono() + timeout;
    let mut problem = "ProPresenter did not report the presentation".to_string();
    while ctx.mono() < deadline {
        let attempt = (|| -> VvResult<Result<String, String>> {
            let reply = transport.api_get(&format!("/v1/presentation/{presentation_uuid}"))?;
            let detail = req(&reply, "presentation")?;
            let mut slides: Vec<String> = Vec::new();
            for group in as_list(req(detail, "groups")?)? {
                for slide in as_list(req(&group, "slides")?)? {
                    slides.push(get_str(&slide, "text"));
                }
            }
            if slides.len() < expected_slides {
                return Ok(Err(format!(
                    "API reports {} slides; expected at least {expected_slides}",
                    slides.len()
                )));
            }
            let visible: Vec<String> = slides.iter().map(|s| norm(s)).collect();
            let missing = plan
                .slides
                .iter()
                .filter(|s| {
                    let first = norm(s.body_text.split('\n').next().unwrap_or(""));
                    !visible.iter().any(|v| v.contains(&first))
                })
                .count();
            if missing == 0 {
                return Ok(Ok(format!(
                    "ProPresenter reports all {} generated slides",
                    plan.slides.len()
                )));
            }
            Ok(Err(format!(
                "{missing} generated slide(s) not reported by ProPresenter"
            )))
        })();
        match attempt {
            Ok(Ok(done)) => return (true, done),
            Ok(Err(why)) => problem = why,
            Err(e) => problem = e.message, // the app may still be loading
        }
        ctx.sleep(1.5);
    }
    (false, problem)
}

fn norm_words(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        .collect()
}

/// `_visible_text`: (number of slides, all the words on them as lower-case letters and digits only). Punctuation, rtf codes, slide names,
/// ids and order are ignored: ProPresenter changes all of those when it re-saves a file, but not the words.
fn visible_text(path: &Path) -> VvResult<(usize, String)> {
    let message = read_presentation(&fs::read(path)?)?;
    let uni = Regex::new(r"\\u(-?\d+)\??").expect("pattern");
    let hex = Regex::new(r"\\'[0-9a-fA-F]{2}").expect("pattern");
    let word = Regex::new(r"\\[a-zA-Z]+-?\d* ?|[{}]").expect("pattern");
    let mut chunks: Vec<String> = Vec::new();
    for cue in message.msgs("cues") {
        for action in cue.msgs("actions") {
            if !action.has("slide") {
                continue;
            }
            for wrapper in elements_of_action(action) {
                let Some(text) = wrapper.msg("element").and_then(|e| e.msg("text")) else {
                    continue;
                };
                let raw: String = text.bytes("rtf_data").iter().map(|b| *b as char).collect();
                let raw = uni.replace_all(&raw, |m: &regex::Captures| {
                    let n: i64 = m[1].parse().unwrap_or(0);
                    char::from_u32(n.rem_euclid(65536) as u32)
                        .map(String::from)
                        .unwrap_or_default()
                });
                let raw = hex.replace_all(&raw, "");
                let raw = word.replace_all(&raw, " ");
                chunks.push(raw.into_owned());
            }
        }
    }
    Ok((message.len("cues"), norm_words(&chunks.join(" "))))
}

/// What a file on disk is checked against.
struct FileCheck<'a> {
    path: &'a str,
    sha: &'a str,
    local: Option<&'a Path>,
    texts: &'a [String],
}

/// `_verify_file_on_disk`: the file ProPresenter was given still holds what was written. Byte-for-byte equal is the strongest proof.
/// ProPresenter re-saves a file it has open (same words, different bytes, ids and names), so when the bytes differ the slides are
/// checked: the same number of slides, and the words of every new slide present.
fn verify_file_on_disk(
    ctx: &mut Ctx,
    transport: &dyn Transport,
    check: &FileCheck,
) -> (bool, String) {
    let tmp = match tempfile::Builder::new()
        .prefix("versevision-chk-")
        .tempdir()
    {
        Ok(t) => t,
        Err(e) => return (false, format!("could not read the file back: {e}")),
    };
    let local = tmp.path().join("check.pro");
    let sha = match transport.fetch_file(check.path, &local) {
        Ok(sha) => sha,
        Err(exc) => return (false, format!("could not read the file back: {exc}")),
    };
    if sha == check.sha {
        return (true, "file intact".to_string());
    }
    let read = (|| -> VvResult<(usize, String, Option<usize>)> {
        let (count, haystack) = visible_text(&local)?;
        let wanted = match check.local {
            Some(w) => Some(visible_text(w)?.0),
            None => None,
        };
        Ok((count, haystack, wanted))
    })();
    let (count, haystack, wanted_count) = match read {
        Ok(v) => v,
        Err(exc) => {
            return (
                false,
                format!(
                "the file on disk is not the one that was written and could not be read ({exc})"
            ),
            )
        }
    };
    drop(tmp);
    let missing = check
        .texts
        .iter()
        .filter(|t| {
            let w = norm_words(t);
            !w.is_empty() && !haystack.contains(&w)
        })
        .count();
    let sha12: String = sha.chars().take(12).collect();
    let written12: String = check.sha.chars().take(12).collect();
    ctx.event(
        "verify",
        "warn",
        json!({
            "message": format!("the file on disk differs from what was written (sha {sha12} vs {written12}): it holds {count} slides{}; {missing} of {} new slides not found in it",
                wanted_count.map(|w| format!(" ({w} written)")).unwrap_or_default(), check.texts.len()),
            "found_sha": sha.chars().take(16).collect::<String>(),
            "written_sha": check.sha.chars().take(16).collect::<String>(),
        }),
    );
    if !check.texts.is_empty() && wanted_count.is_none_or(|w| w == count) && missing == 0 {
        return (
            true,
            "ProPresenter re-saved the file; it holds the same number of slides and every new slide's words".to_string(),
        );
    }
    (
        false,
        format!(
            "the file on disk is not the one that was written (it holds {count} slides{}; {missing} new slides missing)",
            wanted_count.map(|w| format!(", {w} were written")).unwrap_or_default()
        ),
    )
}

/// `_file_uuid`: the id inside the file, upper case.
fn file_uuid(local: &Path) -> VvResult<String> {
    Ok(uuid_of(&read_presentation(&fs::read(local)?)?).to_uppercase())
}

fn cue_count_of(path: &Path) -> VvResult<usize> {
    Ok(read_presentation(&fs::read(path)?)?.len("cues"))
}

fn versions_of(plan: &SlidePlan) -> Vec<String> {
    let set: BTreeSet<String> = plan
        .slides
        .iter()
        .filter(|s| !s.version_text.is_empty())
        .map(|s| s.version_text.clone())
        .collect();
    set.into_iter().collect()
}

/// `append_audit_entry(user_log_path("versevision") / "audit.jsonl", AuditEntry(...))`.
#[allow(clippy::too_many_arguments)]
fn append_audit(
    ctx: &Ctx,
    target_uuid: Option<&str>,
    presentation_name: &str,
    reference_count: usize,
    slide_count: usize,
    versions: Vec<String>,
    backup_path: Option<&str>,
    verification_result: &str,
) -> VvResult<()> {
    let entry = json!({
        "timestamp": pydantic_datetime(ctx.now_micros()),
        "application_version": ctx.app_version,
        "propresenter_version": "19.0.1",
        "target_uuid": target_uuid,
        "presentation_name": presentation_name,
        "reference_count": reference_count,
        "slide_count": slide_count,
        "versions": versions,
        "backup_path": backup_path,
        "verification_result": verification_result,
    });
    let path = ctx.dirs.audit_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    use std::io::Write;
    let mut stream = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    stream.write_all(format!("{}\n", dumps(&entry, None, false)).as_bytes())?;
    Ok(())
}

// ------------------------------------------------------------------------------------------------------------ editing in place

fn new_record(
    ctx: &mut Ctx,
    import_id: &str,
    source_name: &str,
    text: &str,
    target_uuid: &str,
    target_name: &str,
    node: &str,
) -> ImportRecord {
    let mut record = ImportRecord::new(
        import_id,
        ctx.now_micros(),
        source_name,
        &source_fingerprint(text),
        target_uuid,
        target_name,
    );
    record.node = node.to_string();
    record
}

/// `apply_import`: change an EXISTING presentation in place. This needs ProPresenter closed (it ignores edits to an open file).
///
/// `replace`: remove the slides an earlier import of the same file added. `clear_generated`: remove every verse and sermon-point slide in
/// the presentation first (blank, background, title and media slides are kept).
#[allow(clippy::too_many_arguments)]
pub fn apply_import(
    ctx: &mut Ctx,
    text: &str,
    source_name: &str,
    plan: &Planned,
    target: &Target,
    config: &AppConfig,
    transport: &dyn Transport,
    replace: bool,
    clear_generated: bool,
    close_app: bool,
    reopen: bool,
) -> VvResult<ApplyResult> {
    let Target {
        library: library_name,
        uuid: presentation_uuid,
        name: presentation_name,
        path,
        ..
    } = target.clone();
    let Some(definition) = config.presentation_template.as_ref() else {
        return Err(VvError::configuration(
            "No template is configured; run versevision template select",
        ));
    };
    if plan.result.slide_plan.slides.is_empty() {
        return Err(VvError::configuration("There are no slides to import"));
    }
    let node_name = config
        .node
        .name
        .clone()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "local".into());
    let mut steps: Vec<String> = Vec::new();

    let previous = if replace && !clear_generated {
        ctx.history
            .latest_active(&presentation_uuid, Some(source_name), Some(&node_name))
    } else {
        None
    };
    let mut remove: Vec<String> = previous
        .as_ref()
        .map(|p| p.cue_uuids.clone())
        .unwrap_or_default();
    if let Some(p) = &previous {
        steps.push(format!(
            "replacing earlier import {} ({} slides)",
            p.import_id,
            p.cue_uuids.len()
        ));
    }

    // The template slides live in their own presentation. Fetch it now, while ProPresenter can still answer.
    let work = workdir()?;
    let mut template_local: Option<PathBuf> = None;
    match template_target(ctx, transport, definition) {
        Ok(where_) => {
            if where_.path != path {
                let local = work.path().join("template.pro");
                transport.fetch_file(&where_.path, &local)?;
                template_local = Some(local);
            }
        }
        Err(e) if e.is_configuration() => template_local = None, // older setups keep the templates inside the target itself
        Err(e) => return Err(e),
    }

    let was_running = transport.is_running();
    let reopen_after = reopen && was_running;
    if was_running {
        if !close_app {
            return Err(VvError::configuration(
                "ProPresenter is running; close it completely first (or allow VerseVision to close it)",
            ));
        }
        ctx.progress("Closing ProPresenter");
        transport.quit_app()?;
        let deadline = ctx.mono() + config.node.quit_wait_seconds;
        while ctx.mono() < deadline && transport.is_running() {
            ctx.sleep(1.0);
        }
        if transport.is_running() {
            return Err(VvError::write(
                "ProPresenter is waiting for someone at that computer to confirm quitting or saving \
                 (look for an 'Are you sure you want to quit?' or 'Save changes?' prompt). \
                 Answer it, then run the import again. No files changed",
            ));
        }
        steps.push("closed ProPresenter".into());
    }

    let local_copy = work.path().join(file_name_of(&path));
    let mut backup: Option<String> = None;
    let mut installed = false;
    let written = (|| -> VvResult<crate::writer::PlanWriteResult> {
        ctx.progress("Reading the presentation");
        let original_sha = transport.fetch_file(&path, &local_copy)?;
        let check_path = template_local.as_deref().unwrap_or(&local_copy);
        validate_template(&read_presentation(&fs::read(check_path)?)?, definition)?;
        ctx.progress("Backing up");
        let made = transport.backup_file(&path)?;
        steps.push(format!("backup: {made}"));
        backup = Some(made);
        ctx.progress("Writing slides");
        if clear_generated {
            let current = read_presentation(&fs::read(&local_copy)?)?;
            remove = generated_cue_uuids(&current, definition);
            steps.push(format!(
                "clearing {} verse and sermon-point slides (other slides are kept)",
                remove.len()
            ));
        }
        let mut writer = ctx.writer();
        let now = ctx.now_micros().div_euclid(1_000_000);
        let done = append_plan_in_place(
            &mut writer,
            &local_copy,
            &plan.result.slide_plan,
            definition,
            plan.looks.as_ref(),
            template_local.as_deref(),
            if remove.is_empty() {
                None
            } else {
                Some(&remove)
            },
            now,
        )?;
        transport.install_file(&local_copy, &path, &original_sha)?;
        installed = true;
        steps.push("installed updated presentation".into());
        Ok(done.result)
    })();
    let written = match written {
        Ok(w) => w,
        Err(error) => {
            if installed {
                if let Some(b) = &backup {
                    let _ = transport.restore_backup(b, &path);
                }
            }
            if was_running && reopen_after {
                let _ = transport.launch_app();
            }
            return Err(error);
        }
    };
    let backup = backup.unwrap_or_default();

    let (verified, message, reopened);
    if reopen_after {
        ctx.progress("Reopening ProPresenter");
        transport.launch_app()?;
        reopened = true;
        let deadline = ctx.mono() + config.node.launch_wait_seconds;
        while ctx.mono() < deadline {
            if transport.api_get("/version").is_ok() {
                break;
            }
            ctx.sleep(2.0);
        }
        ctx.progress("Verifying slides in ProPresenter");
        let (ok, msg) = verify_via_api(
            ctx,
            transport,
            &presentation_uuid,
            &plan.result.slide_plan,
            written.cue_count_after,
            25.0,
        );
        verified = ok;
        message = msg;
        if !ok {
            ctx.progress("Verification failed; rolling back");
            transport.quit_app()?;
            ctx.sleep(config.node.quit_wait_seconds / 2.0);
            if transport.is_running() {
                return Err(VvError::rollback(format!(
                    "Verification failed ({message}) and ProPresenter would not close. Close it and restore {backup} over {path}"
                )));
            }
            transport.restore_backup(&backup, &path)?;
            transport.launch_app()?;
            return Err(VvError::rollback(format!(
                "Verification failed ({message}). The original was restored from {backup}"
            )));
        }
    } else {
        verified = true;
        message = "Written and checked in the file; ProPresenter was closed, so the slides appear the next time you open it".to_string();
        reopened = false;
    }

    let import_id = ctx.short_id();
    if let Some(p) = &previous {
        ctx.history.set_status(&p.import_id, "replaced")?;
    }
    let mut record = new_record(
        ctx,
        &import_id,
        source_name,
        text,
        &presentation_uuid,
        &presentation_name,
        &node_name,
    );
    record.cue_uuids = written.cue_uuids.clone();
    record.slide_count = written.cue_uuids.len() as i64;
    record.verse_count = plan.result.verse_count as i64;
    record.point_count = plan.result.point_count as i64;
    record.backup_path = Some(backup.clone());
    record.replaces = previous.as_ref().map(|p| p.import_id.clone());
    record.mode = if clear_generated { "replace" } else { "append" }.into();
    record.library = Some(library_name);
    ctx.history.add(record)?;
    append_audit(
        ctx,
        Some(&presentation_uuid),
        &presentation_name,
        plan.result.verse_count,
        written.cue_uuids.len(),
        versions_of(&plan.result.slide_plan),
        if backup.is_empty() {
            None
        } else {
            Some(&backup)
        },
        &message,
    )?;
    let mut result = ApplyResult::blank(&import_id, &presentation_name, &node_name);
    result.mode = if clear_generated { "replace" } else { "append" }.into();
    result.presentation_name = presentation_name;
    result.restart_needed = !reopen_after && !was_running;
    result.slide_count = written.cue_uuids.len() as i64;
    result.cue_count_before = written.cue_count_before as i64;
    result.cue_count_after = written.cue_count_after as i64;
    result.removed_slides = written.removed_cue_count as i64;
    result.backup_path = Some(backup);
    result.verification = message;
    result.verified = verified;
    result.reopened = reopened;
    result.steps = steps;
    Ok(result)
}

/// `undo_import`: remove exactly the slides an earlier import added. Other edits to the presentation are kept.
pub fn undo_import(
    ctx: &mut Ctx,
    import_id: &str,
    config: &AppConfig,
    transport: &dyn Transport,
    close_app: bool,
    reopen: bool,
) -> VvResult<ApplyResult> {
    let record = ctx.history.get(import_id);
    let Some(record) = record.filter(|r| r.status == "active") else {
        return Err(VvError::configuration(format!(
            "No active import with id {import_id}"
        )));
    };
    match record.mode.as_str() {
        "new" => return undo_new(ctx, &record, config, transport),
        "swap" => return undo_swap(ctx, &record, config, transport),
        _ => {}
    }
    let Some(definition) = config.presentation_template.as_ref() else {
        return Err(VvError::configuration("No template is configured"));
    };
    let found = target_from_api(ctx, transport, Some(&record.target_uuid), None, None)?;
    let (presentation_name, path) = (found.name, found.path);
    let was_running = transport.is_running();
    if was_running {
        if !close_app {
            return Err(VvError::configuration(
                "ProPresenter is running; close it completely first",
            ));
        }
        ctx.progress("Closing ProPresenter");
        transport.quit_app()?;
        let deadline = ctx.mono() + config.node.quit_wait_seconds;
        while ctx.mono() < deadline && transport.is_running() {
            ctx.sleep(1.0);
        }
        if transport.is_running() {
            return Err(VvError::write(
                "ProPresenter did not close; close it yourself and retry. No files changed",
            ));
        }
    }
    let work = workdir()?;
    let local_copy = work.path().join(file_name_of(&path));
    let sha = transport.fetch_file(&path, &local_copy)?;
    let backup = transport.backup_file(&path)?;
    let mut writer = ctx.writer();
    let now = ctx.now_micros().div_euclid(1_000_000);
    let empty = SlidePlan {
        slides: Vec::new(),
        attribution_slides: Vec::new(),
        looks: None,
    };
    let written = append_plan_in_place(
        &mut writer,
        &local_copy,
        &empty,
        definition,
        None,
        None,
        Some(&record.cue_uuids),
        now,
    )?;
    transport.install_file(&local_copy, &path, &sha)?;
    if was_running && reopen {
        ctx.progress("Reopening ProPresenter");
        transport.launch_app()?;
    }
    ctx.history.set_status(import_id, "undone")?;
    let node = config
        .node
        .name
        .clone()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "local".into());
    let mut result = ApplyResult::blank(import_id, &presentation_name, &node);
    result.cue_count_before = written.result.cue_count_before as i64;
    result.cue_count_after = written.result.cue_count_after as i64;
    result.removed_slides = written.result.removed_cue_count as i64;
    result.backup_path = Some(backup);
    result.verification = "Slides removed".into();
    result.verified = true;
    result.reopened = was_running && reopen;
    Ok(result)
}

// ------------------------------------------------------------------------------------------------------------ playlists

/// A playlist that holds the presentation, with its current items (so the new entry can go right under it).
struct Slot {
    playlist: String,
    playlist_name: String,
    position: usize,
    /// Shared by every slot of one playlist, as the one list object of Python is.
    items: Rc<RefCell<Vec<Value>>>,
    by_name: bool,
}

/// Every playlist that holds the presentation, with its current items. Matches by id. If no playlist matches by id (ProPresenter can
/// re-issue ids) it falls back to the entry's name.
fn find_playlist_slots(
    transport: &dyn Transport,
    presentation_uuid: &str,
    name: Option<&str>,
) -> VvResult<Vec<Slot>> {
    let mut found = Vec::new();
    let mut by_name = Vec::new();
    fn walk(
        transport: &dyn Transport,
        nodes: &[Value],
        presentation_uuid: &str,
        name: Option<&str>,
        found: &mut Vec<Slot>,
        by_name: &mut Vec<Slot>,
    ) -> VvResult<()> {
        for node in nodes {
            let children = match node.get("children") {
                Some(Value::Array(a)) => a.clone(),
                _ => Vec::new(),
            };
            if node.get("field_type").and_then(Value::as_str) == Some("playlist")
                || children.is_empty()
            {
                let id = req(node, "id")?;
                let pid = req_str(id, "uuid")?;
                let items = transport
                    .api_get(&format!("/v1/playlist/{pid}"))
                    .and_then(|v| items_of(&v))
                    .unwrap_or_default();
                let shared = Rc::new(RefCell::new(items.clone()));
                for (position, item) in items.iter().enumerate() {
                    if item.get("type").and_then(Value::as_str) != Some("presentation") {
                        continue;
                    }
                    let info_uuid = item
                        .get("presentation_info")
                        .filter(|v| !v.is_null())
                        .map(|i| get_str(i, "presentation_uuid"))
                        .unwrap_or_default();
                    let slot = Slot {
                        playlist: pid.clone(),
                        playlist_name: req_str(id, "name")?,
                        position,
                        items: shared.clone(),
                        by_name: false,
                    };
                    if eq_fold(&info_uuid, presentation_uuid) {
                        found.push(slot);
                    } else if name.is_some_and(|n| {
                        !n.is_empty()
                            && eq_fold(
                                &item
                                    .get("id")
                                    .map(|i| get_str(i, "name"))
                                    .unwrap_or_default(),
                                n,
                            )
                    }) {
                        by_name.push(slot);
                    }
                }
            }
            walk(
                transport,
                &children,
                presentation_uuid,
                name,
                found,
                by_name,
            )?;
        }
        Ok(())
    }
    let nodes = as_list(&transport.api_get("/v1/playlists")?)?;
    walk(
        transport,
        &nodes,
        presentation_uuid,
        name,
        &mut found,
        &mut by_name,
    )?;
    Ok(if found.is_empty() { by_name } else { found })
}

/// Put the new presentation directly under the original in each playlist that had the original.
fn add_under_in_playlists(
    ctx: &mut Ctx,
    transport: &dyn Transport,
    slots: &[Slot],
    new_uuid: &str,
    new_name: &str,
) -> VvResult<Vec<Value>> {
    let mut added = Vec::new();
    for slot in slots {
        let item_id = ctx.uuid().to_uppercase();
        let entry = json!({
            "id": {"uuid": item_id, "name": new_name, "index": slot.position + 1},
            "type": "presentation", "is_hidden": false, "is_pco": false,
            "presentation_info": {"presentation_uuid": new_uuid, "arrangement_name": ""},
        });
        let mut merged: Vec<Value> = {
            let items = slot.items.borrow();
            let mut m: Vec<Value> = items[..slot.position + 1].to_vec();
            m.push(entry);
            m.extend(items[slot.position + 1..].iter().cloned());
            m
        };
        for (index, item) in merged.iter_mut().enumerate() {
            // keep the order numbers in step with the order
            set_index(item, index);
        }
        // the originals share their ids with the merged list in Python: bring them in step too
        {
            let mut items = slot.items.borrow_mut();
            let mut merged_iter = merged.iter().enumerate();
            for (i, original) in items.iter_mut().enumerate() {
                let want = if i <= slot.position { i } else { i + 1 };
                let _ = merged_iter.next();
                set_index(original, want);
            }
        }
        transport.api_put(
            &format!("/v1/playlist/{}", slot.playlist),
            &Value::Array(merged),
        )?;
        added.push(json!({"playlist": slot.playlist, "item": item_id, "name": new_name, "presentation": new_uuid}));
    }
    Ok(added)
}

/// `item.setdefault("id", {})["index"] = index`.
fn set_index(item: &mut Value, index: usize) {
    if let Value::Object(map) = item {
        let id = map.entry("id".to_string()).or_insert_with(|| json!({}));
        if let Value::Object(id) = id {
            id.insert("index".to_string(), json!(index));
        }
    }
}

fn remove_from_playlists(ctx: &mut Ctx, transport: &dyn Transport, record: &ImportRecord) {
    for entry in &record.playlist_items {
        let playlist = get_str(entry, "playlist");
        let shown_name = entry
            .get("name")
            .map(crate::pyrepr::py_str)
            .unwrap_or_else(|| "None".into());
        ctx.event(
            "playlist",
            "info",
            json!({"message": format!("removing {shown_name} from playlist {playlist}")}),
        );
        let attempt = (|| -> VvResult<()> {
            let items = items_of(&transport.api_get(&format!("/v1/playlist/{playlist}"))?)?;
            // ProPresenter hands out new item ids AND new presentation ids when it saves a playlist, so neither can be matched later.
            // The name is stable, and a copy always has a unique numbered name.
            let gone = casefold(&get_str(entry, "name"));
            let mut kept: Vec<Value> = items
                .iter()
                .filter(|i| {
                    !(i.get("type").and_then(Value::as_str) == Some("presentation")
                        && !gone.is_empty()
                        && casefold(&i.get("id").map(|x| get_str(x, "name")).unwrap_or_default())
                            == gone)
                })
                .cloned()
                .collect();
            for (index, item) in kept.iter_mut().enumerate() {
                set_index(item, index);
            }
            if kept.len() != items.len() {
                transport.api_put(&format!("/v1/playlist/{playlist}"), &Value::Array(kept))?;
            }
            Ok(())
        })();
        if let Err(exc) = attempt {
            ctx.event(
                "playlist",
                "warn",
                json!({"message": format!("could not take it out of the playlist: {exc}")}),
            );
            ctx.progress(&format!(
                "Could not take it out of the playlist ({exc}); remove it there by hand"
            ));
        }
    }
}

struct PlaylistUndo {
    playlist: String,
    items: Vec<Value>,
}

/// Make every playlist entry for this presentation open `new_uuid`. Returns the untouched items (to put back on failure).
fn repoint_playlists(
    ctx: &mut Ctx,
    transport: &dyn Transport,
    slots: &[Slot],
    old_uuid: &str,
    new_uuid: &str,
    name: &str,
) -> VvResult<Vec<PlaylistUndo>> {
    let mut undo = Vec::new();
    for slot in slots {
        let mut fresh = items_of(&transport.api_get(&format!("/v1/playlist/{}", slot.playlist))?)?;
        let before = fresh.clone();
        let mut changed = 0;
        for item in fresh.iter_mut() {
            let info_uuid = item
                .get("presentation_info")
                .filter(|v| !v.is_null())
                .map(|i| get_str(i, "presentation_uuid"))
                .unwrap_or_default();
            let item_name = item
                .get("id")
                .map(|i| get_str(i, "name"))
                .unwrap_or_default();
            let mine =
                eq_fold(&info_uuid, old_uuid) || (!name.is_empty() && eq_fold(&item_name, name));
            if item.get("type").and_then(Value::as_str) == Some("presentation")
                && mine
                && !eq_fold(&info_uuid, new_uuid)
            {
                // `info = item.get("presentation_info") or {}`: an absent or empty block is a copy nobody sees
                if let Some(Value::Object(info)) = item.get_mut("presentation_info") {
                    if !info.is_empty() {
                        info.insert("presentation_uuid".to_string(), json!(new_uuid));
                    }
                }
                changed += 1;
            }
        }
        if changed > 0 {
            transport.api_put(
                &format!("/v1/playlist/{}", slot.playlist),
                &Value::Array(fresh),
            )?;
            undo.push(PlaylistUndo {
                playlist: slot.playlist.clone(),
                items: before,
            });
            ctx.event(
                "playlist",
                "info",
                json!({"message": format!("entry in {} now opens the new file", slot.playlist_name), "playlist": slot.playlist_name, "entries": changed}),
            );
        }
    }
    Ok(undo)
}

/// Point the playlist entries at whatever id ProPresenter currently lists for the file, until they agree for 6 s in a row.
fn converge_playlists(
    ctx: &mut Ctx,
    transport: &dyn Transport,
    slots: &[Slot],
    lib_name: &str,
    name: &str,
    old_uuid: &str,
    timeout: f64,
) -> VvResult<bool> {
    let deadline = ctx.mono() + timeout;
    let mut agreed_since = 0.0_f64;
    while ctx.mono() < deadline {
        let current = library_uuid(transport, lib_name, name)?;
        let step = (|ctx: &mut Ctx| -> VvResult<Option<bool>> {
            let mut all_ok = !current.is_empty();
            if all_ok {
                for slot in slots {
                    if !playlist_ok(transport, slot, name, &current)? {
                        all_ok = false;
                        break;
                    }
                }
            }
            if all_ok {
                if agreed_since == 0.0 {
                    agreed_since = ctx.mono();
                }
                if ctx.mono() - agreed_since >= 6.0 {
                    ctx.event(
                        "swap_pacing",
                        "info",
                        json!({"message": format!("playlist and library agree on {}", current.chars().take(8).collect::<String>())}),
                    );
                    return Ok(Some(true));
                }
            } else {
                agreed_since = 0.0;
                if !current.is_empty() {
                    repoint_playlists(ctx, transport, slots, old_uuid, &current, name)?;
                }
            }
            Ok(None)
        })(ctx);
        match step {
            Ok(Some(done)) => return Ok(done),
            Ok(None) => {}
            Err(exc) if exc.is_connection() => {
                agreed_since = 0.0;
                let short: String = exc.message.chars().take(120).collect();
                ctx.event(
                    "swap_pacing",
                    "warn",
                    json!({"message": format!("playlist not ready yet: {short}")}),
                );
            }
            Err(other) => return Err(other),
        }
        ctx.sleep(2.0);
    }
    ctx.event(
        "swap_pacing",
        "warn",
        json!({"message": "the playlist entry never settled on the new id; it was left as it was"}),
    );
    Ok(false)
}

/// Wait until ProPresenter's library listing satisfies `ready(names)`.
fn wait_library(
    ctx: &mut Ctx,
    transport: &dyn Transport,
    lib_name: &str,
    ready: &dyn Fn(&BTreeSet<String>) -> bool,
    timeout: f64,
) -> bool {
    let deadline = ctx.mono() + timeout;
    while ctx.mono() < deadline {
        if let Ok(list) = list_presentations(transport) {
            let names: BTreeSet<String> = list
                .into_iter()
                .filter(|i| eq_fold(&i.library, lib_name))
                .map(|i| i.name)
                .collect();
            if ready(&names) {
                return true;
            }
        }
        ctx.sleep(0.5);
    }
    false
}

fn library_uuid(transport: &dyn Transport, lib_name: &str, name: &str) -> VvResult<String> {
    let found: Vec<String> = list_presentations(transport)?
        .into_iter()
        .filter(|i| i.name == name && eq_fold(&i.library, lib_name))
        .map(|i| i.uuid)
        .collect();
    Ok(if found.len() == 1 {
        found[0].clone()
    } else {
        String::new()
    })
}

fn playlist_ok(transport: &dyn Transport, slot: &Slot, name: &str, uuid: &str) -> VvResult<bool> {
    let items = items_of(&transport.api_get(&format!("/v1/playlist/{}", slot.playlist))?)?;
    let entries: Vec<&Value> = items
        .iter()
        .filter(|i| {
            i.get("type").and_then(Value::as_str) == Some("presentation")
                && eq_fold(
                    &i.get("id").map(|x| get_str(x, "name")).unwrap_or_default(),
                    name,
                )
        })
        .collect();
    Ok(!entries.is_empty()
        && entries.iter().all(|i| {
            eq_fold(
                &i.get("presentation_info")
                    .filter(|v| !v.is_null())
                    .map(|x| get_str(x, "presentation_uuid"))
                    .unwrap_or_default(),
                uuid,
            )
        }))
}

// ------------------------------------------------------------------------------------------------------------ a new file

/// `_wait_listed`.
#[allow(clippy::too_many_arguments)]
fn wait_listed(
    ctx: &mut Ctx,
    transport: &dyn Transport,
    name: &str,
    lib_name: &str,
    plan: &SlidePlan,
    expected: usize,
    timeout: f64,
    file_check: Option<&FileCheck>,
    file_uuid_value: &str,
) -> (bool, String, String) {
    let deadline = ctx.mono() + timeout;
    let mut problem = "ProPresenter has not listed the new presentation yet".to_string();
    let mut blank_since: Option<f64> = None;
    while ctx.mono() < deadline {
        let attempt = (|ctx: &mut Ctx| -> VvResult<Option<(bool, String, String)>> {
            let listing = list_presentations(transport)?;
            let matched = listing
                .into_iter()
                .find(|i| i.name == name && eq_fold(&i.library, lib_name));
            if let (Some(m), Some(check)) = (&matched, file_check) {
                if m.uuid.is_empty() {
                    // listed but not loaded (blank id): ProPresenter loads it when it is first opened. Check the file itself.
                    let since = *blank_since.get_or_insert(ctx.mono());
                    if ctx.mono() - since >= ctx.timing.load_grace {
                        let (ok, message) = verify_file_on_disk(ctx, transport, check);
                        if ok {
                            return Ok(Some((
                                true,
                                "ProPresenter lists it but loads it only when it is first opened; the file on disk is intact".into(),
                                file_uuid_value.to_string(),
                            )));
                        }
                        problem = message;
                    }
                    return Ok(None);
                }
            }
            if let Some(m) = matched {
                // short: ProPresenter may re-issue the id, so re-read the listing
                let t = (deadline - ctx.mono()).clamp(1.0, 4.0);
                let (ok, message) = verify_via_api(ctx, transport, &m.uuid, plan, expected, t);
                if ok {
                    return Ok(Some((true, message, m.uuid)));
                }
                problem = message.clone();
                if let Some(check) = file_check {
                    if message.contains(NOT_LOADED) {
                        // listed with an id, but ProPresenter answers 404 for it: not loaded yet
                        let since = *blank_since.get_or_insert(ctx.mono());
                        if ctx.mono() - since >= ctx.timing.load_grace {
                            let (ok, check_message) = verify_file_on_disk(ctx, transport, check);
                            if ok {
                                let id = if file_uuid_value.is_empty() {
                                    m.uuid
                                } else {
                                    file_uuid_value.to_string()
                                };
                                return Ok(Some((
                                    true,
                                    "ProPresenter lists it but has not loaded it yet (it loads when first opened); the file on disk is intact".into(),
                                    id,
                                )));
                            }
                            problem = check_message;
                        }
                    }
                }
            }
            Ok(None)
        })(ctx);
        match attempt {
            Ok(Some(done)) => return done,
            Ok(None) => {}
            Err(exc) => problem = exc.message,
        }
        ctx.sleep(1.5);
    }
    (false, problem, String::new())
}

/// `publish_new`: write the result as a NEW file. ProPresenter lists new files at once, so it never needs to restart.
///
/// `base: None` is a fresh presentation named after the document; `base: Some(target)` a COPY of that presentation (with `clear_generated`
/// its verse and point slides are dropped first: Replace; without, the new slides go at the end: Append). The copy is saved next to the
/// original as "Name 2", "Name 3"... and, when the original is in a playlist, it is added just under it.
#[allow(clippy::too_many_arguments)]
pub fn publish_new(
    ctx: &mut Ctx,
    text: &str,
    source_name: &str,
    title: &str,
    plan: &Planned,
    config: &AppConfig,
    transport: &dyn Transport,
    library: Option<&str>,
    verify_timeout: f64,
    base: Option<&Target>,
    clear_generated: bool,
    numbered: bool,
    add_to_playlists: bool,
) -> VvResult<ApplyResult> {
    let Some(definition) = config.presentation_template.as_ref() else {
        return Err(VvError::configuration(
            "No template is configured; run versevision template setup",
        ));
    };
    if plan.result.slide_plan.slides.is_empty() {
        return Err(VvError::configuration("There are no slides to import"));
    }
    let node_name = config
        .node
        .name
        .clone()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "local".into());
    let (lib_name, wanted) = match base {
        Some(b) => (b.library.clone(), b.name.clone()),
        None => (
            library
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| config.output.library.clone()),
            safe_presentation_name(title, "Sermon Notes"),
        ),
    };
    let folder = transport.library_folder(&lib_name);

    let mut existing: BTreeSet<String> = BTreeSet::new();
    if let Ok(list) = list_presentations(transport) {
        existing = list
            .into_iter()
            .filter(|i| eq_fold(&i.library, &lib_name))
            .map(|i| i.name)
            .collect();
    }
    let pick = |existing: &BTreeSet<String>| {
        if numbered || base.is_some() {
            numbered_name(&wanted, existing)
        } else {
            unique_name(&wanted, existing)
        }
    };
    let mut name = pick(&existing);
    for _ in 0..50 {
        // also guard against a file on disk that the listing does not show
        if transport
            .find_presentation(&name, Some(&lib_name))
            .is_empty()
        {
            break;
        }
        let mut more = existing.clone();
        more.insert(name.clone());
        name = pick(&more);
    }
    let mut steps = vec![format!(
        "new file: {name}{}",
        base.map(|b| format!(" (copy of {})", b.name))
            .unwrap_or_default()
    )];

    let template = template_target(ctx, transport, definition)?;
    let work = workdir()?;
    let template_local = work.path().join("template.pro");
    ctx.progress("Reading the template");
    transport.fetch_file(&template.path, &template_local)?;
    validate_template(&read_presentation(&fs::read(&template_local)?)?, definition)?;
    let out_local = work.path().join(format!("{name}.pro"));
    let mut writer = ctx.writer();
    let now = ctx.now_micros().div_euclid(1_000_000);
    let written = if let Some(b) = base {
        let base_local = work.path().join("base.pro");
        ctx.progress(&format!("Copying {}", b.name));
        transport.fetch_file(&b.path, &base_local)?;
        ctx.progress("Building the new file");
        derive_presentation_file(
            &mut writer,
            &base_local,
            &out_local,
            &plan.result.slide_plan,
            definition,
            plan.looks.as_ref(),
            Some(&template_local),
            &name,
            clear_generated,
            false,
            now,
        )?
        .result
    } else {
        ctx.progress("Building the presentation");
        create_presentation_file(
            &mut writer,
            &template_local,
            &out_local,
            &plan.result.slide_plan,
            definition,
            plan.looks.as_ref(),
            &name,
            now,
        )?
        .result
    };
    let remote_path = format!("{folder}/{name}.pro");
    ctx.progress("Adding it to ProPresenter");
    transport.create_file(&out_local, &remote_path)?;
    steps.push(format!("placed {remote_path}"));
    ctx.event(
        "file_placed",
        "info",
        json!({
            "message": remote_path, "path": remote_path, "slides": written.cue_count_after,
            "sha256": written.output_sha256.chars().take(16).collect::<String>(),
            "original_sha256": written.original_sha256.chars().take(16).collect::<String>(),
            "removed": written.removed_cue_count,
        }),
    );

    let expected_total = written.cue_count_after;
    let verified;
    let message: String;
    let mut new_uuid = String::new();
    let mut playlist_items: Vec<Value> = Vec::new();
    if transport.is_running() {
        ctx.progress("Waiting for ProPresenter to show it");
        let texts: Vec<String> = plan
            .result
            .slide_plan
            .slides
            .iter()
            .map(|x| x.body_text.clone())
            .collect();
        let file_uuid_value = file_uuid(&out_local)?;
        let check = FileCheck {
            path: &remote_path,
            sha: &written.output_sha256,
            local: Some(&out_local),
            texts: &texts,
        };
        let (ok, msg, uuid) = wait_listed(
            ctx,
            transport,
            &name,
            &lib_name,
            &plan.result.slide_plan,
            plan.result.slide_plan.slides.len(),
            verify_timeout,
            Some(&check),
            &file_uuid_value,
        );
        verified = ok;
        message = msg;
        new_uuid = uuid;
        ctx.event(
            "verify",
            if verified { "info" } else { "error" },
            json!({"message": message, "verified": verified, "presentation_uuid": new_uuid}),
        );
        if !verified {
            return Err(VvError::rollback(format!(
                "ProPresenter did not show the new file ({message}). The file {remote_path} was left in place; \
                 reopen ProPresenter to see it, or remove the file."
            )));
        }
        if let (Some(b), true) = (base, add_to_playlists) {
            let attempt = (|ctx: &mut Ctx| -> VvResult<()> {
                let slots = find_playlist_slots(transport, &b.uuid, None)?;
                if !slots.is_empty() {
                    ctx.progress("Adding it to the playlist");
                    playlist_items =
                        add_under_in_playlists(ctx, transport, &slots, &new_uuid, &name)?;
                    let names: Vec<String> =
                        slots.iter().map(|s| s.playlist_name.clone()).collect();
                    ctx.event(
                        "playlist",
                        "info",
                        json!({
                            "message": format!("added under the original in {}", names.join(", ")),
                            "playlists": names,
                            "positions": slots.iter().map(|s| s.position + 1).collect::<Vec<_>>(),
                        }),
                    );
                    let unique: BTreeSet<String> = names.into_iter().collect();
                    steps.push(format!(
                        "added to playlist {}",
                        unique.into_iter().collect::<Vec<_>>().join(", ")
                    ));
                }
                Ok(())
            })(ctx);
            if let Err(exc) = attempt {
                // the file is in place and listed; a playlist problem should not undo that
                steps.push(format!("could not add it to the playlist: {exc}"));
                ctx.event(
                    "playlist",
                    "warn",
                    json!({"message": format!("could not add it to the playlist: {exc}")}),
                );
            }
        }
    } else {
        verified = true;
        message = "Placed in the library folder; ProPresenter will list it the next time it opens"
            .to_string();
    }

    let import_id = ctx.short_id();
    ctx.event(
        "history_recorded",
        "info",
        json!({"message": format!("import {import_id}"), "import_id": import_id, "cue_uuids": written.cue_uuids.len()}),
    );
    let target_uuid = if new_uuid.is_empty() {
        written.cue_uuids.first().cloned().unwrap_or_default()
    } else {
        new_uuid.clone()
    };
    let mut record = new_record(
        ctx,
        &import_id,
        source_name,
        text,
        &target_uuid,
        &name,
        &node_name,
    );
    record.cue_uuids = written.cue_uuids.clone();
    record.slide_count = written.cue_uuids.len() as i64;
    record.verse_count = plan.result.verse_count as i64;
    record.point_count = plan.result.point_count as i64;
    record.mode = "new".into();
    record.library = Some(lib_name.clone());
    record.file_path = Some(remote_path.clone());
    record.copied_from = base.map(|b| b.name.clone());
    record.playlist_items = playlist_items;
    record.file_cue_count = expected_total as i64;
    ctx.history.add(record)?;
    append_audit(
        ctx,
        if new_uuid.is_empty() {
            None
        } else {
            Some(&new_uuid)
        },
        &name,
        plan.result.verse_count,
        written.cue_uuids.len(),
        versions_of(&plan.result.slide_plan),
        None,
        &message,
    )?;
    let mut result = ApplyResult::blank(&import_id, &name, &node_name);
    result.mode = if base.is_some() && clear_generated {
        "replace"
    } else if base.is_some() {
        "append"
    } else {
        "new"
    }
    .into();
    result.presentation_name = name;
    result.slide_count = written.cue_uuids.len() as i64;
    result.cue_count_before = written.cue_count_before as i64;
    result.cue_count_after = expected_total as i64;
    result.removed_slides = written.removed_cue_count as i64;
    result.verification = message;
    result.verified = verified;
    result.steps = steps;
    Ok(result)
}

// ------------------------------------------------------------------------------------------------------------ the swap

/// `_wait_stable`: wait until ProPresenter lists the name with the same id for `stable` seconds in a row AND shows the right slides.
/// It re-issues ids for a moment after a file lands, so an id seen once cannot be trusted.
fn wait_stable(
    ctx: &mut Ctx,
    transport: &dyn Transport,
    lib_name: &str,
    name: &str,
    old_uuid: &str,
    verify: &Verifier,
    timeout: f64,
) -> VvResult<String> {
    let deadline = ctx.mono() + timeout;
    let (mut seen, mut since, mut problem) = (
        String::new(),
        0.0_f64,
        "ProPresenter has not listed the new file yet".to_string(),
    );
    let mut missing_since: Option<f64> = None;
    while ctx.mono() < deadline {
        let attempt = (|ctx: &mut Ctx| -> VvResult<Option<String>> {
            let current = library_uuid(transport, lib_name, name)?;
            if !current.is_empty() && !eq_fold(&current, old_uuid) {
                if current != seen {
                    seen = current.clone();
                    since = ctx.mono();
                }
                let (ok, p) = verify.check(ctx, transport, &current);
                problem = p;
                if ok && ctx.mono() - since >= ctx.timing.stable {
                    return Ok(Some(current));
                }
                if !ok {
                    since = ctx.mono();
                }
                if !ok && problem.contains(NOT_LOADED) {
                    let first = *missing_since.get_or_insert(ctx.mono());
                    if ctx.mono() - first >= ctx.timing.load_grace {
                        return Err(VvError::not_loaded(format!(
                            "ProPresenter lists {name} but answers 404 when asked for it ({problem})"
                        )));
                    }
                } else {
                    missing_since = None;
                }
            }
            Ok(None)
        })(ctx);
        match attempt {
            Ok(Some(done)) => return Ok(done),
            Ok(None) => {}
            Err(e) if e.kind == Kind::NotLoaded => return Err(e),
            Err(exc) => {
                problem = exc.message;
                seen = String::new();
            }
        }
        ctx.sleep(1.0);
    }
    Err(VvError::rollback(format!(
        "ProPresenter did not show the new version ({problem})"
    )))
}

/// `_settle_new_version`: wait for ProPresenter to show the swapped-in file. Returns the id the playlists should use. ProPresenter
/// sometimes lists a new file without loading it (blank id) until someone opens it; then the file on disk is checked instead, and it
/// loads the next time it is opened.
#[allow(clippy::too_many_arguments)]
fn settle_new_version(
    ctx: &mut Ctx,
    transport: &dyn Transport,
    lib_name: &str,
    name: &str,
    path: &str,
    old_uuid: &str,
    written_sha: &str,
    verify: &Verifier,
    written_local: Option<&Path>,
    expect_texts: &[String],
) -> VvResult<String> {
    let deadline = ctx.mono() + ctx.timing.swap_wait;
    let mut blank_since: Option<f64> = None;
    let mut problem = "ProPresenter has not listed the new file yet".to_string();
    while ctx.mono() < deadline {
        let attempt = (|ctx: &mut Ctx| -> VvResult<Option<Result<String, ()>>> {
            let listed: Vec<Presentation> = list_presentations(transport)?
                .into_iter()
                .filter(|i| i.name == name && eq_fold(&i.library, lib_name))
                .collect();
            if let Some(first) = listed.first() {
                if !first.uuid.is_empty() {
                    let remaining = (deadline - ctx.mono()).max(5.0);
                    return match wait_stable(ctx, transport, lib_name, name, "", verify, remaining)
                    {
                        Ok(done) => Ok(Some(Ok(done))),
                        Err(e) if e.kind == Kind::NotLoaded => {
                            // listed with an id, but ProPresenter will not return it: same as a blank id
                            problem = e.message;
                            Ok(Some(Err(())))
                        }
                        Err(other) => Err(other),
                    };
                }
            }
            if !listed.is_empty() {
                let since = *blank_since.get_or_insert(ctx.mono());
                if ctx.mono() - since >= ctx.timing.load_grace {
                    return Ok(Some(Err(())));
                }
            }
            Ok(None)
        })(ctx);
        match attempt {
            Ok(Some(Ok(done))) => return Ok(done),
            Ok(Some(Err(()))) => break,
            Ok(None) => {}
            Err(exc) => problem = exc.message,
        }
        ctx.sleep(1.0);
    }
    let check = FileCheck {
        path,
        sha: written_sha,
        local: written_local,
        texts: expect_texts,
    };
    let (ok, message) = verify_file_on_disk(ctx, transport, &check);
    if !ok {
        return Err(VvError::rollback(format!(
            "ProPresenter did not show the new version ({problem}; {message})"
        )));
    }
    ctx.event(
        "verify",
        "warn",
        json!({"message": "ProPresenter lists the new version but has not loaded it yet; the file on disk is intact, so it keeps its place in the playlists and loads when opened", "verified": true}),
    );
    Ok(old_uuid.to_string())
}

/// What `_swap_core` returns.
pub struct SwapOutcome {
    pub new_uuid: String,
    pub old_uuid: String,
    pub backup: String,
    pub sha256: String,
    pub steps: Vec<String>,
    pub playlists: usize,
}

struct SwapState {
    renamed: bool,
    created: bool,
    playlist_undo: Vec<PlaylistUndo>,
}

/// Put `new_local` in place of the presentation called `name` without restarting ProPresenter.
///
/// The steps: back the original up, rename it out of the way, write the new file under the original name, wait until ProPresenter lists and
/// shows it, repoint the playlist entries, delete the renamed original. Any failure puts everything back exactly as it was.
#[allow(clippy::too_many_arguments)]
fn swap_core(
    ctx: &mut Ctx,
    transport: &dyn Transport,
    lib_name: &str,
    name: &str,
    new_local: &Path,
    verify: &Verifier,
    expect_texts: &[String],
) -> VvResult<SwapOutcome> {
    if !transport.is_running() {
        return Err(VvError::configuration(
            "ProPresenter must be open for this (it keeps playlists and the library in memory). \
             Open it, or use --in-place which closes and reopens it",
        ));
    }
    let folder = transport.library_folder(lib_name);
    let path = format!("{folder}/{name}.pro");
    let listing = list_presentations(transport)?;
    let mine: Vec<&Presentation> = listing
        .iter()
        .filter(|i| i.name == name && eq_fold(&i.library, lib_name))
        .collect();
    if mine.len() != 1 || transport.find_presentation(name, Some(lib_name)).is_empty() {
        return Err(VvError::configuration(format!(
            "Could not find exactly one presentation named {} in {lib_name}",
            py_repr(name)
        )));
    }
    let old_uuid = mine[0].uuid.clone();
    let existing: BTreeSet<String> = listing
        .iter()
        .filter(|i| eq_fold(&i.library, lib_name))
        .map(|i| i.name.clone())
        .collect();
    // a name that has never been used: ProPresenter keeps stale entries for paths that were deleted and then reused
    let short = ctx.short_id()[..6].to_string();
    let temp_name = unique_name(&format!("{name} (old {short})"), &existing);
    let temp_path = format!("{folder}/{temp_name}.pro");

    let work = workdir()?;
    ctx.progress("Backing up the original");
    let old_sha = transport.fetch_file(&path, &work.path().join("old.pro"))?;
    let backup = transport.backup_file(&path)?;
    let mut slots = find_playlist_slots(transport, &old_uuid, Some(name))?;
    for slot in slots.iter_mut() {
        slot.by_name = !slot.items.borrow().iter().any(|i| {
            eq_fold(
                &i.get("presentation_info")
                    .filter(|v| !v.is_null())
                    .map(|x| get_str(x, "presentation_uuid"))
                    .unwrap_or_default(),
                &old_uuid,
            )
        });
    }
    let mut steps = vec![format!("backup: {backup}")];
    ctx.event(
        "swap_start",
        "info",
        json!({
            "message": format!("{name}: original {}, {} playlist entries", old_uuid.chars().take(8).collect::<String>(), slots.len()),
            "backup": backup, "old_sha256": old_sha.chars().take(16).collect::<String>(), "temp": temp_name,
        }),
    );

    ctx.progress("Renaming the original");
    transport.rename_file(&path, &temp_path)?;
    // Do NOT query ProPresenter here: measured on the iMac, reading its library right after a rename makes it keep a dead entry for the
    // old name. A plain pause lets it notice the rename on its own.
    ctx.sleep(ctx.timing.min_gap);
    let mut state = SwapState {
        renamed: true,
        created: false,
        playlist_undo: Vec::new(),
    };

    let new_cue_count = cue_count_of(new_local)?;
    let mut created_sha = String::new();
    let outcome = (|ctx: &mut Ctx, state: &mut SwapState| -> VvResult<(String, Vec<String>)> {
        ctx.progress("Writing the new file");
        created_sha = transport.create_file(new_local, &path)?;
        state.created = true;
        steps.push(format!(
            "renamed original to {temp_name}; wrote new {name}.pro"
        ));
        ctx.event(
            "file_placed",
            "info",
            json!({"message": path, "path": path, "sha256": created_sha.chars().take(16).collect::<String>()}),
        );

        ctx.progress("Waiting for ProPresenter to show it");
        // Quiet period: asking ProPresenter about a file in the first seconds after it lands can leave it with an id that does not match
        // the file (the entry then cannot be opened). Measured on the iMac: no queries here = it works.
        ctx.sleep(ctx.timing.quiet);
        let mut new_uuid = settle_new_version(
            ctx,
            transport,
            lib_name,
            name,
            &path,
            &old_uuid,
            &created_sha,
            verify,
            Some(new_local),
            expect_texts,
        )?;

        let mut playlist_left = false;
        if !slots.is_empty() {
            ctx.progress("Updating the playlist");
            let mut settled = false;
            'attempts: for _attempt in 0..4 {
                match repoint_playlists(ctx, transport, &slots, &old_uuid, &new_uuid, name) {
                    Ok(done) => {
                        if state.playlist_undo.is_empty() {
                            state.playlist_undo = done;
                        }
                    }
                    Err(exc) if exc.is_connection() => {
                        // ProPresenter refuses the id of a file it has not loaded yet; the entry then finds the file by name when it is opened
                        ctx.event(
                            "swap_pacing",
                            "warn",
                            json!({"message": format!("the playlist would not take the new id ({exc}); left as it was")}),
                        );
                        playlist_left = true;
                        settled = true;
                        break 'attempts;
                    }
                    Err(other) => return Err(other),
                }
                ctx.sleep(2.5);
                let current = library_uuid(transport, lib_name, name)?;
                if !current.is_empty() && !eq_fold(&current, &new_uuid) {
                    ctx.event(
                        "swap_pacing",
                        "warn",
                        json!({"message": format!("ProPresenter re-issued the id ({} -> {}); following it",
                            new_uuid.chars().take(8).collect::<String>(), current.chars().take(8).collect::<String>())}),
                    );
                    match wait_stable(
                        ctx,
                        transport,
                        lib_name,
                        name,
                        &old_uuid,
                        verify,
                        ctx.timing.swap_wait,
                    ) {
                        Ok(id) => new_uuid = id,
                        Err(e) if e.kind == Kind::NotLoaded => {
                            // ProPresenter lists the file but will not return it (not loaded yet). The playlist API refuses ids of files
                            // that are not loaded, so the playlist is left as it was; the file on disk is intact and the entry loads
                            // when first opened.
                            ctx.event(
                                "swap_pacing",
                                "warn",
                                json!({"message": "ProPresenter has not loaded the new file yet, so the playlist entry was left unchanged"}),
                            );
                            playlist_left = true;
                            settled = true;
                            break 'attempts;
                        }
                        Err(other) => return Err(other),
                    }
                    continue;
                }
                let mut all = true;
                for slot in &slots {
                    if !playlist_ok(transport, slot, name, &new_uuid)? {
                        all = false;
                        break;
                    }
                }
                if all {
                    settled = true;
                    break 'attempts;
                }
            }
            if !settled {
                return Err(VvError::write(
                    "the playlist would not settle on the new version",
                ));
            }
            if playlist_left {
                // ProPresenter hands out a new id while it takes the file in, and refuses a playlist entry for an id it cannot open yet.
                // Keep following the id it lists until the playlist entry and the listing agree for a few seconds.
                ctx.progress("Waiting for ProPresenter to take the new file in");
                let timeout = ctx.timing.playlist_converge;
                playlist_left = !converge_playlists(
                    ctx, transport, &slots, lib_name, name, &old_uuid, timeout,
                )?;
            }
            let names: BTreeSet<String> = slots.iter().map(|s| s.playlist_name.clone()).collect();
            let joined = names.into_iter().collect::<Vec<_>>().join(", ");
            steps.push(if playlist_left {
                format!("playlist left as it was (ProPresenter has not loaded the new file yet; it loads when first opened): {joined}")
            } else {
                format!("playlist now opens the new file: {joined}")
            });
        }

        ctx.progress("Removing the renamed original");
        // ProPresenter gives a renamed file a new id and name, so compare what matters: the same slides
        let check = work.path().join("renamed.pro");
        let now_sha = transport.fetch_file(&temp_path, &check)?;
        if now_sha != old_sha {
            let before = read_presentation(&fs::read(work.path().join("old.pro"))?)?;
            let after = read_presentation(&fs::read(&check)?)?;
            let cues = |m: &Message| {
                m.msgs("cues")
                    .iter()
                    .map(|c| uuid_of(c).to_string())
                    .collect::<Vec<_>>()
            };
            let groups = |m: &Message| {
                m.msgs("cue_groups")
                    .iter()
                    .map(|g| {
                        g.msg("group")
                            .map(|x| uuid_of(x).to_string())
                            .unwrap_or_default()
                    })
                    .collect::<Vec<_>>()
            };
            if cues(&before) != cues(&after) || groups(&before) != groups(&after) {
                return Err(VvError::write(format!(
                    "{temp_name}.pro changed since VerseVision read it, so it was not deleted"
                )));
            }
            ctx.event(
                "swap_note",
                "info",
                json!({"message": "the renamed original was rewritten by ProPresenter but holds the same slides"}),
            );
        }
        let outcome_text = transport.delete_file(&temp_path, &now_sha)?;
        state.renamed = false;
        let settle = ctx.timing.settle;
        let gone = temp_name.clone();
        if !wait_library(
            ctx,
            transport,
            lib_name,
            &move |n| !n.contains(&gone),
            settle,
        ) {
            ctx.event(
                "swap_pacing",
                "warn",
                json!({"message": format!("ProPresenter still lists {} after it was deleted", py_repr(&temp_name))}),
            );
        }
        steps.push(format!("removed {temp_name}.pro ({outcome_text})"));
        ctx.event(
            "swap_done",
            "info",
            json!({"message": format!("{name} replaced; original removed ({outcome_text})")}),
        );
        Ok((new_uuid, steps.clone()))
    })(ctx, &mut state);
    match outcome {
        Ok((new_uuid, steps)) => Ok(SwapOutcome {
            new_uuid,
            old_uuid,
            backup,
            sha256: created_sha,
            steps,
            playlists: slots.len(),
        }),
        Err(exc) => {
            swap_rollback(
                ctx,
                transport,
                lib_name,
                name,
                &path,
                &temp_path,
                &state,
                new_cue_count,
                &exc.message,
            )?;
            Err(exc)
        }
    }
}

/// The `rollback` of `_swap_core`: playlists, then the new file, then the original's name.
#[allow(clippy::too_many_arguments)]
fn swap_rollback(
    ctx: &mut Ctx,
    transport: &dyn Transport,
    lib_name: &str,
    name: &str,
    path: &str,
    temp_path: &str,
    state: &SwapState,
    new_cue_count: usize,
    reason: &str,
) -> VvResult<()> {
    ctx.event("swap_rollback", "error", json!({"message": reason}));
    for entry in &state.playlist_undo {
        if let Err(exc) = transport.api_put(
            &format!("/v1/playlist/{}", entry.playlist),
            &Value::Array(entry.items.clone()),
        ) {
            ctx.event(
                "swap_rollback",
                "error",
                json!({"message": format!("could not restore a playlist: {exc}")}),
            );
            break;
        }
    }
    if state.created {
        if let Err(exc) = transport.delete_created_file(path, new_cue_count) {
            ctx.event(
                "swap_rollback",
                "error",
                json!({"message": format!("could not remove the new file: {exc}")}),
            );
        }
    }
    if state.created {
        // let it notice the removal first
        let settle = ctx.timing.settle;
        let gone = name.to_string();
        wait_library(
            ctx,
            transport,
            lib_name,
            &move |n| !n.contains(&gone),
            settle,
        );
        ctx.sleep(ctx.timing.min_gap);
    }
    if state.renamed {
        if let Err(exc) = transport.rename_file(temp_path, path) {
            return Err(VvError::write(format!(
                "{reason}. Your original is safe at {temp_path} (rename it back to {name}.pro): {exc}"
            )));
        }
    }
    Ok(())
}

/// `publish_swap`: replace or append in the chosen presentation WITHOUT restarting ProPresenter and without a second entry in the playlist.
#[allow(clippy::too_many_arguments)]
pub fn publish_swap(
    ctx: &mut Ctx,
    text: &str,
    source_name: &str,
    plan: &Planned,
    config: &AppConfig,
    transport: &dyn Transport,
    base: &Target,
    clear_generated: bool,
) -> VvResult<ApplyResult> {
    let Some(definition) = config.presentation_template.as_ref() else {
        return Err(VvError::configuration(
            "No template is configured; run versevision template setup",
        ));
    };
    if plan.result.slide_plan.slides.is_empty() {
        return Err(VvError::configuration("There are no slides to import"));
    }
    let (lib_name, name) = (base.library.clone(), base.name.clone());
    let node_name = config
        .node
        .name
        .clone()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "local".into());
    let work = workdir()?;
    let template = template_target(ctx, transport, definition)?;
    let template_local = work.path().join("template.pro");
    ctx.progress("Reading the template");
    transport.fetch_file(&template.path, &template_local)?;
    validate_template(&read_presentation(&fs::read(&template_local)?)?, definition)?;
    let base_local = work.path().join("base.pro");
    ctx.progress(&format!("Reading {name}"));
    transport.fetch_file(&base.path, &base_local)?;
    let out_local = work.path().join(format!("{name}.pro"));
    ctx.progress("Building the new version");
    let mut writer = ctx.writer();
    let now = ctx.now_micros().div_euclid(1_000_000);
    let written = derive_presentation_file(
        &mut writer,
        &base_local,
        &out_local,
        &plan.result.slide_plan,
        definition,
        plan.looks.as_ref(),
        Some(&template_local),
        &name,
        clear_generated,
        config.output.swap_keeps_id,
        now,
    )?
    .result;
    let expected = plan.result.slide_plan.slides.len();
    let texts: Vec<String> = plan
        .result
        .slide_plan
        .slides
        .iter()
        .map(|x| x.body_text.clone())
        .collect();
    let verify = Verifier::Plan {
        plan: &plan.result.slide_plan,
        expected,
    };
    let core = swap_core(
        ctx, transport, &lib_name, &name, &out_local, &verify, &texts,
    )?;
    let import_id = ctx.short_id();
    ctx.event(
        "history_recorded",
        "info",
        json!({"message": format!("import {import_id}"), "import_id": import_id}),
    );
    let mut record = new_record(
        ctx,
        &import_id,
        source_name,
        text,
        &core.new_uuid,
        &name,
        &node_name,
    );
    record.cue_uuids = written.cue_uuids.clone();
    record.slide_count = written.cue_uuids.len() as i64;
    record.verse_count = plan.result.verse_count as i64;
    record.point_count = plan.result.point_count as i64;
    record.backup_path = Some(core.backup.clone());
    record.mode = "swap".into();
    record.library = Some(lib_name.clone());
    record.file_path = Some(format!(
        "{}/{name}.pro",
        transport.library_folder(&lib_name)
    ));
    record.copied_from = Some(name.clone());
    record.file_sha256 = core.sha256.clone();
    record.file_cue_count = written.cue_count_after as i64;
    ctx.history.add(record)?;
    append_audit(
        ctx,
        Some(&core.new_uuid),
        &name,
        plan.result.verse_count,
        written.cue_uuids.len(),
        versions_of(&plan.result.slide_plan),
        if core.backup.is_empty() {
            None
        } else {
            Some(&core.backup)
        },
        "ProPresenter shows the new version",
    )?;
    let mut result = ApplyResult::blank(&import_id, &name, &node_name);
    result.mode = if clear_generated { "replace" } else { "append" }.into();
    result.presentation_name = name;
    result.slide_count = written.cue_uuids.len() as i64;
    result.cue_count_before = written.cue_count_before as i64;
    result.cue_count_after = written.cue_count_after as i64;
    result.removed_slides = written.removed_cue_count as i64;
    result.backup_path = Some(core.backup);
    result.verification = format!("ProPresenter reports all {expected} generated slides");
    result.verified = true;
    result.steps = core.steps;
    Ok(result)
}

/// `_undo_swap`: put the previous version back the same way (rename, write, repoint, delete).
fn undo_swap(
    ctx: &mut Ctx,
    record: &ImportRecord,
    config: &AppConfig,
    transport: &dyn Transport,
) -> VvResult<ApplyResult> {
    let (Some(backup_path), Some(lib)) = (
        record.backup_path.as_deref().filter(|b| !b.is_empty()),
        record.library.as_deref().filter(|l| !l.is_empty()),
    ) else {
        return Err(VvError::configuration(
            "This import has no backup recorded, so it cannot be undone automatically",
        ));
    };
    let name = record.target_name.clone();
    let folder = transport.library_folder(lib);
    let path = format!("{folder}/{name}.pro");
    let work = workdir()?;
    let current = work.path().join("current.pro");
    ctx.progress("Checking nothing changed since the import");
    let sha = transport.fetch_file(&path, &current)?;
    let cur = read_presentation(&fs::read(&current)?)?;
    // ProPresenter rewrites a file it has loaded, so a checksum is not a fair test; the slides are.
    let cur_cues: BTreeSet<String> = cur
        .msgs("cues")
        .iter()
        .map(|c| casefold(uuid_of(c)))
        .collect();
    let recorded: BTreeSet<String> = record.cue_uuids.iter().map(|u| casefold(u)).collect();
    let unchanged = sha == record.file_sha256
        || (!record.cue_uuids.is_empty()
            && recorded.is_subset(&cur_cues)
            && cur.len("cues") as i64 == record.file_cue_count);
    if !record.file_sha256.is_empty() && !unchanged {
        return Err(VvError::write(format!(
            "{name} was edited after VerseVision wrote it, so it was not changed. \
             The previous version is saved at {backup_path} if you want it back"
        )));
    }
    let restore = work.path().join(format!("{name}.pro"));
    transport.fetch_file(backup_path, &restore)?;
    let prior = read_presentation(&fs::read(&restore)?)?;
    let wanted = prior.len("cues");
    let verify = Verifier::SlideCount { wanted };
    swap_core(ctx, transport, lib, &name, &restore, &verify, &[])?;
    ctx.history.set_status(&record.import_id, "undone")?;
    let node = config
        .node
        .name
        .clone()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "local".into());
    let mut result = ApplyResult::blank(&record.import_id, &name, &node);
    result.mode = "swap".into();
    result.presentation_name = name.clone();
    result.cue_count_before = record.cue_uuids.len() as i64;
    result.cue_count_after = wanted as i64;
    result.removed_slides = record.cue_uuids.len() as i64;
    result.verification = format!("Put the previous version of {name} back ({wanted} slides)");
    result.verified = true;
    Ok(result)
}

/// `_undo_new`: undo a "new presentation" import: delete that file. Only a file VerseVision created and that is unchanged.
fn undo_new(
    ctx: &mut Ctx,
    record: &ImportRecord,
    config: &AppConfig,
    transport: &dyn Transport,
) -> VvResult<ApplyResult> {
    let Some(file_path) = record.file_path.as_deref().filter(|p| !p.is_empty()) else {
        return Err(VvError::configuration(
            "This import has no recorded file; remove the presentation in ProPresenter instead",
        ));
    };
    ctx.progress("Removing the presentation");
    if !record.playlist_items.is_empty() {
        remove_from_playlists(ctx, transport, record);
    }
    let expected = if record.file_cue_count != 0 {
        record.file_cue_count as usize
    } else {
        record.cue_uuids.len()
    };
    let removed = transport.delete_created_file(file_path, expected)?;
    ctx.history.set_status(&record.import_id, "undone")?;
    let node = config
        .node
        .name
        .clone()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "local".into());
    let mut result = ApplyResult::blank(&record.import_id, &record.target_name, &node);
    result.mode = "new".into();
    result.presentation_name = record.target_name.clone();
    result.cue_count_before = record.cue_uuids.len() as i64;
    result.removed_slides = record.cue_uuids.len() as i64;
    result.verification = removed;
    result.verified = true;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalising_collapses_space_and_case() {
        assert_eq!(norm("  For God\n so  LOVED "), "for god so loved");
        assert_eq!(norm_words("For God, so-loved 3:16!"), "forgodsoloved316");
    }
}
