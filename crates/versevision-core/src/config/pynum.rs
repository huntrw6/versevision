//! Python's `int()`, `float()` and `str.casefold()` for the words the option parser feeds them, and pydantic's string
//! to number rules.

use crate::pystr::strip;

/// The first character of each run of ten decimal digits (Unicode category Nd, Unicode 15.1 as in CPython 3.13): the
/// ASCII digits, Arabic-Indic, Devanagari, full-width, mathematical, ...
const DIGIT_ZEROS: [u32; 68] = [
    0x30, 0x660, 0x6F0, 0x7C0, 0x966, 0x9E6, 0xA66, 0xAE6, 0xB66, 0xBE6, 0xC66, 0xCE6, 0xD66,
    0xDE6, 0xE50, 0xED0, 0xF20, 0x1040, 0x1090, 0x17E0, 0x1810, 0x1946, 0x19D0, 0x1A80, 0x1A90,
    0x1B50, 0x1BB0, 0x1C40, 0x1C50, 0xA620, 0xA8D0, 0xA900, 0xA9D0, 0xA9F0, 0xAA50, 0xABF0, 0xFF10,
    0x104A0, 0x10D30, 0x11066, 0x110F0, 0x11136, 0x111D0, 0x112F0, 0x11450, 0x114D0, 0x11650,
    0x116C0, 0x11730, 0x118E0, 0x11950, 0x11C50, 0x11D50, 0x11DA0, 0x11F50, 0x16A60, 0x16AC0,
    0x16B50, 0x1D7CE, 0x1D7D8, 0x1D7E2, 0x1D7EC, 0x1D7F6, 0x1E140, 0x1E2F0, 0x1E4F0, 0x1E950,
    0x1FBF0,
];

/// Python's `int()` and `float()` read every script's decimal digits (`"１２"` is 12): write them as ASCII.
fn ascii_digits(text: &str) -> String {
    text.chars()
        .map(|c| {
            let code = c as u32;
            DIGIT_ZEROS
                .iter()
                .find(|zero| code >= **zero && code < **zero + 10)
                .and_then(|zero| char::from_digit(code - zero, 10))
                .unwrap_or(c)
        })
        .collect()
}

/// Digits with single underscores between them (`1_000`), as Python and pydantic accept.
fn digits_ok(s: &str) -> bool {
    if s.is_empty() || s.starts_with('_') || s.ends_with('_') || s.contains("__") {
        return false;
    }
    s.chars().all(|c| c.is_ascii_digit() || c == '_')
}

/// What `int(text)` gives: a number that fits an i64 or one that does not (kept as the nearest f64: only its size matters).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PyInt {
    Small(i64),
    Huge(f64),
}

/// `int(text)`: surrounding whitespace, a sign, decimal digits (any script) with single underscores between them.
pub fn py_int(text: &str) -> Option<PyInt> {
    ascii_int(&ascii_digits(text))
}

/// `int(text)` for ASCII digits only (what pydantic reads from a string).
fn ascii_int(text: &str) -> Option<PyInt> {
    let t = strip(text);
    let (negative, body) = match t.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    if !digits_ok(body) {
        return None;
    }
    let clean: String = body.chars().filter(|c| *c != '_').collect();
    let signed = if negative { format!("-{clean}") } else { clean };
    Some(match signed.parse::<i64>() {
        Ok(v) => PyInt::Small(v),
        Err(_) => PyInt::Huge(signed.parse::<f64>().unwrap_or(if negative {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        })),
    })
}

/// `float(text)`: whitespace, a sign, digits with single underscores between digits, a fraction, an exponent, `inf`, `infinity`, `nan`.
pub fn py_float(text: &str) -> Option<f64> {
    ascii_float(&ascii_digits(text))
}

/// `float(text)` for ASCII digits only (what pydantic reads from a string).
pub fn ascii_float(text: &str) -> Option<f64> {
    let t = strip(text);
    if t.is_empty() {
        return None;
    }
    if t.contains('_') {
        // an underscore is only allowed between two digits
        let chars: Vec<char> = t.chars().collect();
        for (i, c) in chars.iter().enumerate() {
            if *c == '_' {
                let before = i > 0 && chars[i - 1].is_ascii_digit();
                let after = chars.get(i + 1).is_some_and(char::is_ascii_digit);
                if !(before && after) {
                    return None;
                }
            }
        }
    }
    let clean: String = t.chars().filter(|c| *c != '_').collect();
    // Rust also accepts "infinity"/"nan" in any case, like Python; reject what Python rejects.
    let body = clean.trim_start_matches(['+', '-']);
    if clean.len() - body.len() > 1 {
        return None;
    }
    let first = body.chars().next()?;
    if !(first.is_ascii_digit()
        || first == '.'
        || body.eq_ignore_ascii_case("inf")
        || body.eq_ignore_ascii_case("infinity")
        || body.eq_ignore_ascii_case("nan"))
    {
        return None;
    }
    clean.parse::<f64>().ok()
}

/// pydantic's integer from a string: `int` syntax, or digits with a fraction of zeros only (`"5.00"`).
pub fn pydantic_int_from_str(text: &str) -> Option<PyInt> {
    let t = strip(text);
    if let Some(v) = ascii_int(t) {
        return Some(v);
    }
    let (whole, fraction) = t.split_once('.')?;
    if fraction.is_empty() || !fraction.chars().all(|c| c == '0') {
        return None;
    }
    ascii_int(whole)
}

/// `text.casefold()` as far as it matters for the words true/false/yes/no/on/off: lower case, and the long s `ſ` is `s`.
pub fn casefold(text: &str) -> String {
    text.to_lowercase().replace('ſ', "s")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ints() {
        assert_eq!(py_int(" 1_0 "), Some(PyInt::Small(10)));
        assert_eq!(py_int("+5"), Some(PyInt::Small(5)));
        assert_eq!(py_int("1__0"), None);
        assert_eq!(py_int("1.0"), None);
        assert_eq!(py_int("0x10"), None);
        assert!(matches!(
            py_int("99999999999999999999999"),
            Some(PyInt::Huge(_))
        ));
        assert_eq!(pydantic_int_from_str("5.00"), Some(PyInt::Small(5)));
        assert_eq!(pydantic_int_from_str("1.5"), None);
        assert_eq!(pydantic_int_from_str("1."), None);
    }

    #[test]
    fn digits_of_other_scripts() {
        assert_eq!(py_int("１２"), Some(PyInt::Small(12)));
        assert_eq!(py_int("٣"), Some(PyInt::Small(3)));
        assert_eq!(py_float("１２.５"), Some(12.5));
        assert_eq!(
            pydantic_int_from_str("１２"),
            None,
            "pydantic only reads ASCII digits from a string"
        );
    }

    #[test]
    fn floats() {
        assert_eq!(py_float("1_0.5"), Some(10.5));
        assert_eq!(py_float(" .5 "), Some(0.5));
        assert_eq!(py_float("5."), Some(5.0));
        assert_eq!(py_float("1e1"), Some(10.0));
        assert_eq!(py_float("-Infinity"), Some(f64::NEG_INFINITY));
        assert!(py_float("nan").is_some_and(f64::is_nan));
        assert_eq!(py_float("1__0"), None);
        assert_eq!(py_float("0x1p3"), None);
        assert_eq!(py_float("--1"), None);
        assert_eq!(py_float("e5"), None);
        assert_eq!(py_float(""), None);
    }
}
