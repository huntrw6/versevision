//! The few things of Python's text formatting the run log and `explain` use: `repr(str)`, `str(value)` and width padding by characters.

use serde_json::Value;

use crate::pyjson::{dumps, float_repr};

/// Does `str.isprintable` say no? (Unicode categories Cc, Cf, Zs other than the space, Zl, Zp, Co; the unassigned ones (Cn) are not
/// known here, so such a character is printed as itself: **a difference from Python for unassigned code points only**.)
fn is_unprintable(c: char) -> bool {
    let n = c as u32;
    if c == ' ' {
        return false;
    }
    c.is_control()
        || matches!(n,
            0x00A0 | 0x00AD | 0x0600..=0x0605 | 0x061C | 0x06DD | 0x070F | 0x0890..=0x0891 | 0x08E2 | 0x1680 | 0x180E
            | 0x2000..=0x200F | 0x2028..=0x202F | 0x205F..=0x2064 | 0x2066..=0x206F | 0x3000 | 0xFEFF | 0xFFF9..=0xFFFB
            | 0xE000..=0xF8FF | 0xE0001 | 0xE0020..=0xE007F | 0xF0000..=0xFFFFD | 0x100000..=0x10FFFD)
}

/// `repr(text)`.
pub fn py_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::new();
    out.push(quote);
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if is_unprintable(c) => {
                let n = c as u32;
                if n < 0x100 {
                    out.push_str(&format!("\\x{n:02x}"));
                } else if n < 0x10000 {
                    out.push_str(&format!("\\u{n:04x}"));
                } else {
                    out.push_str(&format!("\\U{n:08x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// `str(value)` of a JSON value read by Python (`None`, `True`, `3`, `1.5`, the text itself, `repr` of a list or dict).
pub fn py_str(value: &Value) -> String {
    match value {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::String(s) => s.clone(),
        Value::Number(n) => {
            if n.is_f64() {
                float_repr(n.as_f64().unwrap_or(0.0))
            } else {
                n.to_string()
            }
        }
        other => py_container_repr(other),
    }
}

fn py_container_repr(value: &Value) -> String {
    match value {
        Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(py_item_repr)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(k, v)| format!("{}: {}", py_repr(k), py_item_repr(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        other => py_item_repr(other),
    }
}

/// `repr(value)` of a JSON value read by Python.
pub fn py_repr_value(value: &Value) -> String {
    py_item_repr(value)
}

fn py_item_repr(value: &Value) -> String {
    match value {
        Value::String(s) => py_repr(s),
        Value::Array(_) | Value::Object(_) => py_container_repr(value),
        other => py_str(other),
    }
}

/// `text[start:end]` by characters.
pub fn slice_chars(text: &str, start: usize, end: usize) -> String {
    text.chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

/// `f"{text:<width}"`.
pub fn pad_right(text: &str, width: usize) -> String {
    let n = text.chars().count();
    format!("{text}{}", " ".repeat(width.saturating_sub(n)))
}

/// `f"{text:>width}"`.
pub fn pad_left(text: &str, width: usize) -> String {
    let n = text.chars().count();
    format!("{}{text}", " ".repeat(width.saturating_sub(n)))
}

/// `json.dumps(value, ensure_ascii=False)` of a value Python prints as JSON.
pub fn json_text(value: &Value) -> String {
    dumps(value, None, false)
}

/// Python's `==` on values read from JSON (`1 == 1.0`, `True == 1`).
pub fn py_eq(a: &Value, b: &Value) -> bool {
    fn num(v: &Value) -> Option<f64> {
        match v {
            Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            Value::Number(n) => n.as_f64(),
            _ => None,
        }
    }
    match (num(a), num(b)) {
        (Some(x), Some(y)) => return x == y,
        (Some(_), None) | (None, Some(_)) => return false,
        _ => {}
    }
    match (a, b) {
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| py_eq(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| py_eq(v, w)))
        }
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn repr_picks_quotes_and_escapes_like_python() {
        assert_eq!(py_repr("it's"), "\"it's\"");
        assert_eq!(py_repr("say \"hi\" it's"), "'say \"hi\" it\\'s'");
        assert_eq!(py_repr("a\tb\n\u{a0}é"), "'a\\tb\\n\\xa0é'");
    }

    #[test]
    fn str_of_values() {
        assert_eq!(py_str(&json!(null)), "None");
        assert_eq!(py_str(&json!(true)), "True");
        assert_eq!(py_str(&json!(2.0)), "2.0");
        assert_eq!(py_str(&json!(["a", 1])), "['a', 1]");
        assert_eq!(py_str(&json!({"k": null})), "{'k': None}");
    }
}
