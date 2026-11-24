//! Python's number formatting where the planner prints one: `round(x, 2)`, `f"{x:.2f}"`, `f"{x:.0%}"` and `str.format(n=...)`.
//!
//! Python rounds a float by its exact binary value, half to even; Rust's `{:.2}` is not relied on for that: the digits are
//! taken from the exact expansion and rounded here, so a score such as `1/8` gives `0.12` in both languages.

/// `format(x, f".{decimals}f")`: the exact decimal expansion of `x`, rounded half to even.
pub fn fixed(x: f64, decimals: usize) -> String {
    if !x.is_finite() {
        return if x.is_nan() {
            "nan".to_string()
        } else if x < 0.0 {
            "-inf".to_string()
        } else {
            "inf".to_string()
        };
    }
    let negative = x.is_sign_negative();
    // `{:.N}` prints the exact digits of the binary value (enough of them for any score or percentage).
    let exact = format!("{:.80}", x.abs());
    let (int_part, frac) = exact.split_once('.').unwrap_or((&exact, ""));
    let mut digits: Vec<u8> = int_part.bytes().map(|b| b - b'0').collect();
    let frac: Vec<u8> = frac.bytes().map(|b| b - b'0').collect();
    digits.extend(frac.iter().take(decimals));
    let rest = &frac[decimals.min(frac.len())..];
    let round_up = match rest.first() {
        None => false,
        Some(&d) if d > 5 => true,
        Some(&5) => rest[1..].iter().any(|&x| x != 0) || digits.last().is_some_and(|d| d % 2 == 1),
        Some(_) => false,
    };
    if round_up {
        let mut i = digits.len();
        loop {
            if i == 0 {
                digits.insert(0, 1);
                break;
            }
            i -= 1;
            if digits[i] == 9 {
                digits[i] = 0;
            } else {
                digits[i] += 1;
                break;
            }
        }
    }
    let int_len = digits.len() - decimals;
    let mut out = String::new();
    if negative {
        out.push('-');
    }
    out.extend(digits[..int_len].iter().map(|d| char::from(b'0' + d)));
    if decimals > 0 {
        out.push('.');
        out.extend(digits[int_len..].iter().map(|d| char::from(b'0' + d)));
    }
    out
}

/// `round(x, 2)`.
pub fn round2(x: f64) -> f64 {
    fixed(x, 2).parse().unwrap_or(x)
}

/// `format(x, ".0%")`: `x * 100` as a float, then rounded half to even, with a `%`.
pub fn percent0(x: f64) -> String {
    format!("{}%", fixed(x * 100.0, 0))
}

/// Why a `number_style` could not be formatted (Python raises `KeyError`, `IndexError` or `ValueError` there).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatError(pub String);

/// `style.format(n=value)` for the shapes people write: `{n})`, `{n}.`, `#{n}:`, `{n:02d}`, `{{`, `}}`.
/// Any other field (`{}`, `{0}`, `{x}`, `{n.real}`, nested specs) is a [`FormatError`].
pub fn format_n(style: &str, n: i64) -> Result<String, FormatError> {
    let chars: Vec<char> = style.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '{' if chars.get(i + 1) == Some(&'{') => {
                out.push('{');
                i += 2;
            }
            '}' if chars.get(i + 1) == Some(&'}') => {
                out.push('}');
                i += 2;
            }
            '}' => {
                return Err(FormatError(
                    "Single '}' encountered in format string".into(),
                ))
            }
            '{' => {
                let close = chars[i + 1..]
                    .iter()
                    .position(|&c| c == '}')
                    .ok_or_else(|| FormatError("expected '}' before end of string".into()))?;
                let field: String = chars[i + 1..i + 1 + close].iter().collect();
                if field.contains('{') {
                    return Err(FormatError(
                        "nested replacement fields are not supported".into(),
                    ));
                }
                out.push_str(&format_field(&field, n)?);
                i += close + 2;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    Ok(out)
}

fn format_field(field: &str, n: i64) -> Result<String, FormatError> {
    let (name_conv, spec) = match field.split_once(':') {
        Some((a, b)) => (a, b),
        None => (field, ""),
    };
    let name = match name_conv.split_once('!') {
        Some((name, "s" | "r" | "a")) => name,
        Some(_) => return Err(FormatError("Unknown conversion specifier".into())),
        None => name_conv,
    };
    if name != "n" {
        return Err(FormatError(
            if name.is_empty() || name.chars().all(|c| c.is_ascii_digit()) {
                "Replacement index out of range for positional args tuple".to_string()
            } else {
                format!("'{name}'")
            },
        ));
    }
    if name_conv.contains('!') {
        return Ok(n.to_string());
    }
    format_int(n, spec)
}

/// The `format_spec` of an int: `[[fill]align][sign][0][width][,|_][type]` with type `d`, `x`, `X`, `o`, `b` or none.
fn format_int(n: i64, spec: &str) -> Result<String, FormatError> {
    let chars: Vec<char> = spec.chars().collect();
    let mut i = 0;
    let (mut fill, mut align) = (' ', None);
    let is_align = |c: char| matches!(c, '<' | '>' | '^' | '=');
    if chars.len() >= 2 && is_align(chars[1]) {
        fill = chars[0];
        align = Some(chars[1]);
        i = 2;
    } else if !chars.is_empty() && is_align(chars[0]) {
        align = Some(chars[0]);
        i = 1;
    }
    let mut sign = '-';
    if matches!(chars.get(i), Some('+' | '-' | ' ')) {
        sign = chars[i];
        i += 1;
    }
    if chars.get(i) == Some(&'0') {
        if align.is_none() {
            fill = '0';
            align = Some('=');
        }
        i += 1;
    }
    let mut width = 0usize;
    while let Some(d) = chars.get(i).and_then(|c| c.to_digit(10)) {
        width = width * 10 + d as usize;
        i += 1;
    }
    let grouping = match chars.get(i) {
        Some(&c @ (',' | '_')) => {
            i += 1;
            Some(c)
        }
        _ => None,
    };
    let kind = match &chars[i..] {
        [] => 'd',
        [c @ ('d' | 'x' | 'X' | 'o' | 'b')] => *c,
        _ => {
            return Err(FormatError(format!(
                "Invalid format specifier '{spec}' for object of type 'int'"
            )))
        }
    };
    let magnitude = n.unsigned_abs();
    let mut body = match kind {
        'x' => format!("{magnitude:x}"),
        'X' => format!("{magnitude:X}"),
        'o' => format!("{magnitude:o}"),
        'b' => format!("{magnitude:b}"),
        _ => magnitude.to_string(),
    };
    if let (Some(sep), 'd') = (grouping, kind) {
        let mut grouped = String::new();
        for (k, c) in body.chars().enumerate() {
            if k > 0 && (body.len() - k) % 3 == 0 {
                grouped.push(sep);
            }
            grouped.push(c);
        }
        body = grouped;
    }
    let prefix = if n < 0 {
        "-"
    } else if sign == '+' {
        "+"
    } else if sign == ' ' {
        " "
    } else {
        ""
    };
    let length = prefix.chars().count() + body.chars().count();
    let pad = width.saturating_sub(length);
    let padding = |count: usize| fill.to_string().repeat(count);
    Ok(match align.unwrap_or('>') {
        '<' => format!("{prefix}{body}{}", padding(pad)),
        '^' => format!(
            "{}{prefix}{body}{}",
            padding(pad / 2),
            padding(pad - pad / 2)
        ),
        '=' => format!("{prefix}{}{body}", padding(pad)),
        _ => format!("{}{prefix}{body}", padding(pad)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_half_to_even_on_the_exact_value() {
        assert_eq!(fixed(0.125, 2), "0.12");
        assert_eq!(fixed(0.375, 2), "0.38");
        assert_eq!(fixed(0.5, 0), "0");
        assert_eq!(fixed(1.5, 0), "2");
        assert_eq!(fixed(2.5, 0), "2");
        assert_eq!(fixed(0.675, 2), "0.68"); // 0.675 is 0.67500000000000004...
        assert_eq!(fixed(0.995, 2), "0.99"); // 0.995 is 0.99499999999999999...
        assert_eq!(fixed(1.0, 2), "1.00");
        assert_eq!(fixed(9.995, 2), "9.99");
        assert_eq!(fixed(99.5, 0), "100");
        assert_eq!(round2(0.125), 0.12);
        assert_eq!(round2(2.0 / 3.0), 0.67);
        assert_eq!(percent0(0.5), "50%");
        assert_eq!(percent0(0.005), "0%"); // 0.5 -> even
        assert_eq!(percent0(0.015), "2%"); // 1.5 -> even
    }

    #[test]
    fn number_styles() {
        assert_eq!(format_n("{n})", 3).unwrap(), "3)");
        assert_eq!(format_n("#{n}:", 12).unwrap(), "#12:");
        assert_eq!(format_n("{{{n}}}", 7).unwrap(), "{7}");
        assert_eq!(format_n("{n:02d}.", 4).unwrap(), "04.");
        assert_eq!(format_n("{n:>4}|", 4).unwrap(), "   4|");
        assert_eq!(format_n("{n:*^5}", 4).unwrap(), "**4**");
        assert_eq!(format_n("{n:+d}", 4).unwrap(), "+4");
        assert_eq!(format_n("{n:x}", 255).unwrap(), "ff");
        assert_eq!(format_n("{n!r}", 5).unwrap(), "5");
        assert_eq!(format_n("{n:,}", 12345).unwrap(), "12,345");
        assert_eq!(format_n("no field", 1).unwrap(), "no field");
        assert!(format_n("{}", 1).is_err());
        assert!(format_n("{x}", 1).is_err());
        assert!(format_n("}", 1).is_err());
        assert!(format_n("{n", 1).is_err());
        assert!(format_n("{n:.2f}", 1).is_err());
    }
}
