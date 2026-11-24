//! The older commands of `cli.py` that talk to ProPresenter's typed API and read presentation files: `doctor`, `propresenter ...`,
//! `template inspect|select|select-point|validate` and the node-style `template check|new-file|setup` of `cli_run.py`.

use std::io::BufRead;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use versevision_core::config::{AppConfig, PresentationTemplateConfig};
use versevision_pp::api_client::{PresentationSummary, ProPresenterApiClient};
use versevision_pp::errors::{ExitCode, VvError};
use versevision_pp::pipeline::find_template_target;
use versevision_pp::process::ProcessProbe;
use versevision_pp::transport::Transport;

use crate::app::{echo, err, out, Rt, Usage};
use crate::cmds::{clock, dumps, dumps_utf8, fail, is_json, load_config, save_config};
use crate::def::*;
use crate::rich::{markup, Overflow, Style, Table, R};
use crate::runcmds::{config_with_node, transport};

fn cell(text: &str) -> R {
    R::Text {
        cells: markup(text, Style::NONE),
        overflow: Overflow::Ellipsis,
        no_wrap: false,
    }
}

fn usage_error(path: &str, message: &str) -> Usage {
    Usage {
        path: path.to_string(),
        message: format!("Invalid value: {message}"),
    }
}

fn client(config: &AppConfig, host: Option<&str>, port: Option<i64>) -> ProPresenterApiClient {
    let pp = &config.propresenter;
    ProPresenterApiClient::new(
        host.filter(|h| !h.is_empty()).unwrap_or(&pp.host),
        port.filter(|p| *p != 0).map_or(pp.port, |p| p as u16),
        pp.timeout_seconds,
    )
}

/// Typer's `Abort`: "Aborted." in red on the error stream, exit 1.
pub struct Abort;

pub fn aborted(rt: &Rt) -> i32 {
    err(&rt.term.err().print_styled("Aborted.", "red"));
    1
}

fn read_line() -> Option<String> {
    let mut line = String::new();
    match std::io::stdin().lock().read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line.trim_end_matches(['\n', '\r']).to_string()),
    }
}

/// `typer.prompt(text, type=int)`.
pub fn prompt_int(text: &str) -> Result<i64, Abort> {
    loop {
        out(&format!("{text}: "));
        let Some(line) = read_line() else {
            out("\n");
            return Err(Abort);
        };
        match line.trim().parse::<i64>() {
            Ok(n) => return Ok(n),
            Err(_) => out(&format!(
                "Error: {} is not a valid integer.\n",
                versevision_pp::pyrepr::py_repr(line.trim())
            )),
        }
    }
}

/// `typer.prompt(text, default="", show_default=False)`: press Enter.
pub fn prompt_enter(text: &str) -> Result<(), Abort> {
    out(&format!("{text}: "));
    match read_line() {
        Some(_) => Ok(()),
        None => {
            out("\n");
            Err(Abort)
        }
    }
}

/// `typer.confirm(text)` (default no).
pub fn confirm(text: &str) -> Result<bool, Abort> {
    loop {
        out(&format!("{text} [y/N]: "));
        let Some(line) = read_line() else {
            out("\n");
            return Err(Abort);
        };
        match line.trim().to_lowercase().as_str() {
            "y" | "yes" => return Ok(true),
            "n" | "no" | "" => return Ok(false),
            _ => out("Error: invalid input\n"),
        }
    }
}

fn read_presentation(path: &Path) -> Result<versevision_pp::pb::Message, VvError> {
    let bytes =
        std::fs::read(path).map_err(|e| VvError::general(crate::cmds::os_error_text(&e, path)))?;
    versevision_pp::model::read_presentation(&bytes).map_err(VvError::from)
}

// ------------------------------------------------------------------------------------------------------------ doctor

pub fn doctor(rt: &Rt, a: DoctorArgs) -> i32 {
    let config = match load_config(rt) {
        Ok(c) => c,
        Err(e) => return fail(rt, &e),
    };
    let api = client(&config, a.host.as_deref(), a.port);
    let result = api
        .version()
        .and_then(|version| api.libraries().map(|libraries| (version, libraries)));
    let (version, libraries) = match result {
        Ok(r) => r,
        Err(e) => return fail(rt, &e),
    };
    let running = versevision_pp::process::SystemProbe.is_running();
    if is_json(&a.output) {
        echo(&dumps(&json!({
            "healthy": true, "process_running": running, "version": serde_json::to_value(&version).unwrap_or(Value::Null),
            "library_count": libraries.len(), "files_changed": false,
        })));
    } else {
        let console = rt.term.out();
        let mut text = console.print(&format!(
            "[green]Connected[/green]: {}, API {}",
            version.host_description, version.api_version
        ));
        text.push_str(&console.print(&format!(
            "Libraries: {} · ProPresenter process running: {}",
            libraries.len(),
            if running { "True" } else { "False" }
        )));
        out(&text);
    }
    0
}

// ------------------------------------------------------------------------------------------------------- propresenter

pub fn propresenter_list(rt: &Rt, kind: &str, a: OutputOnly) -> i32 {
    let config = match load_config(rt) {
        Ok(c) => c,
        Err(e) => return fail(rt, &e),
    };
    let api = client(&config, None, None);
    // (name, uuid, library) of every item, and the JSON of each
    type Listing = (Vec<(String, String, String)>, Vec<Value>);
    let result: Result<Listing, VvError> = (|| match kind {
        "libraries" => {
            let items = api.libraries()?;
            Ok((
                items
                    .iter()
                    .map(|i| (i.name.clone(), i.uuid.clone(), "—".to_string()))
                    .collect(),
                items
                    .iter()
                    .map(|i| serde_json::to_value(i).unwrap_or(Value::Null))
                    .collect(),
            ))
        }
        "presentations" => {
            let items = api.presentations()?;
            Ok((
                items
                    .iter()
                    .map(|i| {
                        (
                            i.name.clone(),
                            i.uuid.clone(),
                            i.library_name
                                .clone()
                                .filter(|l| !l.is_empty())
                                .unwrap_or_else(|| "—".into()),
                        )
                    })
                    .collect(),
                items
                    .iter()
                    .map(|i| serde_json::to_value(i).unwrap_or(Value::Null))
                    .collect(),
            ))
        }
        _ => {
            let items = api.playlists()?;
            Ok((
                items
                    .iter()
                    .map(|i| {
                        let id = i.id.as_ref();
                        (
                            id.map_or("—".to_string(), |x| x.name.clone()),
                            id.map_or("—".to_string(), |x| x.uuid.clone()),
                            "—".to_string(),
                        )
                    })
                    .collect(),
                items
                    .iter()
                    .map(|i| serde_json::to_value(i).unwrap_or(Value::Null))
                    .collect(),
            ))
        }
    })();
    let (rows, values) = match result {
        Ok(r) => r,
        Err(e) => return fail(rt, &e),
    };
    if is_json(&a.output) {
        echo(&dumps(&Value::Array(values)));
    } else {
        let mut table = Table::new(&["Name", "UUID", "Library"]);
        for (name, uuid, library) in rows {
            table.add_row(vec![cell(&name), cell(&uuid), cell(&library)]);
        }
        out(&rt.term.out().print_r(&R::Table(Box::new(table))));
    }
    0
}

// ------------------------------------------------------------------------------------------------------------ template

struct Resolved {
    selected: PresentationSummary,
    path: PathBuf,
}

fn resolve_template_target(
    config: &AppConfig,
    uuid: Option<&str>,
    name: Option<&str>,
    library: Option<&str>,
) -> Result<Resolved, VvError> {
    let api = client(config, None, None);
    let presentations = api.presentations()?;
    let selected =
        versevision_pp::selection::select_unique_presentation(&presentations, uuid, name, library)?;
    let metadata = api.presentation(&selected.uuid)?;
    Ok(Resolved {
        selected,
        path: metadata.presentation_path,
    })
}

fn with_overrides(mut config: AppConfig, host: Option<&str>, port: Option<i64>) -> AppConfig {
    if host.is_none_or(str::is_empty) && port.is_none_or(|p| p == 0) {
        return config;
    }
    if let Some(h) = host.filter(|h| !h.is_empty()) {
        config.propresenter.host = h.to_string();
    }
    if let Some(p) = port.filter(|p| *p != 0) {
        config.propresenter.port = p as u16;
    }
    config
}

pub fn template(rt: &Rt, command: TemplateCommand) -> Result<i32, Usage> {
    match command {
        TemplateCommand::Inspect {
            target_uuid,
            target_presentation,
            library,
            host,
            port,
            output,
            ..
        } => {
            let config = match load_config(rt) {
                Ok(c) => with_overrides(c, host.as_deref(), port),
                Err(e) => return Ok(fail(rt, &e)),
            };
            let resolved = match resolve_template_target(
                &config,
                target_uuid.as_deref(),
                target_presentation.as_deref(),
                library.as_deref(),
            ) {
                Ok(r) => r,
                Err(e) => return Ok(fail(rt, &e)),
            };
            let presentation = match read_presentation(&resolved.path) {
                Ok(p) => p,
                Err(e) => return Ok(fail(rt, &e)),
            };
            let candidates = versevision_pp::template::inspect_template_candidates(
                &presentation,
                "Reference",
                "Verse",
            );
            if is_json(&output) {
                let values: Vec<Value> = candidates
                    .iter()
                    .map(|c| serde_json::to_value(c).unwrap_or(Value::Null))
                    .collect();
                echo(&dumps(&Value::Array(values)));
                return Ok(0);
            }
            let console = rt.term.out();
            let mut table = Table::new(&["Slide", "Reference", "Verse preview", "Cue UUID"]);
            for c in &candidates {
                table.add_row(vec![
                    cell(&c.slide_number.to_string()),
                    cell(&c.reference_preview),
                    cell(&c.body_preview),
                    cell(&c.cue_uuid),
                ]);
            }
            let mut text = console.print(&format!(
                "{} / {}",
                resolved
                    .selected
                    .library_name
                    .clone()
                    .unwrap_or_else(|| "None".into()),
                resolved.selected.name
            ));
            text.push_str(&console.print_r(&R::Table(Box::new(table))));
            out(&text);
            Ok(0)
        }
        TemplateCommand::Select {
            target_uuid,
            target_presentation,
            library,
            cue_uuid,
            host,
            port,
            ..
        } => {
            let path = "versevision template select";
            let config = match load_config(rt) {
                Ok(c) => with_overrides(c, host.as_deref(), port),
                Err(e) => return Ok(fail(rt, &e)),
            };
            let resolved = match resolve_template_target(
                &config,
                target_uuid.as_deref(),
                target_presentation.as_deref(),
                library.as_deref(),
            ) {
                Ok(r) => r,
                Err(e) => return Ok(fail(rt, &e)),
            };
            let presentation = match read_presentation(&resolved.path) {
                Ok(p) => p,
                Err(e) => return Ok(fail(rt, &e)),
            };
            let candidates = versevision_pp::template::inspect_template_candidates(
                &presentation,
                "Reference",
                "Verse",
            );
            if candidates.is_empty() {
                return Err(usage_error(
                    path,
                    "no slides contain exactly one Reference and one Verse text object",
                ));
            }
            let wanted = cue_uuid.as_deref().filter(|c| !c.is_empty());
            let mut chosen = wanted.and_then(|w| {
                candidates
                    .iter()
                    .find(|c| c.cue_uuid.to_lowercase() == w.to_lowercase())
            });
            if wanted.is_some() && chosen.is_none() {
                return Err(usage_error(
                    path,
                    "--cue-uuid is not one of the viable template cues",
                ));
            }
            if chosen.is_none() {
                let console = rt.term.out();
                let mut table = Table::new(&["Choice", "Slide", "Reference", "Verse preview"]);
                for (index, c) in candidates.iter().enumerate() {
                    table.add_row(vec![
                        cell(&(index + 1).to_string()),
                        cell(&c.slide_number.to_string()),
                        cell(&c.reference_preview),
                        cell(&c.body_preview),
                    ]);
                }
                out(&console.print_r(&R::Table(Box::new(table))));
                let Ok(choice) = prompt_int("Choose template") else {
                    return Ok(aborted(rt));
                };
                if choice < 1 || choice as usize > candidates.len() {
                    return Err(usage_error(path, "template choice is out of range"));
                }
                chosen = candidates.get(choice as usize - 1);
            }
            let Some(chosen) = chosen else { return Ok(1) };
            let definition = match versevision_pp::template::create_template_definition(
                &presentation,
                &resolved.selected.uuid,
                &resolved.selected.name,
                resolved.selected.library_name.as_deref(),
                &chosen.cue_uuid,
                "Reference",
                "Verse",
            ) {
                Ok(d) => d,
                Err(e) => return Ok(fail(rt, &VvError::from(e))),
            };
            let mut updated = config.clone();
            updated.presentation_template = Some(definition);
            updated.slides.include_verse_numbers = false;
            if let Err(e) = save_config(rt, &updated, true) {
                return Ok(fail(rt, &e));
            }
            out(&rt.term.out().print(&format!(
                "Template saved from slide {}: {}",
                chosen.slide_number, chosen.reference_preview
            )));
            Ok(0)
        }
        TemplateCommand::SelectPoint {
            cue_uuid,
            point_object,
            ..
        } => {
            let path = "versevision template select-point";
            let config = match load_config(rt) {
                Ok(c) => c,
                Err(e) => return Ok(fail(rt, &e)),
            };
            let Some(definition) = config.presentation_template.clone() else {
                return Err(usage_error(
                    path,
                    "select the verse template first: versevision template select",
                ));
            };
            let resolved = match resolve_template_target(
                &config,
                Some(&definition.presentation_uuid),
                None,
                definition.library_name.as_deref(),
            ) {
                Ok(r) => r,
                Err(e) => return Ok(fail(rt, &e)),
            };
            let presentation = match read_presentation(&resolved.path) {
                Ok(p) => p,
                Err(e) => return Ok(fail(rt, &e)),
            };
            let candidates =
                versevision_pp::template::inspect_point_candidates(&presentation, &point_object);
            if candidates.is_empty() {
                return Err(usage_error(
                    path,
                    &format!(
                        "no slide in {} has exactly one text object named {}",
                        resolved.selected.name,
                        versevision_pp::pyrepr::py_repr(&point_object)
                    ),
                ));
            }
            let wanted = cue_uuid.as_deref().filter(|c| !c.is_empty());
            let mut chosen = wanted.and_then(|w| {
                candidates
                    .iter()
                    .find(|c| c.1.to_lowercase() == w.to_lowercase())
            });
            if wanted.is_some() && chosen.is_none() {
                return Err(usage_error(
                    path,
                    "--cue-uuid is not one of the viable sermon-point cues",
                ));
            }
            if chosen.is_none() {
                if candidates.len() == 1 {
                    chosen = candidates.first();
                } else {
                    let mut table = Table::new(&["Choice", "Slide", "Cue UUID"]);
                    for (number, item) in candidates.iter().enumerate() {
                        table.add_row(vec![
                            cell(&(number + 1).to_string()),
                            cell(&item.0.to_string()),
                            cell(&item.1),
                        ]);
                    }
                    out(&rt.term.out().print_r(&R::Table(Box::new(table))));
                    let Ok(choice) = prompt_int("Choose sermon-point template") else {
                        return Ok(aborted(rt));
                    };
                    if choice < 1 || choice as usize > candidates.len() {
                        return Err(usage_error(path, "choice is out of range"));
                    }
                    chosen = candidates.get(choice as usize - 1);
                }
            }
            let Some(chosen) = chosen else { return Ok(1) };
            let mut updated_definition = definition;
            updated_definition.point_cue_uuid = Some(chosen.1.clone());
            updated_definition.point_object = point_object;
            updated_definition.point_style_fingerprint = Some(chosen.2.clone());
            let mut updated = config.clone();
            updated.presentation_template = Some(updated_definition);
            if let Err(e) = save_config(rt, &updated, true) {
                return Ok(fail(rt, &e));
            }
            out(&rt.term.out().print(&format!(
                "Sermon-point template saved from slide {} of {}",
                chosen.0, resolved.selected.name
            )));
            Ok(0)
        }
        TemplateCommand::Validate(a) => {
            let path = "versevision template validate";
            let config = match load_config(rt) {
                Ok(c) => c,
                Err(e) => return Ok(fail(rt, &e)),
            };
            let Some(definition) = config.presentation_template.clone() else {
                return Err(usage_error(
                    path,
                    "no presentation template is configured; run template select",
                ));
            };
            let resolved = match resolve_template_target(
                &config,
                Some(&definition.presentation_uuid),
                None,
                definition.library_name.as_deref(),
            ) {
                Ok(r) => r,
                Err(e) => return Ok(fail(rt, &e)),
            };
            let presentation = match read_presentation(&resolved.path) {
                Ok(p) => p,
                Err(e) => return Ok(fail(rt, &e)),
            };
            let candidate =
                match versevision_pp::template::validate_template(&presentation, &definition) {
                    Ok(c) => c,
                    Err(e) => return Ok(fail(rt, &VvError::from(e))),
                };
            if is_json(&a.output) {
                echo(&dumps(&json!({
                    "valid": true, "presentation_uuid": resolved.selected.uuid, "cue_uuid": candidate.cue_uuid,
                    "slide_number": candidate.slide_number, "reference_object": definition.reference_object,
                    "body_object": definition.body_object, "files_changed": false,
                })));
            } else {
                echo(&format!(
                    "Template is valid: slide {} in {}",
                    candidate.slide_number, resolved.selected.name
                ));
            }
            Ok(0)
        }
        TemplateCommand::Check(a) => Ok(template_check(rt, a)),
        TemplateCommand::NewFile {
            source_file,
            source,
            name,
            library,
            node,
            ..
        } => Ok(template_new_file(
            rt,
            source_file,
            source,
            name,
            library,
            node,
        )),
        TemplateCommand::Setup {
            target_presentation,
            library,
            verse_slide,
            point_slide,
            node,
            ..
        } => Ok(template_setup(
            rt,
            target_presentation,
            library,
            verse_slide,
            point_slide,
            node,
        )),
    }
}

fn run_ctx(rt: &Rt) -> versevision_pp::pipeline::Ctx {
    let mut ctx = versevision_pp::pipeline::Ctx::new(
        clock(),
        versevision_pp::pipeline::SharedUuids::random(),
        rt.paths.pipeline_dirs(),
        crate::app::VERSION,
    );
    ctx.history =
        versevision_pp::history::HistoryStore::new(rt.paths.history_file.clone(), clock());
    ctx
}

fn fetch_to_temp(
    transport: &dyn Transport,
    remote: &str,
) -> Result<(tempfile::TempDir, PathBuf), VvError> {
    let dir = tempfile::tempdir().map_err(VvError::from)?;
    let file_name = Path::new(remote)
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_else(|| "presentation.pro".into());
    let local = dir.path().join(file_name);
    transport.fetch_file(remote, &local)?;
    Ok((dir, local))
}

fn template_check(rt: &Rt, a: NodeArgs) -> i32 {
    let as_json = is_json(&a.output);
    let result = (|| -> Result<Value, VvError> {
        let config = config_with_node(rt, a.node.as_deref())?;
        let Some(definition) = config.presentation_template.clone() else {
            return Err(VvError::configuration(
                "no template is configured; run: versevision template setup",
            ));
        };
        let t = transport(rt, &config)?;
        let mut ctx = run_ctx(rt);
        let target = find_template_target(&mut ctx, &t, &definition)?;
        let (_dir, local) = fetch_to_temp(&t, &target.path)?;
        let presentation = read_presentation(&local)?;
        let candidate = versevision_pp::template::validate_template(&presentation, &definition)
            .map_err(VvError::from)?;
        Ok(
            json!({"ok": true, "presentation": target.name, "verse_slide": candidate.slide_number, "has_point_template": definition.point_cue_uuid.as_deref().is_some_and(|c| !c.is_empty())}),
        )
    })();
    match result {
        Ok(payload) => {
            if as_json {
                echo(&dumps(&payload));
            } else {
                err(&rt.term.err().print(&format!(
                    "[green]Templates are valid[/green] in {} (verse slide {})",
                    payload["presentation"].as_str().unwrap_or(""),
                    payload["verse_slide"]
                )));
            }
            0
        }
        Err(e) => crate::cmds::fail_run(rt, &e, as_json),
    }
}

fn template_setup(
    rt: &Rt,
    target_presentation: String,
    library: Option<String>,
    verse_slide: Option<i64>,
    point_slide: Option<i64>,
    node: Option<String>,
) -> i32 {
    let result = (|| -> Result<String, VvError> {
        let config = config_with_node(rt, node.as_deref())?;
        let t = transport(rt, &config)?;
        let mut ctx = run_ctx(rt);
        let target = versevision_pp::pipeline::target_from_api(
            &mut ctx,
            &t,
            None,
            Some(&target_presentation),
            library.as_deref(),
        )?;
        let (_dir, local) = fetch_to_temp(&t, &target.path)?;
        let presentation = read_presentation(&local)?;
        let verses = versevision_pp::template::inspect_template_candidates(
            &presentation,
            "Reference",
            "Verse",
        );
        let points =
            versevision_pp::template::inspect_point_candidates(&presentation, "Sermon Point");
        if verses.is_empty() {
            return Err(VvError::configuration(
                "no slide has exactly one 'Reference' and one 'Verse' text object",
            ));
        }
        let chosen = match verse_slide.filter(|n| *n != 0) {
            Some(n) => verses.iter().find(|v| v.slide_number as i64 == n),
            None => verses.first(),
        };
        let Some(chosen) = chosen else {
            let valid: Vec<String> = verses.iter().map(|v| v.slide_number.to_string()).collect();
            return Err(VvError::configuration(format!(
                "slide {} is not a valid verse template; valid: [{}]",
                verse_slide.unwrap_or(0),
                valid.join(", ")
            )));
        };
        let mut definition = versevision_pp::template::create_template_definition(
            &presentation,
            &target.uuid,
            &target.name,
            Some(&target.library),
            &chosen.cue_uuid,
            "Reference",
            "Verse",
        )
        .map_err(VvError::from)?;
        let mut point_text = " (no sermon-point slide found)".to_string();
        if !points.is_empty() {
            let pick = match point_slide.filter(|n| *n != 0) {
                Some(n) => points.iter().find(|p| p.0 as i64 == n),
                None => points.first(),
            };
            let Some(pick) = pick else {
                let valid: Vec<String> = points.iter().map(|p| p.0.to_string()).collect();
                return Err(VvError::configuration(format!(
                    "slide {} is not a valid sermon-point template; valid: [{}]",
                    point_slide.unwrap_or(0),
                    valid.join(", ")
                )));
            };
            definition.point_cue_uuid = Some(pick.1.clone());
            definition.point_style_fingerprint = Some(pick.2.clone());
            point_text = format!(" and point template from slide {}", pick.0);
        }
        let mut updated = config.clone();
        updated.presentation_template = Some(definition);
        save_config(rt, &updated, true)?;
        Ok(format!(
            "[green]Saved[/green] verse template from slide {}{point_text}",
            chosen.slide_number
        ))
    })();
    match result {
        Ok(line) => {
            err(&rt.term.err().print(&line));
            0
        }
        Err(e) => crate::cmds::fail_run(rt, &e, false),
    }
}

fn template_new_file(
    rt: &Rt,
    source_file: Option<String>,
    source: Option<String>,
    name: String,
    library: Option<String>,
    node: Option<String>,
) -> i32 {
    let result = (|| -> Result<String, VvError> {
        let config = config_with_node(rt, node.as_deref())?;
        let Some(definition) = config.presentation_template.clone() else {
            return Err(VvError::configuration(
                "No template is configured; run: versevision template setup",
            ));
        };
        let t = transport(rt, &config)?;
        let lib = library
            .clone()
            .filter(|l| !l.is_empty())
            .unwrap_or_else(|| config.output.library.clone());
        let mut ctx = run_ctx(rt);
        let src_path = match source_file.as_deref().filter(|s| !s.is_empty()) {
            Some(path) => path.to_string(),
            None => {
                let wanted = source
                    .clone()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| definition.presentation_name.clone());
                versevision_pp::pipeline::target_from_api(&mut ctx, &t, None, Some(&wanted), None)?
                    .path
            }
        };
        let (dir, local) = fetch_to_temp(&t, &src_path)?;
        let out_path = dir.path().join(format!("{name}.pro"));
        let bytes = std::fs::read(&local).map_err(VvError::from)?;
        let mut writer = versevision_pp::PresentationWriter::new(
            versevision_pp::pipeline::SharedUuids::random(),
        );
        let now = ctx.now_micros() / 1_000_000;
        let written = writer
            .create_template_file(&bytes, &definition, &name, now)
            .map_err(VvError::from)?;
        std::fs::write(&out_path, &written.bytes).map_err(VvError::from)?;
        let folder = t.library_folder(&lib);
        if !t.find_presentation(&name, Some(&lib)).is_empty() {
            return Err(VvError::configuration(format!(
                "a presentation named {} already exists in {lib}; nothing was changed",
                versevision_pp::pyrepr::py_repr(&name)
            )));
        }
        t.create_file(&out_path, &format!("{folder}/{name}.pro"))?;
        err(&rt.term.err().print(&format!(
            "[cyan]Created {name} in {lib}; waiting for ProPresenter to list it[/cyan]"
        )));
        let mut listed = None;
        for _ in 0..20 {
            listed = versevision_pp::pipeline::list_presentations(&t)
                .ok()
                .and_then(|items| {
                    items
                        .into_iter()
                        .find(|i| i.name == name && i.library.to_lowercase() == lib.to_lowercase())
                });
            if listed.is_some() {
                break;
            }
            clock().sleep(1.5);
        }
        let Some(listed) = listed else {
            return Err(VvError::configuration(
                "The file was created but ProPresenter has not listed it yet. Run this command again after it appears (it will keep the file).",
            ));
        };
        let mut updated_definition: PresentationTemplateConfig = definition;
        updated_definition.presentation_uuid = listed.uuid;
        updated_definition.presentation_name = name.clone();
        updated_definition.library_name = Some(lib.clone());
        let mut updated = config.clone();
        updated.presentation_template = Some(updated_definition);
        save_config(rt, &updated, true)?;
        Ok(format!(
            "[green]Done.[/green] The templates now live in {lib} / {name}. Settings updated."
        ))
    })();
    match result {
        Ok(line) => {
            err(&rt.term.err().print(&line));
            0
        }
        Err(e) => crate::cmds::fail_run(rt, &e, false),
    }
}

#[allow(dead_code)]
fn unused() -> (ExitCode, fn(&Value) -> String) {
    (ExitCode::Success, dumps_utf8_ref)
}

fn dumps_utf8_ref(v: &Value) -> String {
    dumps_utf8(v)
}
