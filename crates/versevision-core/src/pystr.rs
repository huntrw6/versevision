//! Python string behaviour the port has to match: `str.isspace`, `split()`, `strip()`, `splitlines()`, `sum()`.
//!
//! Rust's `char::is_whitespace` is the Unicode `White_Space` property; Python's `str.isspace` is the same set plus the four
//! "information separator" controls U+001C..U+001F. Everything here goes through [`is_space`] so the two cannot drift.

/// `str.isspace()` for one character.
pub fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// `text.split()`: runs of whitespace separate words, no empty words.
pub fn split_ws(text: &str) -> Vec<&str> {
    text.split(is_space).filter(|w| !w.is_empty()).collect()
}

/// `" ".join(text.split())`.
pub fn flatten(text: &str) -> String {
    split_ws(text).join(" ")
}

/// `text.strip()`.
pub fn strip(text: &str) -> &str {
    text.trim_matches(is_space)
}

/// `text.rstrip()`.
pub fn rstrip(text: &str) -> &str {
    text.trim_end_matches(is_space)
}

/// `text.splitlines()`: boundaries `\n \r \r\n \v \f \x1c \x1d \x1e \x85 \u2028 \u2029`; no empty last element.
pub fn splitlines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let boundary = matches!(
            c,
            '\n' | '\r'
                | '\u{0b}'
                | '\u{0c}'
                | '\u{1c}'
                | '\u{1d}'
                | '\u{1e}'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if !boundary {
            continue;
        }
        lines.push(&text[start..i]);
        let mut end = i + c.len_utf8();
        if c == '\r' {
            if let Some(&(j, '\n')) = chars.peek() {
                chars.next();
                end = j + 1;
            }
        }
        start = end;
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// `sum(values)` of Python 3.12+: floats are added with Neumaier compensation (the first item starts the total,
/// the correction is added once at the end). An older `sum` adds plainly, so the oracle needs CPython >= 3.12.
pub fn sum(values: &[f64]) -> f64 {
    let mut iter = values.iter().copied();
    let Some(mut total) = iter.next() else {
        return 0.0;
    };
    let mut compensation = 0.0_f64;
    for x in iter {
        let t = total + x;
        if total.abs() >= x.abs() {
            compensation += (total - t) + x;
        } else {
            compensation += (x - t) + total;
        }
        total = t;
    }
    if compensation != 0.0 && compensation.is_finite() {
        total += compensation;
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_matches_python() {
        assert!(
            is_space('\u{1f}') && is_space('\u{a0}') && is_space('\u{3000}') && is_space('\u{85}')
        );
        assert!(!is_space('\u{200b}') && !is_space('a'));
        assert_eq!(split_ws(" a\u{a0}b\t\u{1c}c  "), vec!["a", "b", "c"]);
        assert_eq!(strip("\u{1f} x \n"), "x");
    }

    #[test]
    fn splitlines_matches_python() {
        assert_eq!(splitlines(""), Vec::<&str>::new());
        assert_eq!(splitlines("a\nb\n"), vec!["a", "b"]);
        assert_eq!(
            splitlines("a\r\nb\rc\u{2028}d\u{0c}"),
            vec!["a", "b", "c", "d"]
        );
        assert_eq!(splitlines("\n\nx"), vec!["", "", "x"]);
        assert_eq!(splitlines("a\u{1f}b"), vec!["a\u{1f}b"]);
    }

    #[test]
    fn sum_is_compensated() {
        // Python 3.12+: sum([0.1] * 10) == 1.0 (a plain left-to-right sum gives 0.9999999999999999)
        assert_eq!(sum(&[0.1; 10]), 1.0);
        assert_eq!(sum(&[]), 0.0);
    }
}
