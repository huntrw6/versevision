//! The RTF the text objects of a presentation carry: replacing the visible text of a template's RTF, escaping text for it, measuring it
//! (`presentation/protobuf_writer.py`: `_escape_rtf`, `_text_start`, `_replace_rtf_text`, `_visible_length`; `template.py`: `_plain_rtf`).
//! The patterns are Python's `re` ones on bytes, so they are compiled byte-wise (`(?-u)`): no UTF-8 is assumed.

use std::sync::LazyLock;

use regex::bytes::Regex as BytesRegex;
use regex::Regex;
use versevision_core::pystr;

use crate::error::PpError;

/// `bytes.rstrip()` (ASCII whitespace).
fn rstrip_ascii(bytes: &[u8]) -> &[u8] {
    let mut end = bytes.len();
    while end > 0 && matches!(bytes[end - 1], b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c) {
        end -= 1;
    }
    &bytes[..end]
}

/// `_escape_rtf`: `\`, `{`, `}` escaped, a line end is `\line `, a character above U+007F is `\uN?` (N negative above 32767, even for characters
/// outside the BMP: Python subtracts 65536 from the code point as it is).
pub fn escape_rtf(text: &str) -> Vec<u8> {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '\\' | '{' | '}' => {
                out.push('\\');
                out.push(character);
            }
            '\n' => out.push_str("\\line "),
            c if (c as u32) > 127 => {
                let mut value = c as i64;
                if value > 32767 {
                    value -= 65536;
                }
                out.push_str(&format!("\\u{value}?"));
            }
            c => out.push(c),
        }
    }
    out.into_bytes()
}

static HEADER_GROUPS: LazyLock<BytesRegex> = LazyLock::new(|| {
    BytesRegex::new(r"(?s-u)\{\\(?:fonttbl|colortbl|\*\\expandedcolortbl|stylesheet|\*\\listtable)[^{}]*(?:\{[^{}]*\}[^{}]*)*\}").expect("header groups pattern")
});
static CONTROL: LazyLock<BytesRegex> = LazyLock::new(|| {
    BytesRegex::new(r"(?-u)\\(?P<word>[A-Za-z]+)(?P<num>-?[0-9]+)? ?|\\\*|[\r\n]+")
        .expect("control pattern")
});
static CB_MARKER: LazyLock<BytesRegex> =
    LazyLock::new(|| BytesRegex::new(r"(?-u)\\cb[0-9]+[ \t\n\r\x0b\x0c]?").expect("cb pattern"));

/// `_text_start`: where visible text begins: after the RTF header and the last formatting control word.
pub fn text_start(rtf: &[u8]) -> Result<usize, PpError> {
    if let Some(last) = CB_MARKER.find_iter(rtf).last() {
        return Ok(last.end()); // Windows-style output ends its header with \cbN
    }
    let mut position = 0;
    for group in HEADER_GROUPS.find_iter(rtf) {
        position = group.end();
    }
    if position == 0 {
        return Err(PpError::value("template RTF has no recognizable header"));
    }
    while position < rtf.len() {
        let Some(caps) = CONTROL.captures_at(rtf, position) else {
            break;
        };
        let whole = caps.get(0).expect("group 0");
        if whole.start() != position {
            break;
        }
        if caps.name("word").is_some_and(|w| w.as_bytes() == b"u") && caps.name("num").is_some() {
            break; // \u8217? is text, not formatting
        }
        position = whole.end();
    }
    Ok(position)
}

/// `_replace_rtf_text`: the template's RTF with its visible text replaced.
pub fn replace_rtf_text(rtf: &[u8], text: &str) -> Result<Vec<u8>, PpError> {
    if !rtf.starts_with(b"{\\rtf") || !rstrip_ascii(rtf).ends_with(b"}") {
        return Err(PpError::value(
            "template text element does not contain supported RTF",
        ));
    }
    let trimmed = rstrip_ascii(rtf);
    let mut prefix = trimmed[..text_start(trimmed)?].to_vec();
    if !matches!(prefix.last(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
        prefix.push(b' ');
    }
    prefix.extend_from_slice(&escape_rtf(text));
    prefix.push(b'}');
    Ok(prefix)
}

static LINE: LazyLock<BytesRegex> =
    LazyLock::new(|| BytesRegex::new(r"(?-u)\\line ?").expect("line pattern"));
static UNICODE: LazyLock<BytesRegex> =
    LazyLock::new(|| BytesRegex::new(r"(?-u)\\u-?[0-9]+\??").expect("unicode pattern"));
static ESCAPED: LazyLock<BytesRegex> =
    LazyLock::new(|| BytesRegex::new(r"(?-u)\\([\\{}])").expect("escape pattern"));

/// `_visible_length`: how many characters (UTF-16 units, the way ProPresenter counts them) the visible text of an RTF block has.
/// Python decodes the bytes as latin-1, so every byte is one character of one UTF-16 unit.
pub fn visible_length(rtf: &[u8]) -> Result<usize, PpError> {
    let trimmed = rstrip_ascii(rtf);
    let start = text_start(trimmed)?;
    // Python slices `trimmed[start:-1]` (or `trimmed[start:]`): past the end it is an empty slice, not an error
    let end = if trimmed.ends_with(b"}") {
        trimmed.len() - 1
    } else {
        trimmed.len()
    };
    let body = if start < end {
        &trimmed[start..end]
    } else {
        &[][..]
    };
    let step1 = LINE.replace_all(body, &b"\n"[..]);
    let step2 = UNICODE.replace_all(&step1, &b"\x01"[..]);
    let step3 = ESCAPED.replace_all(&step2, &b"$1"[..]);
    Ok(step3.len())
}

static P_CB: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\cb[0-9]+[\s\x1c-\x1f]").expect("cb pattern"));
static P_UNI: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\u(-?[0-9]+)[\s\x1c-\x1f]*\?").expect("unicode pattern"));
static P_HEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\'[0-9a-fA-F]{2}").expect("hex pattern"));
static P_WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\[A-Za-z]+-?[0-9]*[\s\x1c-\x1f]?").expect("word pattern"));

/// `template._plain_rtf`: the plain text of an RTF block (for the previews of the template picker).
pub fn plain_rtf(value: &[u8]) -> String {
    // bytes.decode("ascii", "replace"): every byte above 0x7F becomes U+FFFD
    let mut text: String = value
        .iter()
        .map(|b| if *b < 0x80 { *b as char } else { '\u{fffd}' })
        .collect();
    if let Some(last) = P_CB.find_iter(&text).last() {
        text = text[last.end()..].to_string();
    }
    if text.ends_with('}') {
        text.pop();
    }
    let text = P_UNI
        .replace_all(&text, |caps: &regex::Captures<'_>| {
            let n: i64 = caps[1].parse().unwrap_or(0);
            let code = n.rem_euclid(65536) as u32;
            // a lone surrogate cannot be a Rust char (Python keeps it): U+FFFD stands in
            char::from_u32(code).unwrap_or('\u{fffd}').to_string()
        })
        .into_owned();
    let text = P_HEX.replace_all(&text, "").into_owned();
    let text = P_WORD.replace_all(&text, "").into_owned();
    let text = text.replace(['{', '}'], "");
    pystr::flatten(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The expected values are what `protobuf_writer._text_start/_visible_length/_replace_rtf_text/_escape_rtf` and `template._plain_rtf` return.
    const FULL: &[u8] = b"{\\rtf1\\ansi{\\fonttbl\\f0 Arial;}{\\colortbl;\\red0\\green0\\blue0;}\\pard\\qc\\f0\\fs40 \\cf1 Hello \\u8217? world\\line there \\\\ \\{x\\}}";

    #[test]
    fn the_text_starts_after_the_last_control_word() {
        assert_eq!(text_start(FULL).unwrap(), 84);
        assert_eq!(
            text_start(b"{\\rtf1\\ansi{\\fonttbl\\f0 Arial;}\\cb2 abc}").unwrap(),
            36
        );
        assert_eq!(
            text_start(b"{\\rtf1\\ansi{\\fonttbl\\f0 Arial;}\\u8217?x}").unwrap(),
            31
        ); // \u8217? is text
        assert_eq!(
            text_start(b"{\\rtf1}").unwrap_err().to_string(),
            "template RTF has no recognizable header"
        );
    }

    #[test]
    fn the_visible_length_counts_what_propresenter_counts() {
        assert_eq!(visible_length(FULL).unwrap(), 25);
        assert_eq!(
            visible_length(b"{\\rtf1\\ansi{\\fonttbl\\f0 Arial;}\\cb2\nabc\n}").unwrap(),
            4
        );
        assert_eq!(
            visible_length(b"{\\rtf1\\ansi{\\fonttbl\\f0 Arial;}}").unwrap(),
            0
        );
        assert_eq!(
            visible_length(b"{\\rtf1\\ansi{\\fonttbl\\f0 Arial;}\\u8217?x}").unwrap(),
            2
        );
    }

    #[test]
    fn replacing_the_text_keeps_the_header() {
        assert_eq!(
            replace_rtf_text(FULL, "Hi \u{2019} there\nx").unwrap(),
            b"{\\rtf1\\ansi{\\fonttbl\\f0 Arial;}{\\colortbl;\\red0\\green0\\blue0;}\\pard\\qc\\f0\\fs40 \\cf1 Hi \\u8217? there\\line x}"
        );
        assert_eq!(
            replace_rtf_text(b"{\\rtf1\\ansi{\\fonttbl\\f0 Arial;}\\cb2\nabc\n}", "Hi").unwrap(),
            b"{\\rtf1\\ansi{\\fonttbl\\f0 Arial;}\\cb2\nHi}"
        );
        assert_eq!(
            replace_rtf_text(b"plain", "x").unwrap_err().to_string(),
            "template text element does not contain supported RTF"
        );
    }

    #[test]
    fn escaping_follows_python_even_for_characters_outside_the_bmp() {
        // apostrophes and dashes become \uN? (not \'92 / \'97): that is what the Python app writes
        assert_eq!(
            escape_rtf("a\\b{c}\n\u{2019}\u{e9}\u{1F600}\u{7fff}\u{8000}"),
            b"a\\\\b\\{c\\}\\line \\u8217?\\u233?\\u62976?\\u32767?\\u-32768?"
        );
        assert_eq!(
            escape_rtf("It\u{2019}s \u{2014} ok"),
            b"It\\u8217?s \\u8212? ok"
        );
    }

    #[test]
    fn plain_text_of_an_rtf_block() {
        assert_eq!(
            plain_rtf(FULL),
            "Arial;;;Hello \u{2019} worldthere \\\\ \\x\\"
        );
        assert_eq!(
            plain_rtf(b"{\\rtf1\\ansi{\\fonttbl\\f0 Arial;}\\cb2 abc}"),
            "abc"
        );
        assert_eq!(
            plain_rtf(b"{\\rtf1\\ansi{\\fonttbl\\f0 Arial;}\\u8217?x}"),
            "Arial;\u{2019}x"
        );
    }
}
