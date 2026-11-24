//! The text of Python's `json.JSONDecodeError`, for the bodies `serde_json` rejects (`httpx.Response.json()` is what the
//! providers call, and the planner shows the exception's text: `API.Bible response failed: Expecting value: line 1 column 1
//! (char 0); no presentation files changed`).
//!
//! This is a checker that follows `json.decoder` (the C scanner of CPython 3.13): it reads the same grammar in the same order and
//! reports the first error the same way. It does not build a value.

/// `json.detect_encoding` and the decode of `json.loads(bytes)`: the text, or the text of the `UnicodeDecodeError`.
pub fn decode_body(body: &[u8]) -> Result<String, String> {
    let utf16 = |bytes: &[u8], big: bool| -> Result<String, String> {
        if !bytes.len().is_multiple_of(2) {
            return Err("'utf-16' codec can't decode byte: truncated data".to_string());
        }
        let units = bytes.chunks(2).map(|c| {
            if big {
                u16::from_be_bytes([c[0], c[1]])
            } else {
                u16::from_le_bytes([c[0], c[1]])
            }
        });
        char::decode_utf16(units)
            .collect::<Result<String, _>>()
            .map_err(|_| "'utf-16' codec can't decode bytes: illegal encoding".to_string())
    };
    let utf32 = |bytes: &[u8], big: bool| -> Result<String, String> {
        if !bytes.len().is_multiple_of(4) {
            return Err("'utf-32' codec can't decode bytes: truncated data".to_string());
        }
        bytes
            .chunks(4)
            .map(|c| {
                let value = if big {
                    u32::from_be_bytes([c[0], c[1], c[2], c[3]])
                } else {
                    u32::from_le_bytes([c[0], c[1], c[2], c[3]])
                };
                char::from_u32(value).ok_or_else(|| {
                    "'utf-32' codec can't decode bytes: code point not in range(0x110000)"
                        .to_string()
                })
            })
            .collect()
    };
    if body.starts_with(&[0x00, 0x00, 0xfe, 0xff]) {
        return utf32(&body[4..], true);
    }
    if body.starts_with(&[0xff, 0xfe, 0x00, 0x00]) {
        return utf32(&body[4..], false);
    }
    if body.starts_with(&[0xfe, 0xff]) {
        return utf16(&body[2..], true);
    }
    if body.starts_with(&[0xff, 0xfe]) {
        return utf16(&body[2..], false);
    }
    let utf8 = |bytes: &[u8]| {
        std::str::from_utf8(bytes)
            .map(str::to_string)
            .map_err(|e| utf8_message(bytes, e))
    };
    if let Some(rest) = body.strip_prefix(b"\xef\xbb\xbf") {
        return utf8(rest);
    }
    if body.len() >= 4 {
        if body[0] == 0 {
            return if body[1] != 0 {
                utf16(body, true)
            } else {
                utf32(body, true)
            };
        }
        if body[1] == 0 {
            return if body[2] != 0 || body[3] != 0 {
                utf16(body, false)
            } else {
                utf32(body, false)
            };
        }
    } else if body.len() == 2 {
        if body[0] == 0 {
            return utf16(body, true);
        }
        if body[1] == 0 {
            return utf16(body, false);
        }
    }
    utf8(body)
}

/// What `httpx.Response.json()` raises for the bytes of a body: the `UnicodeDecodeError` text when they cannot be decoded, otherwise
/// [`decode_error`] of the text. `None`: Python reads it.
pub fn loads_error(body: &[u8]) -> Option<String> {
    match decode_body(body) {
        Ok(text) => decode_error(&text),
        Err(message) => Some(message),
    }
}

/// Python's `UnicodeDecodeError` text for the first bad byte(s) of `body`.
pub(crate) fn utf8_message(body: &[u8], error: std::str::Utf8Error) -> String {
    let position = error.valid_up_to();
    let byte = body[position];
    match error.error_len() {
        None => {
            // a sequence cut off by the end of the data: every remaining byte is reported
            let last = body.len() - 1;
            if last == position {
                format!(
                    "'utf-8' codec can't decode byte 0x{byte:02x} in position {position}: unexpected end of data"
                )
            } else {
                format!(
                    "'utf-8' codec can't decode bytes in position {position}-{last}: unexpected end of data"
                )
            }
        }
        Some(_) => {
            let reason = if (0x80..=0xc1).contains(&byte) || byte >= 0xf5 {
                "invalid start byte"
            } else {
                "invalid continuation byte"
            };
            format!("'utf-8' codec can't decode byte 0x{byte:02x} in position {position}: {reason}")
        }
    }
}

/// What `json.loads(body)` raises, or `None` when Python accepts the text. `body` is already decoded (UTF-8, no BOM).
pub fn decode_error(text: &str) -> Option<String> {
    let s: Vec<char> = text.chars().collect();
    let scanner = Scanner { s: &s };
    let result = (|| {
        let start = scanner.skip_ws(0);
        let end = scanner.scan_once(start).map_err(|stop| match stop {
            Stop::Value(at) => ("Expecting value", at),
            Stop::Error(message, at) => (message, at),
        })?;
        let end = scanner.skip_ws(end);
        if end != s.len() {
            return Err(("Extra data", end));
        }
        Ok(())
    })();
    result.err().map(|(message, pos)| format(message, &s, pos))
}

fn format(message: &str, s: &[char], pos: usize) -> String {
    let before = &s[..pos.min(s.len())];
    let lineno = before.iter().filter(|&&c| c == '\n').count() + 1;
    let colno = match before.iter().rposition(|&c| c == '\n') {
        Some(last) => pos - last,
        None => pos + 1,
    };
    format!("{message}: line {lineno} column {colno} (char {pos})")
}

enum Stop {
    /// `StopIteration(idx)`: no JSON value starts there (the caller says "Expecting value").
    Value(usize),
    Error(&'static str, usize),
}

struct Scanner<'a> {
    s: &'a [char],
}

type Scan = Result<usize, Stop>;

impl Scanner<'_> {
    fn at(&self, i: usize) -> Option<char> {
        self.s.get(i).copied()
    }

    fn skip_ws(&self, mut i: usize) -> usize {
        while matches!(self.at(i), Some(' ' | '\t' | '\n' | '\r')) {
            i += 1;
        }
        i
    }

    fn starts(&self, i: usize, word: &str) -> bool {
        word.chars()
            .enumerate()
            .all(|(k, c)| self.at(i + k) == Some(c))
    }

    /// `scan_once_unicode`: the end of the value that starts at `idx`.
    fn scan_once(&self, idx: usize) -> Scan {
        let Some(c) = self.at(idx) else {
            return Err(Stop::Value(idx));
        };
        match c {
            '"' => self.string(idx + 1),
            '{' => self.object(idx + 1),
            '[' => self.array(idx + 1),
            'n' if self.starts(idx, "null") => Ok(idx + 4),
            't' if self.starts(idx, "true") => Ok(idx + 4),
            'f' if self.starts(idx, "false") => Ok(idx + 5),
            'N' if self.starts(idx, "NaN") => Ok(idx + 3),
            'I' if self.starts(idx, "Infinity") => Ok(idx + 8),
            '-' if self.starts(idx, "-Infinity") => Ok(idx + 9),
            _ => self.number(idx),
        }
    }

    /// `_match_number_unicode`.
    fn number(&self, start: usize) -> Scan {
        let mut idx = start;
        if self.at(idx) == Some('-') {
            idx += 1;
            if idx >= self.s.len() {
                return Err(Stop::Value(start));
            }
        }
        match self.at(idx) {
            Some('1'..='9') => {
                idx += 1;
                while matches!(self.at(idx), Some('0'..='9')) {
                    idx += 1;
                }
            }
            Some('0') => idx += 1,
            _ => return Err(Stop::Value(start)),
        }
        // fraction: '.' followed by at least one digit
        if self.at(idx) == Some('.') && matches!(self.at(idx + 1), Some('0'..='9')) {
            idx += 2;
            while matches!(self.at(idx), Some('0'..='9')) {
                idx += 1;
            }
        }
        // exponent
        if matches!(self.at(idx), Some('e' | 'E')) {
            let e_start = idx;
            idx += 1;
            if matches!(self.at(idx), Some('-' | '+')) {
                idx += 1;
            }
            let digits = idx;
            while matches!(self.at(idx), Some('0'..='9')) {
                idx += 1;
            }
            if idx == digits {
                idx = e_start;
            }
        }
        Ok(idx)
    }

    /// `scanstring_unicode`: `end` is the index after the opening quote. Returns the index after the closing quote.
    fn string(&self, end: usize) -> Scan {
        let begin = end - 1;
        let len = self.s.len();
        let mut next = end;
        loop {
            // the plain run
            let mut c = None;
            while next < len {
                let ch = self.s[next];
                if ch == '"' || ch == '\\' {
                    c = Some(ch);
                    break;
                }
                if (ch as u32) <= 0x1f {
                    return Err(Stop::Error("Invalid control character at", next));
                }
                next += 1;
            }
            let Some(c) = c else {
                return Err(Stop::Error("Unterminated string starting at", begin));
            };
            if c == '"' {
                return Ok(next + 1);
            }
            // a backslash
            next += 1;
            let Some(escape) = self.at(next) else {
                return Err(Stop::Error("Unterminated string starting at", begin));
            };
            if escape != 'u' {
                if !matches!(escape, '"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't') {
                    return Err(Stop::Error("Invalid \\escape", next - 1));
                }
                next += 1;
                continue;
            }
            // \uXXXX
            let hex = |from: usize, error_at: usize| -> Result<u32, Stop> {
                let mut value = 0u32;
                for k in 0..4 {
                    match self.at(from + k).and_then(|c| c.to_digit(16)) {
                        Some(d) if self.at(from + k).is_some_and(|c| c.is_ascii()) => {
                            value = value * 16 + d;
                        }
                        _ => return Err(Stop::Error("Invalid \\uXXXX escape", error_at)),
                    }
                }
                Ok(value)
            };
            next += 1;
            if next + 4 > len {
                return Err(Stop::Error("Invalid \\uXXXX escape", next - 1));
            }
            let first = hex(next, next - 1)?;
            next += 4;
            if (0xd800..=0xdbff).contains(&first)
                && self.at(next) == Some('\\')
                && self.at(next + 1) == Some('u')
            {
                // a low surrogate must follow in the same shape
                let second = hex(next + 2, next + 1)?;
                if (0xdc00..=0xdfff).contains(&second) {
                    next += 6;
                }
            }
        }
    }

    /// `_parse_object_unicode`: `idx` is after `{`. Returns the index after `}`.
    fn object(&self, mut idx: usize) -> Scan {
        idx = self.skip_ws(idx);
        if self.at(idx) != Some('}') {
            loop {
                if self.at(idx) != Some('"') {
                    return Err(Stop::Error(
                        "Expecting property name enclosed in double quotes",
                        idx,
                    ));
                }
                idx = self.string(idx + 1)?;
                idx = self.skip_ws(idx);
                if self.at(idx) != Some(':') {
                    return Err(Stop::Error("Expecting ':' delimiter", idx));
                }
                idx += 1;
                idx = self.skip_ws(idx);
                idx = self.scan_once(idx)?;
                idx = self.skip_ws(idx);
                if self.at(idx) == Some('}') {
                    break;
                }
                if self.at(idx) != Some(',') {
                    return Err(Stop::Error("Expecting ',' delimiter", idx));
                }
                let comma = idx;
                idx += 1;
                idx = self.skip_ws(idx);
                if self.at(idx) == Some('}') {
                    return Err(Stop::Error(
                        "Illegal trailing comma before end of object",
                        comma,
                    ));
                }
            }
        }
        Ok(idx + 1)
    }

    /// `_parse_array_unicode`: `idx` is after `[`. Returns the index after `]`.
    fn array(&self, mut idx: usize) -> Scan {
        idx = self.skip_ws(idx);
        if self.at(idx) != Some(']') {
            loop {
                idx = self.scan_once(idx)?;
                idx = self.skip_ws(idx);
                if self.at(idx) == Some(']') {
                    break;
                }
                if self.at(idx) != Some(',') {
                    return Err(Stop::Error("Expecting ',' delimiter", idx));
                }
                let comma = idx;
                idx += 1;
                idx = self.skip_ws(idx);
                if self.at(idx) == Some(']') {
                    return Err(Stop::Error(
                        "Illegal trailing comma before end of array",
                        comma,
                    ));
                }
            }
        }
        Ok(idx + 1)
    }
}
