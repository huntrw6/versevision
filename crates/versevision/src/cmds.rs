//! The commands (`cli.py`, `cli_run.py`, `cli_explain.py`, `cli_themes.py`), one function each. Output goes through `rich.rs` so the
//! tables, wrapping and colours are Python's; what a command computes comes from the library crates.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Command as ClapCommand;
use serde_json::{json, Map, Value};
use versevision_core::config::schema;
use versevision_core::config::AppConfig;
use versevision_pp::errors::{ExitCode, VvError};
use versevision_pp::pyjson;

use crate::app::{echo, echo_err, err, out, Rt, Usage};
use crate::def::*;
use crate::rich::{self, Column, Console, Overflow, Style, Table, R};

type Result<T> = std::result::Result<T, Usage>;

// ------------------------------------------------------------------------------------------------------------- helpers

pub fn is_json(output: &str) -> bool {
    output.to_lowercase() == "json"
}

/// `json.dumps(value, indent=2)` (ASCII escapes, as Python's default).
pub fn dumps(value: &Value) -> String {
    pyjson::dumps(value, Some(2), true)
}

/// `json.dumps(value, indent=2, ensure_ascii=False)`.
pub fn dumps_utf8(value: &Value) -> String {
    pyjson::dumps(value, Some(2), false)
}

/// `[Errno N] ...: 'path'` of a Python `OSError`.
pub fn os_error_text(error: &std::io::Error, path: &Path) -> String {
    use std::io::ErrorKind;
    let (code, what) = match error.kind() {
        ErrorKind::NotFound => (2, "No such file or directory"),
        ErrorKind::PermissionDenied => (13, "Permission denied"),
        ErrorKind::AlreadyExists => (17, "File exists"),
        _ => {
            if path.is_dir() {
                (21, "Is a directory")
            } else {
                return error.to_string();
            }
        }
    };
    format!(
        "[Errno {code}] {what}: {}",
        versevision_pp::pyrepr::py_repr(&path.display().to_string())
    )
}

pub fn config_error_text(path: &Path, error: &versevision_core::config::ConfigError) -> String {
    use versevision_core::config::ConfigError as E;
    match error {
        E::Invalid(v) => {
            let kind = error_type(&v.msg);
            format!(
                "Invalid configuration at {}: 1 validation error for AppConfig\n{}\n  {} [type={kind}]\n    For further information visit https://errors.pydantic.dev/2.13/v/{kind}",
                path.display(),
                v.location(),
                v.msg
            )
        }
        other => format!("Invalid configuration at {}: {other}", path.display()),
    }
}

/// pydantic's error `type` for the messages the config can produce.
fn error_type(msg: &str) -> &'static str {
    let table: [(&str, &str); 14] = [
        ("Input should be '", "literal_error"),
        ("Input should be a valid integer", "int_parsing"),
        ("Input should be a valid number", "float_parsing"),
        ("Input should be a valid boolean", "bool_parsing"),
        ("Input should be a valid string", "string_type"),
        ("Input should be a valid list", "list_type"),
        ("Input should be a valid dictionary", "dict_type"),
        (
            "Input should be greater than or equal",
            "greater_than_equal",
        ),
        ("Input should be greater than", "greater_than"),
        ("Input should be less than or equal", "less_than_equal"),
        ("Input should be less than", "less_than"),
        ("Field required", "missing"),
        ("String should have at least", "string_too_short"),
        ("Input should be a valid", "model_type"),
    ];
    table
        .iter()
        .find(|(p, _)| msg.starts_with(p))
        .map_or("value_error", |(_, t)| t)
}

pub fn load_config(rt: &Rt) -> std::result::Result<AppConfig, VvError> {
    let path = &rt.paths.config_file;
    if !path.exists() {
        return Ok(AppConfig::default());
    }
    let text = std::fs::read_to_string(path).map_err(|e| {
        VvError::configuration(format!(
            "Invalid configuration at {}: {}",
            path.display(),
            os_error_text(&e, path)
        ))
    })?;
    AppConfig::from_toml_str(&text).map_err(|e| VvError::configuration(config_error_text(path, &e)))
}

pub fn save_config(
    rt: &Rt,
    config: &AppConfig,
    overwrite: bool,
) -> std::result::Result<PathBuf, VvError> {
    let path = rt.paths.config_file.clone();
    if path.exists() && !overwrite {
        return Err(VvError::configuration(format!(
            "Configuration already exists: {}",
            path.display()
        )));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(VvError::from)?;
    }
    let mut temporary = path.clone().into_os_string();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    std::fs::write(&temporary, config.to_toml_string()).map_err(VvError::from)?;
    std::fs::rename(&temporary, &path).map_err(VvError::from)?;
    Ok(path)
}

/// `_fail(exc)` of `cli.py`: the message in red on the standard output, the error's own exit code.
pub fn fail(rt: &Rt, error: &VvError) -> i32 {
    out(&rt
        .term
        .out()
        .print(&format!("[red]{}[/red]", error.message)));
    error.exit_code() as i32
}

/// `_fail(exc, as_json)` of `cli_run.py`: JSON on the standard output, or the message in red on the error stream.
pub fn fail_run(rt: &Rt, error: &VvError, as_json: bool) -> i32 {
    let code = error.exit_code() as i32;
    if as_json {
        echo(&dumps(
            &json!({"ok": false, "error": error.message, "exit_code": code}),
        ));
    } else {
        err(&rt
            .term
            .err()
            .print(&format!("[red]{}[/red]", error.message)));
    }
    code
}

fn general(message: impl Into<String>) -> VvError {
    VvError::general(message)
}

fn table_r(table: Table) -> R {
    R::Table(Box::new(table))
}

fn print_table(console: &Console, table: Table) -> String {
    console.print_r(&table_r(table))
}

/// A cell of `Table.add_row(str)`: markup is read, text wraps with the column's own overflow (ellipsis).
fn row(table: &mut Table, cells: &[&str]) {
    table.add_row(
        cells
            .iter()
            .map(|c| R::Text {
                cells: rich::markup(c, Style::NONE),
                overflow: Overflow::Ellipsis,
                no_wrap: false,
            })
            .collect(),
    );
}

// ------------------------------------------------------------------------------------------------------------ dispatch

pub fn dispatch(rt: &Rt, root: &ClapCommand, command: Command) -> Result<i32> {
    let missing = |path: &str| Usage {
        path: path.to_string(),
        message: "Missing command.".to_string(),
    };
    let _ = root;
    Ok(match command {
        Command::Playlists(a) => crate::runcmds::playlists(rt, a),
        Command::Explain(a) => explain(rt, a),
        Command::Replay(a) => replay(rt, a),
        Command::Parse(a) => parse(rt, a),
        Command::Doctor(a) => crate::legacy::doctor(rt, a),
        Command::Import(a) => return crate::legacy_import::import(rt, a),
        Command::Presentations(a) => crate::runcmds::presentations(rt, a),
        Command::Run(a) => crate::runcmds::run(rt, a),
        Command::Undo(a) => crate::runcmds::undo(rt, a),
        Command::Serve(a) => crate::serve::serve(rt, a),
        Command::History(a) => history(rt, a),
        Command::Node(a) => crate::runcmds::node(rt, a),
        Command::Config { command, .. } => match command {
            None => return Err(missing("versevision config")),
            Some(ConfigCommand::Init { force, .. }) => config_init(rt, force),
            Some(ConfigCommand::Show(a)) => config_show(rt, a),
            Some(ConfigCommand::Options { section, .. }) => config_options(rt, section),
            Some(ConfigCommand::Set { key, value, .. }) => config_set(rt, key, value),
        },
        Command::Themes(a) => crate::themes::themes(rt, a),
        Command::Propresenter { command, .. } => match command {
            None => return Err(missing("versevision propresenter")),
            Some(PropresenterCommand::Libraries(a)) => {
                crate::legacy::propresenter_list(rt, "libraries", a)
            }
            Some(PropresenterCommand::Presentations(a)) => {
                crate::legacy::propresenter_list(rt, "presentations", a)
            }
            Some(PropresenterCommand::Playlists(a)) => {
                crate::legacy::propresenter_list(rt, "playlists", a)
            }
        },
        Command::Versions { command, .. } => match command {
            None => return Err(missing("versevision versions")),
            Some(VersionsCommand::List(a)) => versions_list(rt, a),
        },
        Command::Providers { command, .. } => match command {
            None => return Err(missing("versevision providers")),
            Some(ProvidersCommand::Test(a)) => crate::runcmds::providers_test(rt, a),
        },
        Command::Cache { command, .. } => match command {
            None => return Err(missing("versevision cache")),
            Some(CacheCommand::Inspect(a)) => cache_inspect(rt, a),
            Some(CacheCommand::Clear { .. }) => cache_clear(rt),
        },
        Command::Template { command, .. } => match command {
            None => return Err(missing("versevision template")),
            Some(c) => return crate::legacy::template(rt, c),
        },
        Command::Log { command, .. } => match command {
            None => log_show(rt, None, false, false, false),
            Some(LogCommand::Show(a)) => log_show(rt, a.run_id, a.events, a.problems, a.path),
            Some(LogCommand::List { count, output, .. }) => log_list(rt, count, &output),
        },
        Command::Autostart { command, .. } => match command {
            None => return Err(missing("versevision autostart")),
            Some(AutostartCommand::Status(a)) => autostart_status(rt, a),
            Some(AutostartCommand::Enable { host, port, .. }) => autostart_enable(rt, &host, port),
            Some(AutostartCommand::Disable { .. }) => autostart_disable(rt),
        },
        Command::Keys { command, .. } => match command {
            None => return Err(missing("versevision keys")),
            Some(c) => crate::keys_cmd::keys(rt, c),
        },
    })
}

// -------------------------------------------------------------------------------------------------------------- config

fn config_init(rt: &Rt, force: bool) -> i32 {
    match save_config(rt, &AppConfig::default(), force) {
        Ok(path) => {
            out(&rt
                .term
                .out()
                .print(&format!("Configuration written: {}", path.display())));
            0
        }
        Err(e) => fail(rt, &e),
    }
}

fn config_show(rt: &Rt, a: OutputOnly) -> i32 {
    let config = match load_config(rt) {
        Ok(c) => c,
        Err(e) => return fail(rt, &e),
    };
    if is_json(&a.output) {
        echo(&dumps_utf8(&config.to_value()));
    } else {
        let console = rt.term.out();
        out(&console.print(&format!(
            "Configuration: {}",
            rt.paths.config_file.display()
        )));
        let pretty = rich::pretty_of(&config.to_value());
        let mut text = String::new();
        for line in pretty.lines(console.width) {
            for wrapped in rich::wrap(
                &rich::cells_of(&line, Style::NONE),
                console.width,
                Overflow::Fold,
                false,
            ) {
                text.push_str(&console.line_text(&wrapped));
                text.push('\n');
            }
        }
        out(&text);
    }
    0
}

/// How Python spells a field's type in `config options` (`str(annotation)` with the noise cut).
fn choices_of(ty: &schema::Ty) -> String {
    use schema::Ty;
    match ty {
        Ty::Bool => "bool".into(),
        Ty::Int { .. } => "int".into(),
        Ty::Float { .. } => "float".into(),
        Ty::Str => "str".into(),
        Ty::OptStr => "str | None".into(),
        Ty::Path => "Path".into(),
        Ty::Literal(values) => values.join(", "),
        Ty::StrList => "list[str]".into(),
        Ty::StrMap => "dict[str, str]".into(),
        _ => "dict".into(),
    }
}

fn config_options(rt: &Rt, section: Option<String>) -> i32 {
    let config = match load_config(rt) {
        Ok(c) => c,
        Err(e) => return fail(rt, &e),
    };
    let value = config.to_value();
    let mut table = Table::new(&["Option", "Value", "Choices / meaning"]);
    let models: [(&str, &schema::Model); 4] = [
        ("notes", &schema::NOTES),
        ("points", &schema::POINTS),
        ("verses", &schema::VERSES),
        ("slides", &schema::SLIDES),
    ];
    let section = section.unwrap_or_default();
    for name in ["notes", "points", "verses", "slides"] {
        if !section.is_empty() && section != name {
            continue;
        }
        let model = models
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, m)| *m)
            .expect("a known section");
        for field in model.fields {
            let current = value
                .get(name)
                .and_then(|s| s.get(field.name))
                .cloned()
                .unwrap_or(Value::Null);
            row(
                &mut table,
                &[
                    &format!("{name}.{}", field.name),
                    &versevision_pp::pyrepr::py_str(&current),
                    &choices_of(&field.ty),
                ],
            );
        }
    }
    let console = rt.term.out();
    let mut text = print_table(&console, table);
    text.push_str(&console.print(
        "Change one for a single run: versevision run FILE --set notes.line_breaks=paragraph\n\
         Change it for good:           versevision config set notes.line_breaks paragraph\n\
         Named sets for a kind of document: \\[profiles.NAME.notes] in the config, then --profile NAME",
    ));
    out(&text);
    0
}

fn config_set(rt: &Rt, key: String, value: String) -> i32 {
    let config = match load_config(rt) {
        Ok(c) => c,
        Err(e) => return fail(rt, &e),
    };
    let updated = match config.with_options(None, &[&format!("{key}={value}")]) {
        Ok(u) => u,
        Err(e) => return fail(rt, &VvError::configuration(e.to_string())),
    };
    match save_config(rt, &updated, true) {
        Ok(_) => {
            out(&rt.term.out().print(&format!(
                "{key} = {value}   (saved to {})",
                rt.paths.config_file.display()
            )));
            0
        }
        Err(e) => fail(rt, &e),
    }
}

// ----------------------------------------------------------------------------------------------------- versions, parse

fn versions_list(rt: &Rt, a: OutputOnly) -> i32 {
    let versions = versevision_core::versions::known_versions();
    if is_json(&a.output) {
        let items: Vec<Value> = versions
            .iter()
            .map(|v| serde_json::to_value(v).unwrap_or(Value::Null))
            .collect();
        echo(&dumps(&Value::Array(items)));
    } else {
        let mut table = Table::new(&["Abbreviation", "Name", "Provider"]);
        for v in &versions {
            row(
                &mut table,
                &[
                    &v.abbreviation,
                    &v.name,
                    v.provider
                        .as_deref()
                        .filter(|p| !p.is_empty())
                        .unwrap_or("Not configured"),
                ],
            );
        }
        out(&print_table(&rt.term.out(), table));
    }
    0
}

/// `_read_source`: `Err` is the message and exit code `_fail` ends with.
/// What `read_source` returns: the text, its name and the `extract` trace record of a document.
pub type Source = (String, String, Option<Map<String, Value>>);

pub fn read_source(
    file: &Option<String>,
    text: &Option<String>,
) -> std::result::Result<Source, VvError> {
    if file.is_none() == text.is_none() {
        return Err(general("provide exactly one FILE or --text"));
    }
    if let Some(t) = text {
        return Ok((t.clone(), "text".into(), None));
    }
    let file = file.clone().unwrap_or_default();
    if file == "-" {
        use std::io::Read;
        let mut buf = Vec::new();
        std::io::stdin()
            .read_to_end(&mut buf)
            .map_err(|e| general(e.to_string()))?;
        let text = String::from_utf8(buf).map_err(|e| general(e.to_string()))?;
        return Ok((text, "-".into(), None));
    }
    let path = Path::new(&file);
    let bytes = std::fs::read(path).map_err(|e| general(os_error_text(&e, path)))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| file.clone());
    match versevision_docs::extract_text(&bytes, &name) {
        Ok((text, info)) => {
            let extract = match info.to_json() {
                Value::Object(map) => Some(map),
                _ => None,
            };
            Ok((text, name, extract))
        }
        Err(e) => Err(general(e.to_string())),
    }
}

fn parse(rt: &Rt, a: ParseArgs) -> i32 {
    let (text, _, _) = match read_source(&a.file, &a.text) {
        Ok(t) => t,
        Err(e) => return fail(rt, &e),
    };
    let result = match versevision_core::references::parse_references(&text, None) {
        Ok(r) => r,
        Err(e) => return fail(rt, &general(e.to_string())),
    };
    let issues = result["issues"].as_array().cloned().unwrap_or_default();
    if is_json(&a.output) {
        echo(&dumps_utf8(&result));
    } else if a.output.to_lowercase() == "terminal" {
        let mut table = Table::new(&["#", "Source", "Normalized", "Version", "Status"]);
        for (index, reference) in result["references"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            let version = reference["version"]
                .as_str()
                .filter(|v| !v.is_empty())
                .unwrap_or("—");
            row(
                &mut table,
                &[
                    &(index + 1).to_string(),
                    reference["source_text"].as_str().unwrap_or(""),
                    reference["normalized_text"].as_str().unwrap_or(""),
                    version,
                    "Valid",
                ],
            );
        }
        for issue in &issues {
            row(
                &mut table,
                &[
                    "—",
                    issue["source_text"].as_str().unwrap_or(""),
                    "—",
                    "—",
                    &format!("Invalid: {}", issue["message"].as_str().unwrap_or("")),
                ],
            );
        }
        out(&print_table(&rt.term.out(), table));
    } else {
        return fail(rt, &general("--output must be terminal or json"));
    }
    if issues.is_empty() {
        0
    } else {
        ExitCode::ParsingOrValidation as i32
    }
}

// ---------------------------------------------------------------------------------------------------------------- cache

fn cache_inspect(rt: &Rt, a: OutputOnly) -> i32 {
    let cache = versevision_bible::ScriptureCache::new(rt.paths.cache_dir.join("scripture"));
    let path = cache.dir().display().to_string();
    let count = cache.count();
    if is_json(&a.output) {
        echo(&dumps(&json!({"path": path, "verse_count": count})));
    } else {
        echo(&format!("{count} cached verse(s) at {path}"));
    }
    0
}

fn cache_clear(rt: &Rt) -> i32 {
    let cache = versevision_bible::ScriptureCache::new(rt.paths.cache_dir.join("scripture"));
    let count = cache.clear();
    out(&rt
        .term
        .out()
        .print(&format!("Cleared {count} cached verse(s).")));
    0
}

// -------------------------------------------------------------------------------------------------------------- history

pub fn clock() -> Arc<dyn versevision_pp::clock::Clock> {
    Arc::new(versevision_pp::clock::SystemClock::new())
}

fn history(rt: &Rt, a: OutputOnly) -> i32 {
    let store = versevision_pp::history::HistoryStore::new(rt.paths.history_file.clone(), clock());
    let records = store.load();
    if is_json(&a.output) {
        echo(&dumps(&versevision_pp::history::records_json(&records)));
    } else {
        let mut table = Table::new(&["Id", "When (UTC)", "Source", "Target", "Slides", "Status"]);
        for r in &records {
            row(
                &mut table,
                &[
                    &r.import_id,
                    &r.created_display(),
                    &r.source_name,
                    &r.target_name,
                    &r.slide_count.to_string(),
                    &r.status,
                ],
            );
        }
        out(&print_table(&rt.term.out(), table));
    }
    0
}

// ------------------------------------------------------------------------------------------------------------------ log

fn log_show(rt: &Rt, run_id: Option<String>, events: bool, problems: bool, path: bool) -> i32 {
    let runs = rt.paths.runs_dir();
    let Some(found) = versevision_pp::runlog::resolve(&runs, run_id.as_deref().unwrap_or("latest"))
    else {
        err(&rt
            .term
            .err()
            .print("[red]No such run.[/red] Try: versevision log list"));
        return 1;
    };
    let folder = runs.join(&found);
    if path {
        echo(&folder.display().to_string());
        return 0;
    }
    if events || problems {
        for e in versevision_pp::runlog::read_events(&runs, &found) {
            let level = e["level"].as_str().unwrap_or("");
            if problems && level != "warn" && level != "error" {
                continue;
            }
            let mut extra = Map::new();
            if let Value::Object(map) = &e {
                for (k, v) in map {
                    if !["t", "elapsed", "step", "level"].contains(&k.as_str()) {
                        extra.insert(k.clone(), v.clone());
                    }
                }
            }
            let dumped: String = pyjson::dumps(&Value::Object(extra), None, false)
                .chars()
                .take(300)
                .collect();
            let elapsed = versevision_pp::pyrepr::py_str(&e["elapsed"]);
            let elapsed = format!("{elapsed:>7}");
            echo(&format!(
                "[{elapsed}s] {:<5} {:<16} {}",
                level.to_uppercase(),
                e["step"].as_str().unwrap_or(""),
                dumped
            ));
        }
        return 0;
    }
    let summary = folder.join("summary.txt");
    if let Ok(text) = std::fs::read_to_string(&summary) {
        echo(&text);
    } else {
        let lines: Vec<String> = versevision_pp::runlog::read_events(&runs, &found)
            .iter()
            .map(|e| {
                format!(
                    "[{}s] {} {}",
                    versevision_pp::pyrepr::py_str(&e["elapsed"]),
                    e["step"].as_str().unwrap_or(""),
                    e.get("message")
                        .map(versevision_pp::pyrepr::py_str)
                        .unwrap_or_default()
                )
            })
            .collect();
        echo(&format!(
            "No summary yet (the run may still be going, or it was cut off). Showing the steps:\n{}",
            lines.join("\n")
        ));
    }
    0
}

fn log_list(rt: &Rt, count: i64, output: &str) -> i32 {
    let runs = rt.paths.runs_dir();
    let rows = versevision_pp::runlog::list_runs(&runs, count.max(0) as usize);
    if is_json(output) {
        echo(&dumps(&Value::Array(rows)));
        return 0;
    }
    let mut table = Table::new(&["Run", "Result", "Action", "Document"]);
    for r in &rows {
        let source: String = r["source"]
            .as_str()
            .unwrap_or("")
            .chars()
            .take(50)
            .collect();
        row(
            &mut table,
            &[
                r["id"].as_str().unwrap_or(""),
                r["outcome"].as_str().unwrap_or(""),
                r["mode"].as_str().unwrap_or(""),
                &source,
            ],
        );
    }
    let console = rt.term.err();
    let mut text = print_table(&console, table);
    text.push_str(&console.print(&format!("[dim]{}[/dim]", runs.display())));
    err(&text);
    0
}

// ------------------------------------------------------------------------------------------------- explain and replay

fn explain(rt: &Rt, a: ExplainArgs) -> i32 {
    let runs = rt.paths.runs_dir();
    let run_id = a.run_id.clone().unwrap_or_else(|| "latest".into());
    let result = if is_json(&a.output) {
        versevision_pp::explain::explain_data(&runs, &run_id, a.line, a.tier.as_deref())
            .map(|d| dumps_utf8(&d))
    } else {
        versevision_pp::explain::render_explain(&runs, &run_id, a.line, a.tier.as_deref(), 150)
    };
    match result {
        Ok(text) => {
            echo(&text);
            0
        }
        Err(e) if e.python_name() == "FileNotFoundError" => {
            echo_err(&e.message);
            1
        }
        Err(e) => fail(rt, &e),
    }
}

fn replay(rt: &Rt, a: ReplayArgs) -> i32 {
    let runs = rt.paths.runs_dir();
    let config = match load_config(rt) {
        Ok(c) => c,
        Err(e) => return fail(rt, &e),
    };
    let ids: Vec<String> = if a.recent > 0 {
        versevision_pp::runlog::list_runs(&runs, a.recent as usize)
            .iter()
            .map(|r| r["id"].as_str().unwrap_or("").to_string())
            .collect()
    } else {
        vec![a.run_id.clone().unwrap_or_else(|| "latest".into())]
    };
    let mut failed = false;
    for rid in &ids {
        match versevision_pp::explain::replay_data(&runs, &config, rid, !a.todays_options) {
            Ok(data) => echo(&versevision_pp::explain::render_replay(&data)),
            Err(e) if e.python_name() == "FileNotFoundError" => {
                echo(&format!("{rid}: {}", e.message));
                failed = true;
            }
            Err(e) => return fail(rt, &e),
        }
    }
    if failed && a.recent <= 0 {
        1
    } else {
        0
    }
}

// ---------------------------------------------------------------------------------------------------------- autostart

fn autostart_setup<'a>(
    rt: &Rt,
    runner: &'a dyn crate::autostart::Runner,
) -> crate::autostart::Setup<'a> {
    autostart_setup_for(&rt.paths, runner)
}

/// What autostart needs from this machine (also what the web page's start-at-login switch uses).
pub(crate) fn autostart_setup_for<'a>(
    paths: &crate::paths::Paths,
    runner: &'a dyn crate::autostart::Runner,
) -> crate::autostart::Setup<'a> {
    crate::autostart::Setup {
        platform: crate::autostart::Platform::current(),
        home: paths.home.clone(),
        xdg_config_home: std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from),
        os_logs: paths.os_logs.clone(),
        program: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("versevision")),
        uid: current_uid(),
        runner,
    }
}

#[cfg(unix)]
fn current_uid() -> u32 {
    // `id -u` keeps this free of `unsafe`
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
        .unwrap_or(0)
}

#[cfg(not(unix))]
fn current_uid() -> u32 {
    0
}

fn autostart_status(rt: &Rt, a: OutputOnly) -> i32 {
    let runner = crate::autostart::SystemRunner;
    let state = autostart_setup(rt, &runner).status();
    if is_json(&a.output) {
        echo(&dumps_utf8(&state.to_value()));
    } else {
        let mut line = if state.enabled {
            "[green]On[/green]".to_string()
        } else {
            "Off".to_string()
        };
        line.push_str(&format!(" ({})", state.platform));
        if state.enabled {
            if let Some(port) = state.port.filter(|p| *p != 0) {
                line.push_str(&format!(
                    ", http://{}:{port}/",
                    state.host.clone().unwrap_or_else(|| "None".into())
                ));
            }
        }
        out(&rt.term.out().print(&line));
    }
    0
}

fn autostart_enable(rt: &Rt, host: &str, port: i64) -> i32 {
    let runner = crate::autostart::SystemRunner;
    match autostart_setup(rt, &runner).enable(host, port, true) {
        Ok(state) => {
            let console = rt.term.err();
            let mut text = console.print(&format!("[green]On[/green]: VerseVision will start when you log in at http://{host}:{port}/"));
            if let Some(file) = state.file.filter(|f| !f.is_empty()) {
                text.push_str(&console.print(&format!("Service file: {file}")));
            }
            err(&text);
            0
        }
        Err(message) => fail_run(rt, &VvError::configuration(message), false),
    }
}

fn autostart_disable(rt: &Rt) -> i32 {
    let runner = crate::autostart::SystemRunner;
    match autostart_setup(rt, &runner).disable(false) {
        Ok(_) => {
            err(&rt
                .term
                .err()
                .print("Off: VerseVision will no longer start at login."));
            0
        }
        Err(message) => fail_run(rt, &VvError::configuration(message), false),
    }
}

#[allow(dead_code)]
fn unused(_: Column, _: Console) {}
