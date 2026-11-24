//! The jobs of the page: plan (read the document, check it, show the review), replan (fix lines, check again), apply (import what the
//! review left) and undo. They are `web/server.py`'s `start_plan`, `start_replan`, `start_apply` and `start_undo`: the same steps, the same
//! run folder, the same messages. Each runs on the runtime's blocking pool, because ProPresenter's transport and the file work block.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use serde_json::{json, Map, Value};
use tokio::runtime::Handle;
use versevision_core::config::AppConfig;
use versevision_core::notes::title::{document_title, safe_presentation_name};
use versevision_core::notes::trace::NoTrace;
use versevision_core::plan::ReviewChoices;
use versevision_core::workflow::apply_review_choices;
use versevision_pp::errors::{Kind, VvError, VvResult};
use versevision_pp::pipeline::{plan_for_transport, Ctx};
use versevision_pp::run::{apply_reviewed, report_value, run_undo, ReviewedImport};
use versevision_pp::runlog::RunLog;
use versevision_pp::themes::split_look_key;

use crate::state::{Inner, Job, JobKind, Reviewed, SharedJob, WebState};

/// A planning request as the page makes it (query string and body of `POST /api/plan`).
#[derive(Clone, Default)]
pub struct PlanRequest {
    pub name: String,
    pub data: Vec<u8>,
    pub version: Option<String>,
    pub target: Option<String>,
    pub mode: String,
    pub title: Option<String>,
    pub library: Option<String>,
    pub profile: Option<String>,
    pub verse_look: Option<String>,
    pub point_look: Option<String>,
    /// The text of a recheck: the document with the lines the person corrected.
    pub text_override: Option<String>,
    /// `(run id of the review it follows, the corrected lines)`
    pub replan_of: Option<(String, Vec<Value>)>,
}

/// `str(exc)` the way the page shows a failed job, and the exit code it carries.
fn job_error(error: &VvError) -> (String, i32) {
    match error.kind {
        Kind::Other(_) => (format!("Unexpected problem: {}", error.message), 1),
        _ => (error.message.clone(), error.exit_code() as i32),
    }
}

/// A job that panics must still end: the page polls it, and the one-import-at-a-time flag must be let go (Python's runner catches every
/// exception the same way).
fn guarded(work: impl FnOnce() -> VvResult<()>) -> VvResult<()> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)) {
        Ok(result) => result,
        Err(payload) => {
            let text = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "an internal error".to_string());
            Err(VvError::new(Kind::Other("Panic"), text))
        }
    }
}

fn register(inner: &Inner, job: Job) -> SharedJob {
    let shared = Arc::new(Mutex::new(job));
    let mut control = inner
        .control
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let id = shared
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .id
        .clone();
    control.jobs.insert(id, shared.clone());
    if control.jobs.len() > 40 {
        let oldest: Vec<String> = control.jobs.keys().take(10).cloned().collect();
        for key in oldest {
            control.jobs.shift_remove(&key);
        }
    }
    shared
}

fn finish(inner: &Inner, job: &SharedJob, outcome: Result<(), VvError>) {
    let kind = {
        let mut j = job
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match outcome {
            Ok(()) => j.state = "done",
            Err(error) => {
                let (message, code) = job_error(&error);
                j.state = "error";
                j.error = Some(message);
                j.exit_code = code;
            }
        }
        j.kind
    };
    if kind == JobKind::Apply {
        inner
            .control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .busy = false;
    }
}

fn set_message(job: &SharedJob, message: &str) {
    job.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .message = message.to_string();
}

fn new_ctx(inner: &Inner, job: &SharedJob) -> Ctx {
    let mut ctx = Ctx::new(
        inner.clock.clone(),
        inner.uuids.clone(),
        inner.dirs.clone(),
        &inner.app_version,
    );
    ctx.timing = inner.timing;
    let watching = job.clone();
    ctx.progress_sink = Some(Box::new(move |message| set_message(&watching, message)));
    ctx
}

/// `_config_with_looks`: a look is `Theme name::slide number` as `/api/themes` lists it; empty means the VerseVision template.
fn config_with_looks(
    config: &AppConfig,
    verse_look: Option<&str>,
    point_look: Option<&str>,
) -> VvResult<AppConfig> {
    let mut sets: Vec<String> = Vec::new();
    for (section, key) in [("verses", verse_look), ("points", point_look)] {
        if let Some(key) = key.filter(|k| !k.is_empty()) {
            let (theme, number) = split_look_key(key);
            sets.push(format!("{section}.theme={theme}"));
            sets.push(format!("{section}.theme_slide={number}"));
        }
    }
    if sets.is_empty() {
        return Ok(config.clone());
    }
    let refs: Vec<&str> = sets.iter().map(String::as_str).collect();
    config
        .with_options(None, &refs)
        .map_err(|e| VvError::configuration(e.to_string()))
}

/// The lines of a document as the review numbers them.
pub fn source_lines(text: &str) -> Vec<String> {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\u{a0}', " ")
        .split('\n')
        .map(str::to_string)
        .collect()
}

pub fn start_plan(state: &WebState, request: PlanRequest) -> SharedJob {
    let inner = state.inner.clone();
    let mut job = Job::new(JobKind::Plan);
    job.name = request.name.clone();
    job.mode = request.mode.clone();
    job.title_override = request.title.clone();
    job.library = request.library.clone();
    job.version = request.version.clone();
    job.target_arg = request.target.clone();
    job.profile = request.profile.clone();
    job.verse_look = request.verse_look.clone();
    job.point_look = request.point_look.clone();
    let shared = register(&inner, job);
    let handle = Handle::current();
    let working = shared.clone();
    tokio::task::spawn_blocking(move || {
        let outcome = guarded(|| plan_job(&inner, &handle, &working, &request));
        finish(&inner, &working, outcome);
    });
    shared
}

fn plan_job(
    inner: &Arc<Inner>,
    handle: &Handle,
    job: &SharedJob,
    request: &PlanRequest,
) -> VvResult<()> {
    let config = inner
        .config
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let mut ctx = new_ctx(inner, job);
    let default_version = request
        .version
        .clone()
        .or_else(|| config.bible.default_version.clone());
    let mut meta = Map::new();
    meta.insert("source".into(), json!(request.name));
    meta.insert("mode".into(), json!(request.mode));
    meta.insert("title".into(), json!(request.title));
    meta.insert("library".into(), json!(request.library));
    meta.insert("target".into(), json!(request.target));
    meta.insert(
        "node".into(),
        json!(config
            .node
            .name
            .clone()
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "this computer".to_string())),
    );
    meta.insert("default_version".into(), json!(default_version));
    let run = RunLog::start(
        &ctx.dirs,
        inner.clock.clone(),
        &inner.app_version,
        "web",
        meta,
    )?;
    job.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .run_id = run.id.clone();
    ctx.run = Some(run);
    if let Some((parent, changes)) = &request.replan_of {
        ctx.trace(
            "review_edit",
            json!({
                "parent_run": parent,
                "changes": changes,
                "note": "lines you corrected in the review panel; the whole document was checked again with these replacing the originals",
            }),
        );
    }
    let mut result = None;
    let outcome = match plan_inner(inner, handle, job, request, &config, &mut ctx) {
        Ok(report) => {
            result = Some(report);
            Ok(())
        }
        Err(error) => {
            if let Some(run) = ctx.run.as_mut() {
                run.failed(&error);
            }
            Err(error)
        }
    };
    let error_text = outcome.as_ref().err().map(|e| e.message.clone());
    let finished_outcome = if outcome.is_ok() {
        "reviewed, not imported yet"
    } else {
        "failed"
    };
    if let Some(run) = ctx.run.as_mut() {
        run.finish(
            finished_outcome,
            result.as_ref(),
            None,
            error_text.as_deref(),
        );
    }
    ctx.run = None;
    if let Some(report) = result {
        job.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .result = Some(report);
        set_message(job, "Ready to review");
    }
    outcome
}

fn plan_inner(
    inner: &Arc<Inner>,
    handle: &Handle,
    job: &SharedJob,
    request: &PlanRequest,
    config: &AppConfig,
    ctx: &mut Ctx,
) -> VvResult<Value> {
    let name = &request.name;
    let suffix = versevision_docs::suffix_of(name);
    let text = match &request.text_override {
        Some(text) => {
            ctx.event(
                "document_read",
                "info",
                json!({
                    "message": format!("{name} (edited in the review, {} characters)", text.chars().count()),
                    "file": name, "type": "edited",
                }),
            );
            text.clone()
        }
        None => {
            if !versevision_docs::ALLOWED_SUFFIXES.contains(&suffix.as_str()) {
                return Err(VvError::new(
                    Kind::Document,
                    versevision_docs::unsupported_upload_message(&suffix),
                ));
            }
            set_message(job, "Reading the document");
            let (text, info) =
                versevision_docs::extract_text(&request.data, name).map_err(|e| match e {
                    versevision_docs::DocumentError::Unexpected(message) => {
                        VvError::new(Kind::Other("Exception"), message)
                    }
                    other => VvError::new(Kind::Document, other.message()),
                })?;
            if let Some(Value::Object(fields)) = Some(info.to_json()) {
                ctx.trace("extract", Value::Object(fields));
            }
            ctx.event(
                "document_read",
                "info",
                json!({
                    "message": format!("{name} ({} bytes)", request.data.len()),
                    "file": name, "bytes": request.data.len(), "type": suffix,
                }),
            );
            text
        }
    };
    if let Some(run) = ctx.run.as_mut() {
        run.save_source(&text, name);
    }
    job.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .mode = request.mode.clone();
    if !["new", "replace", "append"].contains(&request.mode.as_str()) {
        return Err(VvError::new(Kind::Document, "Unknown mode"));
    }
    if config.presentation_template.is_none() {
        return Err(VvError::new(
            Kind::Document,
            "No template is set up yet. Run: versevision template setup",
        ));
    }
    set_message(job, "Finding verses and fetching Scripture");
    let transport = (inner.transport)(config);
    let adjusted = match request.profile.as_deref().filter(|p| !p.is_empty()) {
        Some(profile) => config
            .with_options(Some(profile), &[])
            .map_err(|e| VvError::configuration(e.to_string()))?,
        None => config.clone(),
    };
    let adjusted = config_with_looks(
        &adjusted,
        request.verse_look.as_deref(),
        request.point_look.as_deref(),
    )?;
    set_message(job, "Checking ProPresenter");
    let bible = (inner.bible)(&adjusted);
    let target_arg = request.target.as_deref().filter(|t| !t.is_empty());
    if request.mode != "new" && target_arg.is_none() {
        return Err(VvError::new(
            Kind::Document,
            "Choose which presentation to change",
        ));
    }
    let (plan, target) = handle.block_on(plan_for_transport(
        ctx,
        &text,
        &adjusted,
        transport.as_ref(),
        request.version.as_deref(),
        if request.mode != "new" {
            target_arg
        } else {
            None
        },
        None,
        None,
        false,
        Some(&bible),
    ))?;
    let title = match request.title.as_deref().filter(|t| !t.is_empty()) {
        Some(title) => safe_presentation_name(title, "Sermon Notes"),
        None => document_title(&text, name)
            .map_err(|e| VvError::new(Kind::Other("ValueError"), e.to_string()))?,
    };
    let mut report = report_value(&plan, name);
    let lines = source_lines(&text);
    let mut shown = Map::new();
    if let Some(decisions) = report["decisions"].as_array() {
        for d in decisions {
            if let Some(line) = d["line"].as_u64() {
                if line >= 1 && (line as usize) <= lines.len() {
                    shown.insert(line.to_string(), json!(lines[line as usize - 1]));
                }
            }
        }
    }
    report["lines"] = Value::Object(shown);
    report["mode"] = json!(request.mode);
    report["title"] = json!(title);
    report["library"] = json!(request
        .library
        .clone()
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| config.output.library.clone()));
    report["target"] = match &target {
        Some(t) => json!({"name": t.name, "library": t.library, "uuid": t.uuid}),
        None => Value::Null,
    };
    report["node"] = json!(transport.description());
    report["replaces"] = Value::Null;
    if let Some(run) = ctx.run.as_ref() {
        report["run_id"] = json!(run.id);
        run.save_report(&report);
    }
    let mut j = job
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    j.title = title;
    j.reviewed = Some(Reviewed {
        text,
        plan: Arc::new(plan),
        target: target.map(Arc::new),
    });
    Ok(report)
}

/// `start_replan`: fix lines in the review (type a corrected line) and check the whole document again. The file itself is never changed.
pub fn start_replan(
    state: &WebState,
    plan_job_id: &str,
    edits: &Map<String, Value>,
) -> Result<SharedJob, String> {
    let source = state
        .inner
        .control
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .jobs
        .get(plan_job_id)
        .cloned();
    let gone = || "That review is no longer available; drop the file again".to_string();
    let source = source.ok_or_else(gone)?;
    let (request, text) = {
        let s = source
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(reviewed) = s.reviewed.as_ref().filter(|_| s.kind == JobKind::Plan) else {
            return Err(gone());
        };
        if reviewed.text.is_empty() {
            return Err(gone());
        }
        (
            PlanRequest {
                name: s.name.clone(),
                data: Vec::new(),
                version: s.version.clone(),
                target: s.target_arg.clone(),
                mode: s.mode.clone(),
                title: s.title_override.clone(),
                library: s.library.clone(),
                profile: s.profile.clone(),
                verse_look: s.verse_look.clone(),
                point_look: s.point_look.clone(),
                text_override: None,
                replan_of: Some((s.run_id.clone(), Vec::new())),
            },
            reviewed.text.clone(),
        )
    };
    let mut lines = source_lines(&text);
    let mut changes = Vec::new();
    for (key, value) in edits {
        let number: i64 = crate::configpage::py_int_text(key)?;
        if number < 1 || number as usize > lines.len() {
            return Err(format!("There is no line {number}"));
        }
        let after = crate::configpage::py_str(value).replace('\n', " ");
        let at = number as usize - 1;
        changes.push(json!({"line": number, "before": lines[at], "after": after}));
        lines[at] = after;
    }
    let mut request = request;
    request.text_override = Some(lines.join("\n"));
    if let Some((_, slot)) = request.replan_of.as_mut() {
        *slot = changes;
    }
    Ok(start_plan(state, request))
}

fn take_busy(inner: &Inner) -> Result<(), String> {
    let mut control = inner
        .control
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if control.busy {
        return Err("Another import is already running".into());
    }
    control.busy = true;
    Ok(())
}

/// What a review keeps for the import that follows it.
struct Snapshot {
    text: String,
    name: String,
    mode: String,
    title: String,
    library: Option<String>,
    plan: Arc<versevision_pp::pipeline::Planned>,
    target: Option<Arc<versevision_pp::pipeline::Target>>,
    report: Value,
    run_id: String,
}

fn apply_job(
    inner: &Arc<Inner>,
    job: &SharedJob,
    s: &Snapshot,
    accept_flags: bool,
    choices: &ReviewChoices,
) -> VvResult<()> {
    let config = inner
        .config
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let mut ctx = new_ctx(inner, job);
    let transport = (inner.transport)(&config);
    let import = ReviewedImport {
        text: &s.text,
        source_name: &s.name,
        mode: &s.mode,
        title: &s.title,
        library: s.library.as_deref(),
        plan: &s.plan,
        target: s.target.as_deref(),
        report: &s.report,
        run_id: Some(s.run_id.as_str()).filter(|id| !id.is_empty()),
    };
    let outcome = apply_reviewed(
        &mut ctx,
        &import,
        accept_flags,
        choices,
        &config,
        transport.as_ref(),
    );
    if let Some(error) = outcome.error {
        return Err(error);
    }
    if let Some(applied) = outcome.applied {
        let mut j = job
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        j.result = Some(applied.to_value());
        j.message = "Done".into();
    }
    Ok(())
}

/// `start_apply`: import what the review left.
pub fn start_apply(
    state: &WebState,
    plan_job_id: &str,
    accept_flags: bool,
    choices: ReviewChoices,
) -> Result<SharedJob, String> {
    let inner = state.inner.clone();
    let gone = || "That review is no longer available; drop the file again".to_string();
    let source = inner
        .control
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .jobs
        .get(plan_job_id)
        .cloned()
        .ok_or_else(gone)?;
    let snapshot = {
        let s = source
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let reviewed = s
            .reviewed
            .as_ref()
            .filter(|_| s.kind == JobKind::Plan)
            .ok_or_else(gone)?;
        if s.mode != "new" && reviewed.target.is_none() {
            return Err(gone());
        }
        Snapshot {
            text: reviewed.text.clone(),
            name: s.name.clone(),
            mode: s.mode.clone(),
            title: s.title.clone(),
            library: s.library.clone(),
            plan: reviewed.plan.clone(),
            target: reviewed.target.clone(),
            report: s.result.clone().unwrap_or(Value::Null),
            run_id: s.run_id.clone(),
        }
    };
    if snapshot.report["flagged"]
        .as_array()
        .is_some_and(|f| !f.is_empty())
        && !accept_flags
    {
        return Err("Some lines are flagged; confirm you want to import the rest".into());
    }
    let remaining = apply_review_choices(&snapshot.plan.result, &choices, &mut NoTrace);
    if remaining.slides.is_empty() {
        return Err("Every slide was switched off; nothing to import".into());
    }
    take_busy(&inner)?;
    let shared = register(&inner, Job::new(JobKind::Apply));
    let working = shared.clone();
    tokio::task::spawn_blocking(move || {
        let outcome = guarded(|| apply_job(&inner, &working, &snapshot, accept_flags, &choices));
        finish(&inner, &working, outcome);
    });
    Ok(shared)
}

/// `start_undo`.
pub fn start_undo(state: &WebState, import_id: &str) -> Result<SharedJob, String> {
    let inner = state.inner.clone();
    take_busy(&inner)?;
    let shared = register(&inner, Job::new(JobKind::Apply));
    let working = shared.clone();
    let import_id = import_id.to_string();
    tokio::task::spawn_blocking(move || {
        let result = guarded(|| {
            let config = inner
                .config
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            let mut ctx = new_ctx(&inner, &working);
            let transport = (inner.transport)(&config);
            let outcome = run_undo(&mut ctx, &import_id, &config, transport.as_ref(), false);
            match (outcome.error, outcome.applied) {
                (Some(error), _) => Err(error),
                (None, applied) => {
                    if let Some(applied) = applied {
                        let mut j = working
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        j.result = Some(applied.to_value());
                        j.message = "Done".into();
                    }
                    Ok(())
                }
            }
        });
        finish(&inner, &working, result);
    });
    Ok(shared)
}

/// Decision ids from the page (`{int(x) for x in body.get(key, [])}`).
pub fn id_set(body: &Value, key: &str) -> Result<BTreeSet<usize>, String> {
    let mut out = BTreeSet::new();
    let Some(items) = body.get(key) else {
        return Ok(out);
    };
    let Some(items) = items.as_array() else {
        return Err(format!(
            "Bad request: '{}' object is not iterable",
            crate::configpage::py_type(items)
        ));
    };
    for item in items {
        let id = crate::configpage::py_int(item)?;
        if id < 0 {
            return Err("Bad request: ids are not negative".into());
        }
        out.insert(id as usize);
    }
    Ok(out)
}
