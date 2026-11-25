//! `versevision import` (the older, reference-only command of `cli.py`). `run` replaces it for sermon notes; it stays for the same options.
//! NOT ported: writing a plan made with `--notes` (use `versevision run`), which needs the older planner's target resolution.

use std::sync::Arc;

use serde_json::{json, Value};
use versevision_bible::{FileKeyStore, RegistryBuilder, ScriptureCache};
use versevision_core::config::AppConfig;
use versevision_core::plan::PlanOptions;
use versevision_pp::errors::{ExitCode, VvError};
use versevision_pp::import_workflow::dry_run_import;

use crate::app::{echo, err, out, Rt, Usage};
use crate::cmds::{clock, dumps_utf8, fail, is_json, load_config, read_source};
use crate::def::ImportArgs;
use crate::legacy::{aborted, confirm, prompt_enter};
use crate::rich::{markup, Overflow, Style, Table, R};

fn usage_error(message: &str) -> Usage {
    Usage {
        path: "versevision import".into(),
        message: format!("Invalid value: {message}"),
    }
}

fn cell(text: &str) -> R {
    R::Text {
        cells: markup(text, Style::NONE),
        overflow: Overflow::Ellipsis,
        no_wrap: false,
    }
}

pub fn import(rt: &Rt, a: ImportArgs) -> Result<i32, Usage> {
    if !a.dry_run && a.no_fetch {
        return Err(usage_error("production imports cannot use --no-fetch"));
    }
    if !a.dry_run
        && a.target_uuid.as_deref().is_none_or(str::is_empty)
        && a.target_presentation.as_deref().is_none_or(str::is_empty)
    {
        return Err(usage_error(
            "production imports require --target-uuid or --target-presentation",
        ));
    }
    let mut config: AppConfig = match load_config(rt) {
        Ok(c) => c,
        Err(e) => return Ok(fail(rt, &e)),
    };
    if let Some(h) = a.host.as_deref().filter(|h| !h.is_empty()) {
        config.propresenter.host = h.to_string();
    }
    if let Some(p) = a.port.filter(|p| *p != 0) {
        config.propresenter.port = p as u16;
    }
    if a.file.is_none() == a.text.is_none() {
        return Err(usage_error("provide exactly one FILE or --text"));
    }
    let (source, _, _) = match read_source(&a.file, &a.text) {
        Ok(t) => t,
        Err(e) => return Ok(fail(rt, &e)),
    };
    let keys = Arc::new(FileKeyStore::new(rt.paths.keys_file.clone()));
    let cache = Arc::new(ScriptureCache::new(rt.paths.cache_dir.join("scripture")));
    let sources = RegistryBuilder::new(keys)
        .cache(cache)
        .build_sources(&config.bible);
    let console = rt.term.out();
    let as_json = is_json(&a.output);

    if a.notes {
        let options = PlanOptions {
            default_version: a.default_version.clone(),
            no_fetch: a.no_fetch,
        };
        let mut trace = versevision_core::notes::trace::NoTrace;
        let result = match crate::block::block_on(versevision_core::workflow::plan_notes(
            &source,
            &config,
            &options,
            if a.no_fetch { None } else { Some(&sources) },
            &mut trace,
        )) {
            Ok(r) => r,
            Err(e) => return Ok(fail(rt, &VvError::general(e.to_string()))),
        };
        if as_json {
            let whole = serde_json::to_value(&result).unwrap_or(Value::Null);
            let mut value = serde_json::Map::new();
            for key in [
                "slide_plan",
                "verse_count",
                "point_count",
                "skipped",
                "review",
                "failed",
                "notices",
                "decisions",
            ] {
                value.insert(key.into(), whole[key].clone());
            }
            value.insert("target".into(), Value::Null);
            value.insert("fetched".into(), whole["fetched"].clone());
            value.insert("files_changed".into(), json!(false));
            echo(&dumps_utf8(&Value::Object(value)));
        } else {
            let report = notes_report(
                &console,
                &serde_json::to_value(&result).unwrap_or(Value::Null),
            );
            out(&report);
        }
        if a.dry_run {
            out(&console.print("Dry run: no files or playlists changed."));
            return Ok(0);
        }
        return Ok(fail(
            rt,
            &VvError::general(
                "Writing a plan made with --notes is not ported: use `versevision run`",
            ),
        ));
    }

    let target_given = a.target_uuid.as_deref().is_some_and(|t| !t.is_empty())
        || a.target_presentation
            .as_deref()
            .is_some_and(|t| !t.is_empty());
    let result = match crate::block::block_on(dry_run_import(
        &source,
        &config,
        a.default_version.as_deref(),
        a.non_interactive,
        a.target_uuid.as_deref(),
        a.target_presentation.as_deref(),
        a.library.as_deref(),
        target_given,
        a.no_fetch,
        Some(&sources),
    )) {
        Ok(r) => r,
        Err(e) => return Ok(fail(rt, &e)),
    };
    if a.dry_run && as_json {
        echo(&dumps_utf8(
            &serde_json::to_value(&result).unwrap_or(Value::Null),
        ));
        return Ok(0);
    }
    if !as_json {
        let mut table = Table::new(&["#", "Reference", "Version", "Status"]);
        for (index, reference) in result.references.iter().enumerate() {
            let status = if reference
                .warnings
                .iter()
                .any(|w| w == "default version applied")
            {
                "Default applied"
            } else {
                "Ready"
            };
            table.add_row(vec![
                cell(&(index + 1).to_string()),
                cell(&reference.normalized_text),
                cell(
                    reference
                        .version
                        .as_deref()
                        .filter(|v| !v.is_empty())
                        .unwrap_or("—"),
                ),
                cell(status),
            ]);
        }
        let mut text = console.print_r(&R::Table(Box::new(table)));
        if let Some(target) = &result.target {
            text.push_str(&console.print(&format!(
                "Destination: {} / {}",
                target.library_name, target.presentation_name
            )));
        }
        text.push_str(&console.print(&format!(
            "Slides planned: {}",
            result.slide_plan.as_ref().map_or(0, |p| p.slides.len())
        )));
        out(&text);
    }
    if a.dry_run {
        out(&console.print("Dry run: no files or playlists changed."));
        return Ok(0);
    }
    Ok(production(rt, &a, &config, &result))
}

fn production(
    rt: &Rt,
    a: &ImportArgs,
    config: &AppConfig,
    result: &versevision_pp::import_workflow::DryRunResult,
) -> i32 {
    let (Some(target), Some(plan)) = (&result.target, &result.slide_plan) else {
        return fail(
            rt,
            &VvError::configuration("Production import target or slide plan is missing"),
        );
    };
    let Some(definition) = config.presentation_template.clone() else {
        return fail(
            rt,
            &VvError::configuration("No template is configured; run versevision template select"),
        );
    };
    // the template presentation, through the typed API
    let api = versevision_pp::api_client::ProPresenterApiClient::new(
        &config.propresenter.host,
        config.propresenter.port,
        config.propresenter.timeout_seconds,
    );
    let template_path = match api.presentations().and_then(|all| {
        let chosen = versevision_pp::selection::select_unique_presentation(
            &all,
            Some(&definition.presentation_uuid),
            None,
            definition.library_name.as_deref(),
        )?;
        api.presentation(&chosen.uuid)
    }) {
        Ok(m) => m.presentation_path,
        Err(e) => return fail(rt, &e),
    };
    let template_ok = std::fs::read(&template_path)
        .map_err(|e| VvError::general(crate::cmds::os_error_text(&e, &template_path)))
        .and_then(|bytes| versevision_pp::model::read_presentation(&bytes).map_err(VvError::from))
        .and_then(|p| {
            versevision_pp::template::validate_template(&p, &definition).map_err(VvError::from)
        });
    if let Err(e) = template_ok {
        return fail(rt, &e);
    }
    let probe = versevision_pp::process::SystemProbe;
    let running = versevision_pp::process::ProcessProbe::is_running(&probe);
    if running && a.non_interactive {
        return fail(
            rt,
            &VvError::configuration("Noninteractive writes require ProPresenter to be closed so an in-memory presentation cannot overwrite the import"),
        );
    }
    if running {
        if prompt_enter("Close ProPresenter completely, then press Enter").is_err() {
            return aborted(rt);
        }
        if versevision_pp::process::ProcessProbe::is_running(&probe) {
            return fail(
                rt,
                &VvError::configuration(
                    "ProPresenter is still running. Close it completely before importing; an open process can overwrite the imported file or create duplicate .pro files",
                ),
            );
        }
    }
    if !a.yes {
        match confirm(&format!(
            "Append {} slide(s) to {} with a backup?",
            plan.slides.len(),
            target.presentation_name
        )) {
            Ok(true) => {}
            Ok(false) => return ExitCode::UserCancellation as i32,
            Err(_) => return aborted(rt),
        }
    }
    let mut writer =
        versevision_pp::PresentationWriter::new(versevision_pp::pipeline::SharedUuids::random());
    let now = clock().now_micros() / 1_000_000;
    let written = match versevision_pp::files::append_plan_in_place(
        &mut writer,
        std::path::Path::new(&target.path),
        plan,
        &definition,
        None,
        Some(&template_path),
        None,
        now,
    ) {
        Ok(w) => w,
        Err(e) => {
            return fail(
                rt,
                &VvError::new(
                    versevision_pp::errors::Kind::PresentationWrite,
                    format!("Presentation write failed: {e}. No playlist changes were made."),
                ),
            );
        }
    };
    let mut verification =
        "Structural verification passed; ProPresenter was closed during the write".to_string();
    if running {
        if prompt_enter(&format!(
            "Reopen ProPresenter and open '{}', then press Enter for API verification",
            target.presentation_name
        ))
        .is_err()
        {
            return aborted(rt);
        }
        let (valid, text) = versevision_pp::import_workflow::verify_import_via_api(
            &*clock(),
            config,
            target,
            &written.result,
            plan,
            15.0,
        );
        if !valid {
            let _ = prompt_enter(&format!("Verification failed ({text}). Close ProPresenter completely, then press Enter to roll back"));
            let backup = written.backup_path.clone();
            if versevision_pp::process::ProcessProbe::is_running(&probe) {
                return fail(
                    rt,
                    &VvError::rollback(format!(
                        "Verification failed, but rollback was not attempted because ProPresenter is still running. Close it and restore {} over {}: {text}",
                        backup.display(),
                        target.path
                    )),
                );
            }
            let _ =
                versevision_pp::backup::restore_backup(std::path::Path::new(&target.path), &backup);
            return fail(
                rt,
                &VvError::rollback(format!("Verification failed and the original was restored from {}. Rollback succeeded: {text}", backup.display())),
            );
        }
        verification = text;
    }
    let versions: Vec<String> = {
        let mut v: Vec<String> = result
            .references
            .iter()
            .filter_map(|r| r.version.clone())
            .filter(|v| !v.is_empty())
            .collect();
        v.sort();
        v.dedup();
        v
    };
    let backup = Some(written.backup_path.display().to_string());
    if as_json_output(a) {
        echo(&dumps_utf8(&json!({
            "target": {"library_name": target.library_name, "presentation_name": target.presentation_name, "uuid": target.uuid, "path": target.path},
            "reference_count": result.references.len(), "slide_count": plan.slides.len(), "versions": versions,
            "backup_path": backup, "cue_count_before": written.result.cue_count_before, "cue_count_after": written.result.cue_count_after,
            "group_count_before": written.result.group_count_before, "group_count_after": written.result.group_count_after,
            "verification_result": verification, "files_changed": true,
        })));
    } else {
        let console = rt.term.out();
        let mut text = console.print(&format!(
            "[green]Import verified[/green]: {} slide(s) appended",
            plan.slides.len()
        ));
        text.push_str(&console.print(&format!(
            "Backup: {}",
            backup.clone().unwrap_or_else(|| "None".into())
        )));
        text.push_str(&console.print(&verification));
        out(&text);
    }
    let _ = err;
    0
}

fn as_json_output(a: &ImportArgs) -> bool {
    is_json(&a.output)
}

fn text_of(v: &Value) -> String {
    v.as_str()
        .map_or_else(|| versevision_pp::pyrepr::py_str(v), str::to_string)
}

/// `_print_notes_report(result)`.
fn notes_report(console: &crate::rich::Console, result: &Value) -> String {
    let mut table = Table::new(&["#", "Type", "Slide text"]);
    let slides = result["slide_plan"]["slides"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for (index, slide) in slides.iter().enumerate() {
        let body = text_of(&slide["body_text"]);
        let (kind, text) = if slide["kind"] == "point" {
            (
                "point".to_string(),
                body.replace('\n', " / ")
                    .chars()
                    .take(70)
                    .collect::<String>(),
            )
        } else {
            let label = format!(
                "{} {}",
                text_of(&slide["reference_text"]),
                text_of(&slide["version_text"])
            );
            (
                "verse".to_string(),
                format!(
                    "{}: {}",
                    label.trim(),
                    body.chars().take(50).collect::<String>()
                ),
            )
        };
        table.add_row(vec![
            cell(&(index + 1).to_string()),
            cell(&kind),
            cell(&text),
        ]);
    }
    let mut out = console.print_r(&R::Table(Box::new(table)));
    out.push_str(&console.print(&format!(
        "Slides planned: {} ({} verse passages, {} points)",
        slides.len(),
        text_of(&result["verse_count"]),
        text_of(&result["point_count"])
    )));
    for (title, key, style) in [
        ("NOT ADDED (failed)", "failed", "red"),
        ("NEEDS REVIEW (unclear lines)", "review", "yellow"),
        ("Corrections / defaults applied", "notices", "cyan"),
        ("Left out on purpose", "skipped", "dim"),
    ] {
        let entries = result[key].as_array().cloned().unwrap_or_default();
        if entries.is_empty() {
            continue;
        }
        out.push_str(&console.print(&format!("[{style}]{title}: {}[/{style}]", entries.len())));
        for entry in entries {
            let text: String = text_of(&entry["text"]).chars().take(60).collect();
            out.push_str(&console.print(&format!(
                "  [{style}]line {}: {} - {}[/{style}]",
                text_of(&entry["line"]),
                versevision_pp::pyrepr::py_repr(&text),
                text_of(&entry["problem"])
            )));
        }
    }
    out
}
