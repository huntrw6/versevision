//! What `run` prints about a plan (`_print_report` of `cli_run.py`), as a function of the report so it can be checked against Python's
//! output for every report of the S6b scenarios (`oracle/cli/render.jsonl`).

use serde_json::Value;
use versevision_pp::pyrepr::py_repr;

use crate::rich::{markup, Console, Overflow, Style, Table, R};

fn s(v: &Value) -> String {
    match v {
        Value::String(t) => t.clone(),
        Value::Null => "None".into(),
        other => versevision_pp::pyrepr::py_str(other),
    }
}

fn chars_prefix(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

fn count(summary: &Value, key: &str) -> String {
    match summary.get(key) {
        Some(v) if !v.is_null() => versevision_pp::pyrepr::py_str(v),
        _ => "0".into(),
    }
}

/// `_print_report(report)` as the text written to the stream.
pub fn print_report(console: &Console, report: &Value) -> String {
    let summary = &report["summary"];
    let mut table = Table::new(&["#", "Type", "Slide"]);
    for slide in report["slides"].as_array().cloned().unwrap_or_default() {
        let kind = s(&slide["kind"]);
        let mut label = s(&slide["title"]);
        if kind != "point" {
            label.push_str(&format!(": {}", chars_prefix(&s(&slide["text"]), 50)));
        }
        let cells = [s(&slide["n"]), kind, label];
        table.add_row(
            cells
                .iter()
                .map(|c| R::Text {
                    cells: markup(c, Style::NONE),
                    overflow: Overflow::Ellipsis,
                    no_wrap: false,
                })
                .collect(),
        );
    }
    let mut out = console.print_r(&R::Table(Box::new(table)));
    out.push_str(&console.print(&format!(
        "{} slides ({} verse, {} point)",
        s(&summary["slides"]),
        s(&summary["verse_slides"]),
        s(&summary["point_slides"])
    )));
    let empty = Value::Object(Default::default());
    let review = match report.get("review") {
        Some(v) if v.as_object().is_some_and(|m| !m.is_empty()) => v,
        _ => &empty,
    };
    out.push_str(&console.print(&format!(
        "[bold]Review, least certain first:[/bold] [red]{} errors[/red], [yellow]{} warnings[/yellow], [green]{} good[/green]",
        count(summary, "errors"),
        count(summary, "warnings"),
        count(summary, "good")
    )));
    let list = |key: &str| {
        review
            .get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let mut warnings = list("warning");
    warnings.extend(list("left_out_warning"));
    for (title, rows, style) in [
        (
            "ERRORS (could not be decided; nothing is added for these)",
            list("error"),
            "red",
        ),
        (
            "WARNINGS (probably right, but a rule was a guess)",
            warnings,
            "yellow",
        ),
    ] {
        if rows.is_empty() {
            continue;
        }
        out.push_str(&console.print(&format!("[{style}]{title}: {}[/{style}]", rows.len())));
        let mut sorted = rows.clone();
        sorted.sort_by_key(|d| d["line"].as_i64().unwrap_or(0));
        for d in sorted {
            let what = if d["kind"] == "left_out" {
                "left out".to_string()
            } else {
                s(&d["kind"])
            };
            out.push_str(&console.print(&format!(
                "  [{style}]line {} [{what}] {}[/{style}]",
                s(&d["line"]),
                py_repr(&chars_prefix(&s(&d["title"]), 62))
            )));
            for reason in d["reasons"].as_array().cloned().unwrap_or_default() {
                out.push_str(&console.print(&format!("      [{style}]- {}[/{style}]", s(&reason))));
            }
        }
    }
    let good = list("good");
    if !good.is_empty() {
        out.push_str(&console.print(&format!(
            "[green]GOOD ({}): parsed with no guesses[/green]",
            good.len()
        )));
        for d in good {
            out.push_str(&console.print(&format!(
                "  [green]line {} [{}] {}[/green]",
                s(&d["line"]),
                s(&d["kind"]),
                py_repr(&chars_prefix(&s(&d["title"]), 70))
            )));
        }
    }
    let quiet = list("left_out_good");
    if !quiet.is_empty() {
        let parts: Vec<String> = quiet
            .iter()
            .map(|d| {
                format!(
                    "line {} {}",
                    s(&d["line"]),
                    py_repr(&chars_prefix(&s(&d["title"]), 28))
                )
            })
            .collect();
        out.push_str(&console.print(&format!(
            "[dim]Left out, clearly not slides ({}): {}[/dim]",
            quiet.len(),
            parts.join("; ")
        )));
    }
    out
}
