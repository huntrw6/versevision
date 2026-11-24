//! TOML in and out. Reading uses the `toml` crate (tables keep the file's order); writing is a small writer in the
//! style of `tomli_w` (scalars first, then `[tables]`, a blank line between tables, lists on one line) so a saved file
//! looks like the Python app's.

use serde_json::{Map, Value};

/// Parse TOML text into a JSON value (a TOML date or time becomes its text).
pub fn parse(text: &str) -> Result<Value, String> {
    let table: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
    Ok(from_toml(toml::Value::Table(table)))
}

fn from_toml(value: toml::Value) -> Value {
    match value {
        toml::Value::String(s) => Value::String(s),
        toml::Value::Integer(i) => Value::from(i),
        toml::Value::Float(x) => super::validate::float_value(x),
        toml::Value::Boolean(b) => Value::Bool(b),
        toml::Value::Datetime(d) => Value::String(d.to_string()),
        toml::Value::Array(items) => Value::Array(items.into_iter().map(from_toml).collect()),
        toml::Value::Table(table) => {
            Value::Object(table.into_iter().map(|(k, v)| (k, from_toml(v))).collect())
        }
    }
}

/// Write a JSON object as TOML; `null`s are left out (`exclude_none`).
pub fn write(value: &Value) -> String {
    let mut out = String::new();
    if let Some(root) = value.as_object() {
        write_table(&mut out, &[], root);
    }
    out
}

/// The layout of `tomli_w`: a table's own values first under its `[header]` (the header is left out when the table has
/// only sub-tables), then each sub-table, with a blank line before every table chunk but the very first.
fn write_table(out: &mut String, path: &[String], table: &Map<String, Value>) {
    let literals: Vec<(&String, &Value)> = table
        .iter()
        .filter(|(_, v)| !v.is_object() && !v.is_null())
        .collect();
    let tables: Vec<(&String, &Map<String, Value>)> = table
        .iter()
        .filter_map(|(k, v)| v.as_object().map(|m| (k, m)))
        .collect();
    let mut yielded = false;
    if path.is_empty() {
        for (key, value) in &literals {
            out.push_str(&format!("{} = {}\n", key_text(key), inline(value)));
            yielded = true;
        }
    } else if !literals.is_empty() || tables.is_empty() {
        let header: Vec<String> = path.iter().map(|k| key_text(k)).collect();
        out.push_str(&format!("[{}]\n", header.join(".")));
        for (key, value) in &literals {
            out.push_str(&format!("{} = {}\n", key_text(key), inline(value)));
        }
        yielded = true;
    }
    for (key, child) in tables {
        if yielded {
            out.push('\n');
        }
        yielded = true;
        let mut child_path = path.to_vec();
        child_path.push(key.clone());
        write_table(out, &child_path, child);
    }
}

fn key_text(key: &str) -> String {
    if !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        key.to_string()
    } else {
        string_text(key)
    }
}

fn string_text(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if (c < ' ' && c != '\t') || c == '\u{7f}' => {
                out.push_str(&format!("\\u{:04x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// tomli_w writes every non-empty list one item per line, four spaces in, with a trailing comma.
fn inline_array(items: &[Value], nest: usize) -> String {
    if items.is_empty() {
        return "[]".to_string();
    }
    let indent = "    ".repeat(nest + 1);
    let lines: Vec<String> = items
        .iter()
        .map(|item| match item {
            Value::Array(inner) => format!("{indent}{}", inline_array(inner, nest + 1)),
            other => format!("{indent}{}", inline(other)),
        })
        .collect();
    format!("[\n{},\n{}]", lines.join(",\n"), "    ".repeat(nest))
}

fn inline(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => match n.as_f64() {
            Some(x) if n.is_f64() => float_text(x),
            _ => n.to_string(),
        },
        Value::String(s) => string_text(s),
        Value::Array(items) => inline_array(items, 0),
        Value::Object(map) => {
            let parts: Vec<String> = map
                .iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| format!("{} = {}", key_text(k), inline(v)))
                .collect();
            if parts.is_empty() {
                "{}".to_string()
            } else {
                format!("{{ {} }}", parts.join(", "))
            }
        }
    }
}

/// A float the way Python prints it (`5.0`, `0.15`, `1e-05`, `1.5e+16`); TOML reads all of these as floats.
fn float_text(x: f64) -> String {
    let shown = format!("{x:?}"); // Rust switches to an exponent at the same sizes as Python (1e-4 and 1e16)
    match shown.split_once('e') {
        Some((mantissa, exponent)) => {
            let (sign, digits) = match exponent.strip_prefix('-') {
                Some(d) => ('-', d),
                None => ('+', exponent),
            };
            format!("{mantissa}e{sign}{digits:0>2}")
        }
        None => shown,
    }
}
