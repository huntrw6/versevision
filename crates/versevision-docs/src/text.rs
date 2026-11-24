//! `.txt` / `.md`: `path.read_text(encoding="utf-8")`.
use crate::DocumentError;

/// Python `Path.read_text(encoding="utf-8")`: strict UTF-8 (the BOM stays), then universal newlines (`\r\n` and `\r` become `\n`).
///
/// A decoding failure is Python's `UnicodeDecodeError`; its text is reproduced exactly (`can't decode byte 0xff in position 0: invalid
/// start byte`, `can't decode bytes in position 3-4: unexpected end of data`, `... invalid continuation byte`).
pub fn read_text(bytes: &[u8]) -> Result<String, DocumentError> {
    match std::str::from_utf8(bytes) {
        Ok(text) => Ok(universal_newlines(text)),
        Err(error) => Err(DocumentError::Unexpected(decode_error_message(
            bytes,
            error.valid_up_to(),
        ))),
    }
}

/// `TextIOWrapper(newline=None)`: `\r\n` and a lone `\r` become `\n`.
fn universal_newlines(text: &str) -> String {
    if !text.contains('\r') {
        return text.to_string();
    }
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// `str(UnicodeDecodeError)` of CPython's UTF-8 decoder for the first bad sequence, which starts at `start`.
fn decode_error_message(bytes: &[u8], start: usize) -> String {
    let first = bytes[start];
    let at = |offset: usize| bytes.get(start + offset).copied();
    let continuation =
        |b: Option<u8>, low: u8, high: u8| b.is_some_and(|b| (low..=high).contains(&b));
    // (end of the bad range (exclusive), reason)
    let (end, reason) = match first {
        0xc2..=0xdf => {
            if at(1).is_none() {
                (bytes.len(), "unexpected end of data")
            } else if !continuation(at(1), 0x80, 0xbf) {
                (start + 1, "invalid continuation byte")
            } else {
                (start + 1, "invalid start byte") // not reachable: the sequence is valid
            }
        }
        0xe0..=0xef => {
            let (low, high) = match first {
                0xe0 => (0xa0, 0xbf),
                0xed => (0x80, 0x9f),
                _ => (0x80, 0xbf),
            };
            if at(1).is_none() {
                (bytes.len(), "unexpected end of data")
            } else if !continuation(at(1), low, high) {
                (start + 1, "invalid continuation byte")
            } else if at(2).is_none() {
                (bytes.len(), "unexpected end of data")
            } else {
                (start + 2, "invalid continuation byte")
            }
        }
        0xf0..=0xf4 => {
            let (low, high) = match first {
                0xf0 => (0x90, 0xbf),
                0xf4 => (0x80, 0x8f),
                _ => (0x80, 0xbf),
            };
            if at(1).is_none() {
                (bytes.len(), "unexpected end of data")
            } else if !continuation(at(1), low, high) {
                (start + 1, "invalid continuation byte")
            } else if at(2).is_none() {
                (bytes.len(), "unexpected end of data")
            } else if !continuation(at(2), 0x80, 0xbf) {
                (start + 2, "invalid continuation byte")
            } else if at(3).is_none() {
                (bytes.len(), "unexpected end of data")
            } else {
                (start + 3, "invalid continuation byte")
            }
        }
        _ => (start + 1, "invalid start byte"),
    };
    if end - start == 1 {
        format!("'utf-8' codec can't decode byte 0x{first:02x} in position {start}: {reason}")
    } else {
        format!(
            "'utf-8' codec can't decode bytes in position {start}-{}: {reason}",
            end - 1
        )
    }
}
