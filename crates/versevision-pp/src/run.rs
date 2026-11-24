//! The command line's `run` and `undo` (`cli_run.py`) and the web page's apply (`web/server.py: start_apply`) as functions: everything between
//! "the notes were read" and "the answer for the person", without the terminal. Printing (tables, colours, JSON on stdout) is the command
//! line's job (S7); what is here returns the numbers, the report and the exit code, and writes the run folder and the history.
//!
//! LOCAL transport only: choosing the remote-node transport (`--node`) is not ported (see [`crate::transport`]).

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};
use versevision_core::bibles::BiblePassages;
use versevision_core::config::AppConfig;
use versevision_core::notes::title::document_title;
use versevision_core::notes::trace::emit;
use versevision_core::plan::ReviewChoices;
use versevision_core::workflow::{apply_review_choices, build_report};

use crate::errors::{ExitCode, VvError, VvResult};
use crate::history::ImportRecord;
use crate::pipeline::{
    apply_import, plan_for_transport, publish_new, publish_swap, undo_import, ApplyResult, Ctx,
    Planned, Target,
};
use crate::runlog::RunLog;
use crate::transport::Transport;

/// The options of `versevision run`.
#[derive(Debug, Clone, Default)]
pub struct RunRequest {
    /// The text of the document (already read by the document reader) and the name it is known by.
    pub source: String,
    pub source_name: String,
    /// The `extract` trace record of the reader, when there was one (`versevision_docs::ExtractInfo::to_json`).
    pub extract: Option<Map<String, Value>>,
    /// `-p NAME` / `--presentation-uuid ID`: change an existing presentation (needs `replace` or `append`).
    pub presentation: Option<String>,
    pub presentation_uuid: Option<String>,
    pub title: Option<String>,
    pub library: Option<String>,
    pub replace: bool,
    pub append: bool,
    pub default_version: Option<String>,
    pub dry_run: bool,
    pub yes: bool,
    pub no_playlist: bool,
    pub as_copy: bool,
    pub in_place: bool,
    pub no_close: bool,
    pub no_reopen: bool,
    pub profile: Option<String>,
    /// `--add-line N`: left-out lines to add as slides.
    pub add_line: Vec<usize>,
    /// `--set section.option=value`.
    pub set_options: Vec<String>,
}

/// What a run came to.
#[derive(Debug)]
pub struct RunOutcome {
    /// The command's exit code.
    pub exit_code: u8,
    /// The report as printed with `--output json` (the plan's report plus `ok`, `exit_code`, `applied`, `run_id`...).
    pub report: Option<Value>,
    pub applied: Option<ApplyResult>,
    pub error: Option<VvError>,
    pub run_id: Option<String>,
    /// Lines the command line shows in yellow (`--add-line 7: that line is not one the importer left out...`).
    pub warnings: Vec<String>,
}

fn truthy_str(v: &Option<String>) -> Option<&str> {
    v.as_deref().filter(|s| !s.is_empty())
}

/// The report of `build_report` as a dictionary in Python's key order (`source, summary, looks, decisions, review, slides, ...`).
pub fn report_value(plan: &Planned, source_name: &str) -> Value {
    let report = build_report(&plan.result, source_name);
    let Ok(Value::Object(mut map)) = serde_json::to_value(&report) else {
        return Value::Null;
    };
    let mut out = Map::new();
    for key in [
        "source",
        "summary",
        "looks",
        "decisions",
        "review",
        "slides",
        "flagged",
        "adjustments",
        "left_out",
        "clean",
    ] {
        if let Some(v) = map.remove(key) {
            out.insert(key.to_string(), v);
        }
    }
    out.extend(map);
    Value::Object(out)
}

fn start_log(ctx: &mut Ctx, kind: &str, meta: Map<String, Value>) -> VvResult<()> {
    let run = RunLog::start(&ctx.dirs, ctx.clock.clone(), &ctx.app_version, kind, meta)?;
    ctx.run = Some(run);
    ctx.log_progress = true;
    Ok(())
}

/// `versevision run`: plan, check, import. Writes a run folder and (unless it is a dry run) the history.
pub async fn run_import<B: BiblePassages + ?Sized>(
    ctx: &mut Ctx,
    request: &RunRequest,
    config: &AppConfig,
    transport: &dyn Transport,
    bible: Option<&B>,
) -> RunOutcome {
    let mut outcome = RunOutcome {
        exit_code: 1,
        report: None,
        applied: None,
        error: None,
        run_id: None,
        warnings: Vec::new(),
    };
    // `with_options` happens before the run folder exists, as in Python
    let sets: Vec<&str> = request.set_options.iter().map(String::as_str).collect();
    let config = match config.with_options(truthy_str(&request.profile), &sets) {
        Ok(c) => c,
        Err(e) => {
            let error = VvError::configuration(e.to_string());
            outcome.exit_code = error.exit_code() as u8;
            outcome.error = Some(error);
            return outcome;
        }
    };
    let mode_name = if request.replace {
        "replace"
    } else if request.append {
        "append"
    } else {
        "new"
    };
    let target_arg = truthy_str(&request.presentation).or(truthy_str(&request.presentation_uuid));
    let flags: Vec<&str> = [
        ("--yes", request.yes),
        ("--no-playlist", request.no_playlist),
        ("--in-place", request.in_place),
    ]
    .iter()
    .filter(|(_, on)| *on)
    .map(|(f, _)| *f)
    .collect();
    let default_version = truthy_str(&request.default_version)
        .map(str::to_string)
        .or_else(|| {
            config
                .bible
                .default_version
                .clone()
                .filter(|d| !d.is_empty())
        });
    let mut meta = Map::new();
    meta.insert("source".into(), json!(request.source_name));
    meta.insert(
        "mode".into(),
        json!(format!(
            "{mode_name}{}",
            if request.dry_run { " (dry run)" } else { "" }
        )),
    );
    meta.insert("title".into(), json!(request.title));
    meta.insert("library".into(), json!(request.library));
    meta.insert("target".into(), json!(target_arg));
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
    meta.insert(
        "flags".into(),
        if flags.is_empty() {
            Value::Null
        } else {
            json!(flags.join(", "))
        },
    );
    if let Err(e) = start_log(ctx, "run", meta) {
        let error = e;
        outcome.exit_code = error.exit_code() as u8;
        outcome.error = Some(error);
        return outcome;
    }
    outcome.run_id = ctx.run.as_ref().map(|r| r.id.clone());

    let mut st_outcome = "failed".to_string();
    let mut st_error: Option<String> = None;
    let mut st_report: Option<Value> = None;
    let mut st_applied: Option<Value> = None;
    let result = run_inner(
        ctx,
        request,
        &config,
        transport,
        bible,
        &mut st_outcome,
        &mut st_report,
        &mut st_applied,
        &mut outcome,
    )
    .await;
    match result {
        Ok(()) => {}
        Err(exc) => {
            st_error = Some(exc.message.clone());
            if let Some(run) = ctx.run.as_mut() {
                run.failed(&exc);
            }
            outcome.exit_code = exc.exit_code() as u8;
            outcome.error = Some(exc);
        }
    }
    if let Some(run) = ctx.run.as_mut() {
        run.finish(
            &st_outcome,
            st_report.as_ref(),
            st_applied.as_ref(),
            st_error.as_deref(),
        );
    }
    ctx.run = None;
    ctx.log_progress = false;
    outcome.report = st_report;
    outcome
}

#[allow(clippy::too_many_arguments)]
async fn run_inner<B: BiblePassages + ?Sized>(
    ctx: &mut Ctx,
    request: &RunRequest,
    config: &AppConfig,
    transport: &dyn Transport,
    bible: Option<&B>,
    st_outcome: &mut String,
    st_report: &mut Option<Value>,
    st_applied: &mut Option<Value>,
    outcome: &mut RunOutcome,
) -> VvResult<()> {
    if let Some(run) = ctx.run.as_mut() {
        run.save_source(&request.source, &request.source_name);
        if let Some(extract) = &request.extract {
            emit(run, "extract", extract.clone());
        }
    }
    let chosen = truthy_str(&request.presentation);
    let chosen_uuid = truthy_str(&request.presentation_uuid);
    let editing = chosen.is_some() || chosen_uuid.is_some();
    if request.replace && request.append {
        return Err(VvError::configuration(
            "choose either --replace or --append, not both",
        ));
    }
    if (request.replace || request.append) && !editing {
        return Err(VvError::configuration(
            "--replace and --append change an existing presentation: name it with -p NAME (see: versevision presentations)",
        ));
    }
    if editing && !(request.replace || request.append) {
        return Err(VvError::configuration(
            "-p changes an existing presentation: add --replace (swap its verse and point slides) or --append. Leave -p out to make a new presentation",
        ));
    }
    let (mut plan, target) = plan_for_transport(
        ctx,
        &request.source,
        config,
        transport,
        truthy_str(&request.default_version),
        chosen_uuid,
        chosen,
        if editing {
            truthy_str(&request.library)
        } else {
            None
        },
        false,
        bible,
    )
    .await?;
    let new_title = match truthy_str(&request.title) {
        Some(t) => t.to_string(),
        None => document_title(&request.source, &request.source_name)
            .map_err(|e| VvError::new(crate::errors::Kind::Other("ValueError"), e.to_string()))?,
    };
    if !request.add_line.is_empty() {
        let wanted: BTreeSet<usize> = request.add_line.iter().copied().collect();
        let chosen_ids: BTreeSet<usize> = plan
            .result
            .decisions
            .iter()
            .filter(|d| wanted.contains(&d.line) && !d.add_text.is_empty() && d.slide_count == 0)
            .map(|d| d.id)
            .collect();
        let taken: BTreeSet<usize> = plan
            .result
            .decisions
            .iter()
            .filter(|d| chosen_ids.contains(&d.id))
            .map(|d| d.line)
            .collect();
        for number in wanted.difference(&taken) {
            outcome.warnings.push(format!(
                "--add-line {number}: that line is not one the importer left out, so there is nothing to add"
            ));
        }
        let choices = ReviewChoices {
            skip: BTreeSet::new(),
            include: chosen_ids,
            reject_fixes: BTreeSet::new(),
        };
        let new_plan = match ctx.run.as_mut() {
            Some(run) => apply_review_choices(&plan.result, &choices, run),
            None => apply_review_choices(
                &plan.result,
                &choices,
                &mut versevision_core::notes::trace::NoTrace,
            ),
        };
        plan.result.slide_plan = new_plan;
    }
    let mut report = report_value(&plan, &request.source_name);
    let mode_name = if request.replace {
        "replace"
    } else if request.append {
        "append"
    } else {
        "new"
    };
    report["mode"] = json!(mode_name);
    report["title"] = json!(new_title);
    report["target"] = match &target {
        Some(t) => json!({"name": t.name, "uuid": t.uuid, "library": t.library}),
        None => Value::Null,
    };
    report["library"] = json!(truthy_str(&request.library).unwrap_or(&config.output.library));
    report["node"] = json!(transport.description());
    let flags = report["flagged"].as_array().is_some_and(|f| !f.is_empty());
    if let Some(run) = ctx.run.as_ref() {
        run.save_report(&report);
    }
    *st_report = Some(report.clone());
    let finish = |report: &mut Value, fields: Value| {
        if let (Value::Object(r), Value::Object(f)) = (report, fields) {
            r.extend(f);
        }
    };
    if request.dry_run {
        *st_outcome = "dry run".into();
        let code = if flags {
            ExitCode::CompletedWithFlags
        } else {
            ExitCode::Success
        } as u8;
        finish(
            &mut report,
            json!({"ok": true, "dry_run": true, "exit_code": code}),
        );
        *st_report = Some(report);
        outcome.exit_code = code;
        return Ok(());
    }
    if flags && !request.yes {
        *st_outcome = "stopped: flagged lines (rerun with --yes to import the rest)".into();
        let code = ExitCode::ParsingOrValidation as u8;
        finish(
            &mut report,
            json!({"ok": false, "error": "Some lines are flagged. Fix them, or rerun with --yes to import the rest.", "exit_code": code}),
        );
        *st_report = Some(report);
        outcome.exit_code = code;
        return Ok(());
    }
    let applied: ApplyResult = if editing && request.in_place {
        let target = target
            .as_ref()
            .ok_or_else(|| VvError::general("no target"))?;
        apply_import(
            ctx,
            &request.source,
            &request.source_name,
            &plan,
            target,
            config,
            transport,
            true,
            request.replace,
            !request.no_close,
            !request.no_reopen,
        )?
    } else if editing && request.as_copy {
        publish_new(
            ctx,
            &request.source,
            &request.source_name,
            &new_title,
            &plan,
            config,
            transport,
            None,
            25.0,
            target.as_ref(),
            request.replace,
            false,
            !request.no_playlist,
        )?
    } else if editing {
        let target = target
            .as_ref()
            .ok_or_else(|| VvError::general("no target"))?;
        publish_swap(
            ctx,
            &request.source,
            &request.source_name,
            &plan,
            config,
            transport,
            target,
            request.replace,
        )?
    } else {
        publish_new(
            ctx,
            &request.source,
            &request.source_name,
            &new_title,
            &plan,
            config,
            transport,
            truthy_str(&request.library),
            25.0,
            None,
            false,
            false,
            true,
        )?
    };
    let code = if flags {
        ExitCode::CompletedWithFlags
    } else {
        ExitCode::Success
    } as u8;
    finish(
        &mut report,
        json!({"ok": true, "applied": applied.to_value(), "exit_code": code}),
    );
    *st_outcome = "ok".into();
    *st_applied = Some(applied.to_value());
    if let Some(run) = ctx.run.as_ref() {
        report["run_id"] = json!(run.id);
    }
    *st_report = Some(report);
    outcome.exit_code = code;
    outcome.applied = Some(applied);
    Ok(())
}

/// `versevision undo IMPORT_ID`.
pub fn run_undo(
    ctx: &mut Ctx,
    import_id: &str,
    config: &AppConfig,
    transport: &dyn Transport,
    no_close: bool,
) -> RunOutcome {
    let mut outcome = RunOutcome {
        exit_code: 1,
        report: None,
        applied: None,
        error: None,
        run_id: None,
        warnings: Vec::new(),
    };
    let mut meta = Map::new();
    meta.insert(
        "source".into(),
        json!(format!("undo of import {import_id}")),
    );
    meta.insert("mode".into(), json!("undo"));
    if let Err(e) = start_log(ctx, "undo", meta) {
        let error = e;
        outcome.exit_code = error.exit_code() as u8;
        outcome.error = Some(error);
        return outcome;
    }
    outcome.run_id = ctx.run.as_ref().map(|r| r.id.clone());
    let (mut st_outcome, mut st_error, mut st_applied) =
        ("failed".to_string(), None::<String>, None::<Value>);
    match undo_import(ctx, import_id, config, transport, !no_close, true) {
        Ok(result) => {
            st_outcome = "ok".into();
            st_applied = Some(result.to_value());
            outcome.exit_code = 0;
            outcome.applied = Some(result);
        }
        Err(exc) => {
            st_error = Some(exc.message.clone());
            if let Some(run) = ctx.run.as_mut() {
                run.failed(&exc);
            }
            outcome.exit_code = exc.exit_code() as u8;
            outcome.error = Some(exc);
        }
    }
    if let Some(run) = ctx.run.as_mut() {
        run.finish(&st_outcome, None, st_applied.as_ref(), st_error.as_deref());
    }
    ctx.run = None;
    ctx.log_progress = false;
    outcome
}

/// `versevision history`: every earlier import, oldest first.
pub fn history_list(ctx: &Ctx) -> Vec<ImportRecord> {
    ctx.history.load()
}

/// A reviewed plan waiting to be imported (`start_apply` of the web page).
pub struct ReviewedImport<'a> {
    pub text: &'a str,
    pub source_name: &'a str,
    /// `new`, `replace` or `append`.
    pub mode: &'a str,
    pub title: &'a str,
    pub library: Option<&'a str>,
    pub plan: &'a Planned,
    pub target: Option<&'a Target>,
    /// The report the review showed (its `decisions` are read for the `review_excluded` record).
    pub report: &'a Value,
    /// The run that planned it, when it can be carried on (`RunLog::reopen`).
    pub run_id: Option<&'a str>,
}

/// `start_apply` after the review: apply the choices (switched-off decisions, left-out lines to add, automatic fixes not approved),
/// import, and write the run's record.
pub fn apply_reviewed(
    ctx: &mut Ctx,
    review: &ReviewedImport,
    accept_flags: bool,
    choices: &ReviewChoices,
    config: &AppConfig,
    transport: &dyn Transport,
) -> RunOutcome {
    let mut outcome = RunOutcome {
        exit_code: 1,
        report: None,
        applied: None,
        error: None,
        run_id: None,
        warnings: Vec::new(),
    };
    let flagged = review.report["flagged"]
        .as_array()
        .is_some_and(|f| !f.is_empty());
    if flagged && !accept_flags {
        outcome.error = Some(VvError::general(
            "Some lines are flagged; confirm you want to import the rest",
        ));
        return outcome;
    }
    // the plan is first made without a run, so nothing is traced
    let mut sink = versevision_core::notes::trace::NoTrace;
    let slide_plan = apply_review_choices(&review.plan.result, choices, &mut sink);
    if slide_plan.slides.is_empty() {
        outcome.error = Some(VvError::general(
            "Every slide was switched off; nothing to import",
        ));
        return outcome;
    }
    let mut final_plan = review.plan.clone();
    final_plan.result.slide_plan = slide_plan;
    let reopened = review
        .run_id
        .and_then(|id| RunLog::reopen(&ctx.dirs, ctx.clock.clone(), id));
    ctx.run = match reopened {
        Some(run) => Some(run),
        None => {
            let mut meta = Map::new();
            meta.insert("source".into(), json!(review.source_name));
            meta.insert("mode".into(), json!(review.mode));
            match RunLog::start(&ctx.dirs, ctx.clock.clone(), &ctx.app_version, "web", meta) {
                Ok(run) => Some(run),
                Err(e) => {
                    outcome.error = Some(VvError::from(e));
                    return outcome;
                }
            }
        }
    };
    ctx.log_progress = true;
    outcome.run_id = ctx.run.as_ref().map(|r| r.id.clone());
    ctx.event(
        "apply_start",
        "info",
        json!({"message": format!("{}: importing after review", review.mode), "accept_flags": accept_flags}),
    );
    let (mut st_outcome, mut st_error, mut st_applied) =
        ("failed".to_string(), None::<String>, None::<Value>);
    let result = (|| -> VvResult<ApplyResult> {
        if !choices.skip.is_empty() {
            ctx.event("progress", "info", json!({"message": format!("{} decision(s) switched off in the review", choices.skip.len())}));
            let rows: Vec<Value> = review.report["decisions"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let decisions: Vec<Value> = choices
                .skip
                .iter()
                .map(|i| {
                    let row = rows.iter().find(|d| d["id"].as_u64() == Some(*i as u64));
                    let pick = |k: &str| row.map(|r| r[k].clone()).unwrap_or(Value::Null);
                    json!({"id": i, "line": pick("line"), "kind": pick("kind"), "title": pick("title"), "slides": pick("slides")})
                })
                .collect();
            ctx.trace(
                "review_excluded",
                json!({"decisions": decisions, "note": "decisions you switched off in the review: their slides were not imported"}),
            );
        }
        // the same result as above; this call records the added lines in the run's trace
        if let Some(run) = ctx.run.as_mut() {
            apply_review_choices(&review.plan.result, choices, run);
        }
        ctx.trace(
            "review_accepted",
            json!({"flagged_lines_accepted": accept_flags, "slides_imported": final_plan.result.slide_plan.slides.len()}),
        );
        if review.mode == "new" {
            publish_new(
                ctx,
                review.text,
                review.source_name,
                review.title,
                &final_plan,
                config,
                transport,
                review.library,
                25.0,
                None,
                false,
                false,
                true,
            )
        } else {
            let target = review.target.ok_or_else(|| {
                VvError::general("That review is no longer available; drop the file again")
            })?;
            publish_swap(
                ctx,
                review.text,
                review.source_name,
                &final_plan,
                config,
                transport,
                target,
                review.mode == "replace",
            )
        }
    })();
    match result {
        Ok(applied) => {
            st_outcome = "ok".into();
            st_applied = Some(applied.to_value());
            outcome.exit_code = 0;
            outcome.applied = Some(applied);
        }
        Err(exc) => {
            st_error = Some(exc.message.clone());
            if let Some(run) = ctx.run.as_mut() {
                run.failed(&exc);
            }
            outcome.exit_code = exc.exit_code() as u8;
            outcome.error = Some(exc);
        }
    }
    if let Some(run) = ctx.run.as_mut() {
        run.finish(
            &st_outcome,
            Some(review.report),
            st_applied.as_ref(),
            st_error.as_deref(),
        );
    }
    ctx.run = None;
    ctx.log_progress = false;
    outcome
}
