//! Python text semantics the reference parser depends on, where Rust's `char` methods are a different set.
//!
//! * `str.isalpha()` is the categories Lm/Lt/Lu/Ll/Lo (Rust's `is_alphabetic` also counts combining marks and letter numbers);
//! * `\d` and `int()` accept every decimal digit (`Nd`) of every script, and `str.isdigit()` also accepts a few
//!   non-decimal digits (circled digits, superscripts) that `int()` then rejects;
//! * `re.IGNORECASE` on a `[A-Za-z]` set or an ASCII literal also matches `ı` (U+0131), `İ` (U+0130), `ſ` (U+017F) and
//!   the Kelvin sign (U+212A);
//! * `str.casefold()`.
//!
//! The first three are fixed tables from CPython (`unitables.rs`, Unicode 16.0), so the answer does not move with the Rust toolchain.

use super::unitables::{ALPHA, DECIMAL_ZEROS, DIGIT_NOT_DECIMAL};

fn in_ranges(table: &[(u32, u32)], c: char) -> bool {
    let cp = c as u32;
    table
        .binary_search_by(|&(lo, hi)| {
            if cp < lo {
                std::cmp::Ordering::Greater
            } else if cp > hi {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// `c.isalpha()`.
pub(crate) fn is_alpha(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_alphabetic();
    }
    in_ranges(ALPHA, c)
}

/// `unicodedata.decimal(c, None)`: the value of a decimal digit (category Nd), which is what `\d` matches and `int()` reads.
pub fn decimal_value(c: char) -> Option<u32> {
    if c.is_ascii_digit() {
        return Some(c as u32 - '0' as u32);
    }
    let cp = c as u32;
    let index = DECIMAL_ZEROS.partition_point(|&zero| zero <= cp);
    let zero = *DECIMAL_ZEROS.get(index.checked_sub(1)?)?;
    (cp < zero + 10).then(|| cp - zero)
}

/// `c.isdigit()`: a decimal digit, or one of the digits that are not decimal (`①`, `²`, ...).
pub(crate) fn is_digit(c: char) -> bool {
    decimal_value(c).is_some() || in_ranges(DIGIT_NOT_DECIMAL, c)
}

/// What `re.IGNORECASE` makes of a character when it is compared with an ASCII letter: ASCII letters lower-cased, and the
/// four non-ASCII characters Python equates with `i`, `s` and `k` mapped to those.
pub(super) fn fold(c: char) -> char {
    match c {
        'ı' | 'İ' => 'i',
        'ſ' => 's',
        '\u{212A}' => 'k',
        _ => c.to_ascii_lowercase(),
    }
}

/// `[A-Za-z]` under `re.IGNORECASE`.
pub(super) fn is_letter_ic(c: char) -> bool {
    fold(c).is_ascii_lowercase()
}

/// `str.casefold()`. Exact for ASCII and for the characters that fold to something else in the scripts Bible notes
/// use; other non-ASCII characters use the full lower-case mapping (they cannot equal any ASCII name either way).
pub fn casefold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            'ß' | 'ẞ' => out.push_str("ss"),
            'ſ' => out.push('s'),
            'ς' => out.push('σ'),
            'ı' => out.push('ı'),
            _ => out.extend(c.to_lowercase()),
        }
    }
    out
}

/// Titlecase letters (category Lt): `str.isupper()` and `str.islower()` are false for a string that has one.
pub(crate) fn is_title(c: char) -> bool {
    matches!(c as u32, 0x1C5 | 0x1C8 | 0x1CB | 0x1F2 | 0x1F88..=0x1F8F | 0x1F98..=0x1F9F | 0x1FA8..=0x1FAF | 0x1FBC | 0x1FCC | 0x1FFC)
}

/// `str.isupper()`: at least one cased character, no lower-case or titlecase one.
pub(crate) fn is_upper(text: &str) -> bool {
    let mut cased = false;
    for c in text.chars() {
        if c.is_lowercase() || is_title(c) {
            return false;
        }
        if c.is_uppercase() {
            cased = true;
        }
    }
    cased
}

/// `str.islower()`: at least one cased character, no upper-case or titlecase one.
pub(crate) fn is_lower(text: &str) -> bool {
    let mut cased = false;
    for c in text.chars() {
        if c.is_uppercase() || is_title(c) {
            return false;
        }
        if c.is_lowercase() {
            cased = true;
        }
    }
    cased
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_is_the_python_set() {
        assert!(is_alpha('a') && is_alpha('é') && is_alpha('中') && is_alpha('ß'));
        assert!(!is_alpha('1') && !is_alpha('.') && !is_alpha('\u{301}') && !is_alpha('Ⅳ'));
    }

    #[test]
    fn digits() {
        assert_eq!(decimal_value('7'), Some(7));
        assert_eq!(decimal_value('٣'), Some(3)); // Arabic-Indic
        assert_eq!(decimal_value('९'), Some(9)); // Devanagari
        assert_eq!(decimal_value('①'), None);
        assert!(is_digit('①') && is_digit('²') && !is_digit('a'));
    }

    #[test]
    fn ignorecase_equivalences() {
        assert!(is_letter_ic('ı') && is_letter_ic('İ') && is_letter_ic('Q') && !is_letter_ic('é'));
        assert_eq!(casefold("Straße"), "strasse");
        assert!(is_upper("NIV2") && !is_upper("Niv") && !is_upper("22"));
    }
}
