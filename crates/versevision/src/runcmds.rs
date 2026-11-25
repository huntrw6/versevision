//! The commands that talk to ProPresenter (`cli_run.py`, and the older typed-API commands of `cli.py`).

use std::sync::Arc;

use serde_json::{json, Value};
use versevision_bible::FileKeyStore;
use versevision_bible::{BibleSources, RegistryBuilder, ScriptureCache};
use versevision_core::config::{AppConfig, NodeType};
use versevision_pp::errors::{ExitCode, VvError};
use versevision_pp::pipeline::{Ctx, SharedUuids};
use versevision_pp::run::{run_import, run_undo, RunRequest};
use versevision_pp::transport::{LocalTransport, Transport};

use crate::app::{echo, err, out, Rt, VERSION};
use crate::cmds::{clock, dumps, dumps_utf8, fail, fail_run, is_json, load_config, read_source};
use crate::def::*;
use crate::render::print_report;
use crate::rich::{markup, Overflow, Style, Table, R};

fn remote_unsupported() -> VvError {
    VvError::configuration(
        "Remote computers (--node, node.type = \"hermes-remote\") are not supported by this build of VerseVision: run it on the computer that runs ProPresenter.",
    )
}

/// `_config_with_node(load_config(), node, None, None)`.
pub fn config_with_node(rt: &Rt, node: Option<&str>) -> Result<AppConfig, VvError> {
    let config = load_config(rt)?;
    if node.is_some_and(|n| !n.is_empty()) {
        return Err(remote_unsupported());
    }
    Ok(config)
}

/// `make_transport(config.propresenter, config.node)`.
pub fn transport(rt: &Rt, config: &AppConfig) -> Result<LocalTransport, VvError> {
    if config.node.kind == NodeType::HermesRemote {
        return Err(remote_unsupported());
    }
    Ok(LocalTransport::new(
        &config.propresenter,
        &config.node,
        clock(),
        Arc::new(versevision_pp::process::SystemProbe),
        rt.paths.home.clone(),
    ))
}

fn bible(rt: &Rt, config: &AppConfig) -> BibleSources {
    let keys = Arc::new(FileKeyStore::new(rt.paths.keys_file.clone()));
    let cache = Arc::new(ScriptureCache::new(rt.paths.cache_dir.join("scripture")));
    RegistryBuilder::new(keys)
        .cache(cache)
        .build_sources(&config.bible)
}

fn ctx(rt: &Rt, progress: bool) -> Ctx {
    let mut ctx = Ctx::new(
        clock(),
        SharedUuids::random(),
        rt.paths.pipeline_dirs(),
        VERSION,
    );
    ctx.history =
        versevision_pp::history::HistoryStore::new(rt.paths.history_file.clone(), clock());
    if progress {
        let console = rt.term.err();
        ctx.progress_sink = Some(Box::new(move |message: &str| {
            err(&console.print(&format!("[cyan]{message}[/cyan]")));
        }));
    }
    ctx
}

fn cell(text: &str) -> R {
    R::Text {
        cells: markup(text, Style::NONE),
        overflow: Overflow::Ellipsis,
        no_wrap: false,
    }
}

// --------------------------------------------------------------------------------------------------------------- run

pub fn run(rt: &Rt, a: RunArgs) -> i32 {
    let as_json = is_json(&a.output);
    let config = match config_with_node(rt, a.node.as_deref()).and_then(|c| {
        let sets: Vec<&str> = a.set_options.iter().map(String::as_str).collect();
        c.with_options(a.profile.as_deref().filter(|p| !p.is_empty()), &sets)
            .map(|_| c)
            .map_err(|e| VvError::configuration(e.to_string()))
    }) {
        Ok(c) => c,
        Err(e) => return fail_run(rt, &e, as_json),
    };
    let (source, source_name, extract) = match read_source(&a.file, &a.text) {
        Ok(t) => t,
        Err(e) => return fail_run(rt, &e, as_json),
    };
    let transport = match transport(rt, &config) {
        Ok(t) => t,
        Err(e) => return fail_run(rt, &e, as_json),
    };
    let request = RunRequest {
        source,
        source_name: if a.file.is_some() {
            source_name
        } else {
            "text".into()
        },
        extract,
        presentation: a.presentation.clone().or(a.target_presentation.clone()),
        presentation_uuid: a.presentation_uuid.clone().or(a.target_uuid.clone()),
        title: a.title.clone(),
        library: a.library.clone(),
        replace: a.replace,
        append: a.append,
        default_version: a.default_version.clone(),
        dry_run: a.dry_run,
        yes: a.yes,
        no_playlist: a.no_playlist,
        as_copy: a.as_copy,
        in_place: a.in_place,
        no_close: a.no_close,
        no_reopen: a.no_reopen,
        profile: a.profile.clone(),
        add_line: a
            .add_line
            .iter()
            .filter(|n| **n >= 0)
            .map(|n| *n as usize)
            .collect(),
        set_options: a.set_options.clone(),
    };
    let mut context = ctx(rt, !as_json);
    let sources = bible(rt, &config);
    let outcome = crate::block::block_on(run_import(
        &mut context,
        &request,
        &config,
        &transport,
        Some(&sources),
    ));
    for warning in &outcome.warnings {
        err(&rt.term.err().print(&format!("[yellow]{warning}[/yellow]")));
    }
    if let Some(error) = &outcome.error {
        return fail_run(rt, error, as_json);
    }
    let Some(report) = &outcome.report else {
        return outcome.exit_code as i32;
    };
    if as_json {
        echo(&dumps_utf8(report));
    } else {
        let console = rt.term.err();
        let mut text = print_report(&console, report);
        if let Some(applied) = &outcome.applied {
            let what = if applied.mode == "new" {
                "new presentation"
            } else if a.as_copy {
                "new copy"
            } else {
                "presentation"
            };
            let name = if applied.presentation_name.is_empty() {
                &applied.target_name
            } else {
                &applied.presentation_name
            };
            text.push_str(&console.print(&format!(
                "[green]Imported {} slides into {what} '{name}'[/green] (import {})",
                applied.slide_count, applied.import_id
            )));
            text.push_str(&console.print(&applied.verification));
            if let Some(run_id) = &outcome.run_id {
                text.push_str(&console.print(&format!(
                    "[dim]Log: {}[/dim]",
                    rt.paths.runs_dir().join(run_id).display()
                )));
            }
        }
        err(&text);
    }
    outcome.exit_code as i32
}

pub fn undo(rt: &Rt, a: UndoArgs) -> i32 {
    let as_json = is_json(&a.output);
    let config = match config_with_node(rt, a.node.as_deref()) {
        Ok(c) => c,
        Err(e) => return fail_run(rt, &e, as_json),
    };
    let transport = match transport(rt, &config) {
        Ok(t) => t,
        Err(e) => return fail_run(rt, &e, as_json),
    };
    let mut context = ctx(rt, !as_json);
    let outcome = run_undo(&mut context, &a.import_id, &config, &transport, a.no_close);
    if let Some(error) = &outcome.error {
        return fail_run(rt, error, as_json);
    }
    if let Some(result) = &outcome.applied {
        if as_json {
            let mut value = json!({"ok": true});
            if let (Some(map), Value::Object(applied)) = (value.as_object_mut(), result.to_value())
            {
                map.extend(applied);
            }
            echo(&dumps(&value));
        } else {
            err(&rt.term.err().print(&format!(
                "[green]Removed {} slides from {}[/green]",
                result.removed_slides, result.target_name
            )));
        }
    }
    outcome.exit_code as i32
}

pub fn presentations(rt: &Rt, a: PresentationsArgs) -> i32 {
    let as_json = is_json(&a.output);
    let result = config_with_node(rt, a.node.as_deref()).and_then(|config| {
        let t = transport(rt, &config)?;
        let items = versevision_pp::pipeline::list_presentations(&t)?;
        Ok((config, items))
    });
    let (config, mut items) = match result {
        Ok(r) => r,
        Err(e) => return fail_run(rt, &e, as_json),
    };
    if let Some(library) = a.library.as_deref().filter(|l| !l.is_empty()) {
        items.retain(|i| i.library.to_lowercase() == library.to_lowercase());
    } else if !a.all_libraries && !as_json {
        items.retain(|i| i.library.to_lowercase() == config.output.library.to_lowercase());
    }
    if as_json {
        let values: Vec<Value> = items.iter().map(|i| i.to_value()).collect();
        echo(&dumps_utf8(&Value::Array(values)));
        return 0;
    }
    let mut table = Table::new(&["Library", "Presentation", "UUID"]);
    for item in &items {
        table.add_row(vec![
            cell(&item.library),
            cell(&item.name),
            cell(&item.uuid),
        ]);
    }
    out(&rt.term.out().print_r(&R::Table(Box::new(table))));
    0
}

pub fn playlists(rt: &Rt, a: PlaylistsArgs) -> i32 {
    let result = config_with_node(rt, a.node.as_deref()).and_then(|config| {
        let t = transport(rt, &config)?;
        versevision_pp::pipeline::list_playlist_presentations(&t)
    });
    let playlists = match result {
        Ok(p) => p,
        Err(e) => {
            out(&rt.term.out().print(&format!("[red]{}[/red]", e.message)));
            return e.exit_code() as i32;
        }
    };
    if is_json(&a.output) {
        let values: Vec<Value> = playlists.iter().map(|p| p.to_value()).collect();
        echo(&dumps(&Value::Array(values)));
        return 0;
    }
    let console = rt.term.out();
    let mut text = String::new();
    for pl in &playlists {
        text.push_str(&console.print(&format!("[bold]{}[/bold]  ({})", pl.name, pl.entries.len())));
        let mut section: Option<&str> = None;
        for entry in &pl.entries {
            if section != Some(entry.section.as_str()) {
                section = Some(entry.section.as_str());
                let label = if entry.section.is_empty() {
                    "-"
                } else {
                    entry.section.as_str()
                };
                text.push_str(&console.print(&format!("  [dim]{label}[/dim]")));
            }
            let library = if entry.library.is_empty() {
                "not in a library"
            } else {
                entry.library.as_str()
            };
            text.push_str(&console.print(&format!("    {}  [dim]{library}[/dim]", entry.name)));
        }
    }
    out(&text);
    0
}

pub fn node(rt: &Rt, a: NodeArgs) -> i32 {
    let as_json = is_json(&a.output);
    let result = config_with_node(rt, a.node.as_deref()).and_then(|config| {
        let t = transport(rt, &config)?;
        let info = t.api_get("/version")?;
        Ok((t.description(), info, t.is_running()))
    });
    match result {
        Ok((description, info, running)) => {
            if as_json {
                echo(&dumps(
                    &json!({"ok": true, "node": description, "propresenter": info, "app_running": running}),
                ));
            } else {
                err(&rt.term.err().print(&format!(
                    "[green]Connected[/green] to {description}: {}, API {}",
                    py(&info["host_description"]),
                    py(&info["api_version"])
                )));
            }
            0
        }
        Err(e) => fail_run(rt, &e, as_json),
    }
}

fn py(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::String(s) => s.clone(),
        other => versevision_pp::pyrepr::py_str(other),
    }
}

pub fn providers_test(rt: &Rt, a: OutputOnly) -> i32 {
    let config = match load_config(rt) {
        Ok(c) => c,
        Err(e) => return fail(rt, &e),
    };
    let sources = bible(rt, &config);
    let results = crate::block::block_on(sources.own().health_checks());
    if is_json(&a.output) {
        let values: Vec<Value> = results
            .iter()
            .map(|r| serde_json::to_value(r).unwrap_or(Value::Null))
            .collect();
        echo(&dumps(&Value::Array(values)));
    } else {
        let mut table = Table::new(&["Provider", "Healthy", "Versions", "Message"]);
        for r in &results {
            table.add_row(vec![
                cell(if r.provider.as_deref().unwrap_or("").is_empty() {
                    "—"
                } else {
                    r.provider.as_deref().unwrap_or("")
                }),
                cell(if r.healthy { "Yes" } else { "No" }),
                cell(&r.versions.join(", ")),
                cell(&r.message),
            ]);
        }
        out(&rt.term.out().print_r(&R::Table(Box::new(table))));
    }
    if results.iter().any(|r| !r.healthy) {
        ExitCode::BibleProvider as i32
    } else {
        0
    }
}
