//! typer's help page and error box (typer 0.27 `rich_utils.py`), drawn from the clap command tree of `def.rs`.

use clap::{Arg, ArgAction, Command};

use crate::rich::{
    cells_of, escape, markup, text, text_styled, Cells, Column, Console, Overflow, Style, Table, R,
};

/// `inspect.cleandoc`: strip the common indentation of the lines after the first, and blank edges.
pub fn cleandoc(doc: &str) -> String {
    let lines: Vec<&str> = doc.split('\n').collect();
    let indent = lines
        .iter()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let mut out: Vec<String> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if index == 0 {
            out.push(line.trim_start().to_string());
        } else if line.len() >= indent {
            out.push(line[indent.min(line.len())..].trim_end().to_string());
        } else {
            out.push(line.trim().to_string());
        }
    }
    while out.first().is_some_and(|l| l.is_empty()) {
        out.remove(0);
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    out.join("\n")
}

fn command_help(cmd: &Command) -> String {
    cmd.get_long_about()
        .or_else(|| cmd.get_about())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

fn rich_text(markup_text: &str, style: &str, overflow: Overflow) -> R {
    R::Text {
        cells: markup(markup_text, Style::parse(style)),
        overflow,
        no_wrap: false,
    }
}

fn plain_text(t: &str, style: &str, overflow: Overflow) -> R {
    R::Text {
        cells: cells_of(t, Style::parse(style)),
        overflow,
        no_wrap: false,
    }
}

/// `Usage: versevision config set [OPTIONS] {key} {value}`.
pub fn usage(cmd: &Command, path: &str) -> String {
    let mut parts = vec![format!("Usage: {path}"), "[OPTIONS]".to_string()];
    for arg in cmd.get_positionals() {
        let name = arg.get_id().as_str();
        if arg.is_required_set() {
            parts.push(format!("{{{name}}}"));
        } else {
            parts.push(format!("[{name}]"));
        }
    }
    if cmd.get_subcommands().next().is_some() {
        parts.push("COMMAND [ARGS]...".to_string());
    }
    parts.join(" ")
}

struct Row {
    long: String,
    short: String,
    secondary_long: String,
    kind: String,
    help: String,
    default: Option<String>,
    required: bool,
}

fn row_of(arg: &Arg, positional: bool, merged: Option<&Arg>) -> Row {
    let mut long = String::new();
    let mut short = String::new();
    if positional {
        short = arg.get_id().as_str().to_string();
    } else {
        if let Some(l) = arg.get_long() {
            long = format!("--{l}");
        }
        if let Some(s) = arg.get_short() {
            short = format!("-{s}");
        }
    }
    let takes_value = !matches!(
        arg.get_action(),
        ArgAction::SetTrue
            | ArgAction::SetFalse
            | ArgAction::Count
            | ArgAction::Help
            | ArgAction::Version
    );
    let kind = if positional {
        if arg.get_id() == "file" {
            "<path>".to_string()
        } else {
            "<str>".to_string()
        }
    } else if takes_value {
        let name = arg
            .get_value_names()
            .and_then(|v| v.first().map(|s| s.to_string()))
            .unwrap_or_else(|| "str".to_string());
        format!("<{name}>")
    } else {
        String::new()
    };
    let secondary_long = merged
        .and_then(|m| m.get_long())
        .map(|l| format!("--{l}"))
        .unwrap_or_default();
    let default = if merged.is_some() {
        Some("no-open".to_string())
    } else if takes_value {
        arg.get_default_values()
            .first()
            .map(|d| d.to_string_lossy().to_string())
            .filter(|d| !d.is_empty())
    } else {
        None
    };
    Row {
        long,
        short,
        secondary_long,
        kind,
        help: arg.get_help().map(|h| h.to_string()).unwrap_or_default(),
        default,
        required: arg.is_required_set(),
    }
}

fn panel(rows: Vec<Row>, title: &str) -> Option<R> {
    if rows.is_empty() {
        return None;
    }
    let any_required = rows.iter().any(|r| r.required);
    let columns = if any_required { 7 } else { 6 };
    let mut table = Table::plain(columns);
    table.expand = true;
    for row in rows {
        let mut cells: Vec<R> = Vec::new();
        if any_required {
            cells.push(if row.required {
                text_styled("*", "red")
            } else {
                text("")
            });
        }
        cells.push(plain_text(&row.long, "", Overflow::Fold));
        cells.push(plain_text(&row.short, "", Overflow::Fold));
        cells.push(plain_text(&row.secondary_long, "", Overflow::Fold));
        cells.push(plain_text("", "", Overflow::Fold));
        cells.push(plain_text(&row.kind, "bold yellow", Overflow::Fold));
        let mut items: Vec<R> = Vec::new();
        if !row.help.is_empty() {
            let paragraphs: Vec<String> = row
                .help
                .split("\n\n")
                .map(|p| p.replace('\n', " ").trim().to_string())
                .collect();
            items.push(rich_text(paragraphs.join("\n").trim(), "", Overflow::Fold));
        }
        if let Some(default) = &row.default {
            items.push(plain_text(
                &format!("[default: {default}]"),
                "dim",
                Overflow::Fold,
            ));
        }
        if row.required {
            items.push(plain_text("[required]", "dim red", Overflow::Fold));
        }
        cells.push(R::Columns(items));
        table.add_row(cells);
    }
    Some(R::Panel {
        inner: Box::new(R::Table(Box::new(table))),
        title: title.to_string(),
        border: Style::parse("dim"),
    })
}

fn first_paragraph(help: &str) -> String {
    let doc = cleandoc(help);
    doc.split("\n\n").next().unwrap_or("").trim().to_string()
}

/// `rich_format_help`: the whole page for one command.
pub fn render_help(cmd: &Command, path: &str, console: &Console) -> String {
    let mut out = String::new();
    let usage_r = R::Padding(
        Box::new(plain_text(&usage(cmd, path), "bold", Overflow::Fold)),
        [1, 1, 1, 1],
    );
    out.push_str(&console.print_r(&usage_r));
    let help = command_help(cmd);
    if !help.is_empty() {
        let doc = cleandoc(&help);
        let mut paragraphs = doc.split("\n\n");
        let first = paragraphs.next().unwrap_or("").replace('\n', " ");
        let mut group: Vec<R> = vec![rich_text(first.trim(), "", Overflow::Fold)];
        let rest: Vec<&str> = paragraphs.collect();
        if !rest.is_empty() {
            group.push(R::Text {
                cells: Cells::new(),
                overflow: Overflow::Fold,
                no_wrap: false,
            });
            group.push(rich_text(
                &cleandoc(&rest.join("\n\n")),
                "dim",
                Overflow::Fold,
            ));
        }
        let block = R::Padding(Box::new(R::Align(Box::new(R::Group(group)))), [0, 1, 1, 1]);
        out.push_str(&console.print_r(&block));
    }
    let mut argument_rows = Vec::new();
    let mut option_rows = Vec::new();
    for arg in cmd.get_arguments() {
        if arg.is_hide_set() {
            continue;
        }
        if arg.is_positional() {
            argument_rows.push(row_of(arg, true, None));
        } else if arg.get_id() == "no_open" {
            continue;
        } else if arg.get_id() == "open" {
            let other = cmd.get_arguments().find(|a| a.get_id() == "no_open");
            option_rows.push(row_of(arg, false, other));
        } else {
            option_rows.push(row_of(arg, false, None));
        }
    }
    if let Some(p) = panel(argument_rows, "Arguments") {
        out.push_str(&console.print_r(&p));
    }
    if let Some(p) = panel(option_rows, "Options") {
        out.push_str(&console.print_r(&p));
    }
    let subs: Vec<&Command> = cmd.get_subcommands().filter(|c| !c.is_hide_set()).collect();
    if !subs.is_empty() {
        let cmd_len = subs.iter().map(|c| c.get_name().len()).max().unwrap_or(0);
        let mut table = Table::plain(2);
        table.expand = true;
        table.columns[0] = Column {
            style: Style::parse("bold cyan"),
            no_wrap: true,
            width: Some(cmd_len),
            ..Column::new("")
        };
        table.columns[1] = Column {
            ratio: Some(10),
            ..Column::new("Description")
        };
        for sub in subs {
            let helptext = first_paragraph(&command_help(sub));
            table.add_row(vec![
                plain_text(sub.get_name(), "", Overflow::Ellipsis),
                rich_text(&helptext, "", Overflow::Ellipsis),
            ]);
        }
        let p = R::Panel {
            inner: Box::new(R::Table(Box::new(table))),
            title: "Commands".to_string(),
            border: Style::parse("dim"),
        };
        out.push_str(&console.print_r(&p));
    }
    out
}

/// `rich_format_error`: usage, the hint and the red box with the message.
pub fn render_error(cmd: &Command, path: &str, message: &str, console: &Console) -> String {
    let mut out = String::new();
    out.push_str(&console.print_r(&plain_text(&usage(cmd, path), "bold", Overflow::Fold)));
    out.push_str(&console.print_r(&R::Text {
        cells: markup(
            &format!("Try [blue]'{} --help'[/] for help.", escape(path)),
            Style::parse("dim"),
        ),
        overflow: Overflow::Fold,
        no_wrap: false,
    }));
    out.push_str(&console.print_r(&R::Panel {
        inner: Box::new(plain_text(message, "", Overflow::Fold)),
        title: "Error".to_string(),
        border: Style::parse("red"),
    }));
    out
}
