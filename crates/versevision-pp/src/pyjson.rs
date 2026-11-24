//! Python's `json.dumps` for the files the Python app writes and Python reads back (history, run log, report): the same bytes.
//!
//! * `indent=None`: separators `", "` and `": "`; `indent=n`: `","` + newline and `": "`, empty containers as `[]` / `{}`.
//! * `ensure_ascii`: every character above U+007E... above U+007F is `\uXXXX` (lower-case hex, surrogate pairs above U+FFFF).
//!   (Python escapes only non-ASCII: U+007F itself stays.)
//! * floats the way `repr(float)` writes them (`1e-05`, `1e+16`, `1.0`).

use serde_json::Value;

pub fn dumps(value: &Value, indent: Option<usize>, ensure_ascii: bool) -> String {
    let mut out = String::new();
    write_value(&mut out, value, indent, 0, ensure_ascii);
    out
}

/// `repr(float)`.
pub fn float_repr(x: f64) -> String {
    if x.is_nan() {
        return "NaN".to_string();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    if x == 0.0 {
        return if x.is_sign_negative() { "-0.0" } else { "0.0" }.to_string();
    }
    // shortest digits that round-trip, from Rust's own `{:e}` (digits + exponent)
    let sci = format!("{x:e}");
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let negative = mantissa.starts_with('-');
    let digits: String = mantissa.chars().filter(|c| c.is_ascii_digit()).collect();
    let sign = if negative { "-" } else { "" };
    if (-4..16).contains(&exp) {
        // positional
        let point = exp + 1; // digits before the decimal point
        let body = if point <= 0 {
            format!("0.{}{}", "0".repeat((-point) as usize), digits)
        } else if (point as usize) >= digits.len() {
            format!("{}{}.0", digits, "0".repeat(point as usize - digits.len()))
        } else {
            format!(
                "{}.{}",
                &digits[..point as usize],
                &digits[point as usize..]
            )
        };
        format!("{sign}{body}")
    } else {
        let mut m = String::new();
        m.push_str(&digits[..1]);
        if digits.len() > 1 {
            m.push('.');
            m.push_str(&digits[1..]);
        }
        format!(
            "{sign}{m}e{}{:02}",
            if exp < 0 { '-' } else { '+' },
            exp.abs()
        )
    }
}

fn write_string(out: &mut String, text: &str, ensure_ascii: bool) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c if ensure_ascii && (c as u32) > 0x7f => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn write_value(out: &mut String, value: &Value, indent: Option<usize>, level: usize, ascii: bool) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                out.push_str(&i.to_string());
            } else if let Some(u) = n.as_u64() {
                out.push_str(&u.to_string());
            } else {
                out.push_str(&float_repr(n.as_f64().unwrap_or(0.0)));
            }
        }
        Value::String(s) => write_string(out, s, ascii),
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                match indent {
                    Some(step) => {
                        if i > 0 {
                            out.push(',');
                        }
                        out.push('\n');
                        out.push_str(&" ".repeat(step * (level + 1)));
                    }
                    None => {
                        if i > 0 {
                            out.push_str(", ");
                        }
                    }
                }
                write_value(out, item, indent, level + 1, ascii);
            }
            if let Some(step) = indent {
                out.push('\n');
                out.push_str(&" ".repeat(step * level));
            }
            out.push(']');
        }
        Value::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (i, (key, item)) in map.iter().enumerate() {
                match indent {
                    Some(step) => {
                        if i > 0 {
                            out.push(',');
                        }
                        out.push('\n');
                        out.push_str(&" ".repeat(step * (level + 1)));
                    }
                    None => {
                        if i > 0 {
                            out.push_str(", ");
                        }
                    }
                }
                write_string(out, key, ascii);
                out.push_str(": ");
                write_value(out, item, indent, level + 1, ascii);
            }
            if let Some(step) = indent {
                out.push('\n');
                out.push_str(&" ".repeat(step * level));
            }
            out.push('}');
        }
    }
}

/// `round(x, 2)` as Python writes it (round-half-even on the exact value).
pub fn round2(x: f64) -> f64 {
    format!("{x:.2}").parse().unwrap_or(x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn floats_read_like_repr() {
        for (x, want) in [
            (0.0, "0.0"),
            (1.0, "1.0"),
            (38.0, "38.0"),
            (0.5, "0.5"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (1e16, "1e+16"),
            (1.5e300, "1.5e+300"),
            (123456789.125, "123456789.125"),
            (-2.5, "-2.5"),
            (0.1, "0.1"),
        ] {
            assert_eq!(float_repr(x), want, "{x}");
        }
    }

    #[test]
    fn indent_and_ascii_match_python() {
        let v = json!({"a": [1, 2.5, "é😀"], "b": {}, "c": []});
        assert_eq!(
            dumps(&v, Some(2), true),
            "{\n  \"a\": [\n    1,\n    2.5,\n    \"\\u00e9\\ud83d\\ude00\"\n  ],\n  \"b\": {},\n  \"c\": []\n}"
        );
        assert_eq!(
            dumps(&v, None, false),
            "{\"a\": [1, 2.5, \"é😀\"], \"b\": {}, \"c\": []}"
        );
    }

    #[test]
    fn rounding_is_half_even_on_the_exact_value() {
        assert_eq!(round2(0.125), 0.12);
        assert_eq!(round2(0.375), 0.38);
        assert_eq!(round2(1.005), 1.0);
        assert_eq!(round2(2.0), 2.0);
    }
}
