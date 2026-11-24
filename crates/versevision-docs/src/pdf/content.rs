//! pypdf's content-stream tokenizer (`ContentStream._parse_content_stream`, `read_object` and what they call), with the string
//! operands kept as bytes (`ContentStream(content, pdf, "bytes")`, which is how `extract_text` reads a page).

#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    /// `NumberObject` (`int`) or `FloatObject`.
    Number(f64),
    /// `ByteStringObject`.
    Bytes(Vec<u8>),
    /// `NameObject` (without the slash).
    Name(String),
    /// A `str` made by the extractor itself (the space it inserts inside a `TJ`).
    Text(String),
    Array(Vec<Operand>),
    Dict(Vec<(String, Operand)>),
    Bool(bool),
    Null,
}

impl Operand {
    /// Python `float(operand)` for the operands the text operators read (`None` where Python raises).
    pub fn as_float(&self) -> Option<f64> {
        match self {
            Operand::Number(n) => Some(*n),
            Operand::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }
}

pub struct Operation {
    pub operands: Vec<Operand>,
    pub operator: Vec<u8>,
}

/// `PdfReadError`/`PdfStreamError` while reading a content stream: the page cannot be read.
#[derive(Debug, Clone)]
pub struct ContentError(pub String);

const WHITESPACES: &[u8] = b"\x00\t\n\x0c\r ";

fn is_delimiter(b: u8) -> bool {
    // NameObject.delimiter_pattern = rb"\s+|[\(\)<>\[\]{}/%]" (bytes `\s` is [ \t\n\r\f\v])
    matches!(
        b,
        b' ' | b'\t'
            | b'\n'
            | b'\r'
            | 0x0c
            | 0x0b
            | b'('
            | b')'
            | b'<'
            | b'>'
            | b'['
            | b']'
            | b'{'
            | b'}'
            | b'/'
            | b'%'
    )
}

struct Stream<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Stream<'a> {
    fn read1(&mut self) -> Option<u8> {
        let b = self.data.get(self.pos).copied();
        if b.is_some() {
            self.pos += 1;
        }
        b
    }

    fn peek(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    /// `read_non_whitespace`: the next byte that is not a PDF whitespace (consumed), `None` at the end.
    fn read_non_whitespace(&mut self) -> Option<u8> {
        loop {
            let b = self.read1()?;
            if !WHITESPACES.contains(&b) {
                return Some(b);
            }
        }
    }

    fn unread(&mut self) {
        self.pos -= 1;
    }

    /// Moves to the next byte that is not a whitespace without consuming it.
    fn skip_whitespace(&mut self) {
        if self.read_non_whitespace().is_some() {
            self.unread();
        }
    }
}

/// `ContentStream.operations`.
pub fn parse(data: &[u8]) -> Result<Vec<Operation>, ContentError> {
    let mut stream = Stream { data, pos: 0 };
    let mut operations = Vec::new();
    let mut operands: Vec<Operand> = Vec::new();
    while let Some(peek) = stream.read_non_whitespace() {
        stream.unread();
        if peek.is_ascii_alphabetic() || peek == b'\'' || peek == b'"' {
            let start = stream.pos;
            let mut end = start;
            while end < data.len() && !is_delimiter(data[end]) {
                end += 1;
            }
            if end - start >= 128 {
                return Err(ContentError(format!(
                    "Read stream length of {} exceeds maximum allowed length of 128.",
                    end - start + 1
                )));
            }
            stream.pos = end;
            let operator = data[start..end].to_vec();
            if operator == b"BI" {
                inline_image(&mut stream)?;
                operations.push(Operation {
                    operands: Vec::new(),
                    operator: b"INLINE IMAGE".to_vec(),
                });
                operands.clear();
            } else {
                operations.push(Operation {
                    operands: std::mem::take(&mut operands),
                    operator,
                });
            }
        } else if peek == b'%' {
            while let Some(b) = stream.read1() {
                if b == b'\r' || b == b'\n' {
                    break;
                }
            }
        } else {
            operands.push(read_object(&mut stream, 0)?);
        }
    }
    Ok(operations)
}

fn read_object(stream: &mut Stream, depth: usize) -> Result<Operand, ContentError> {
    if depth > 200 {
        return Err(ContentError("maximum recursion depth exceeded".into()));
    }
    let Some(tok) = stream.peek() else {
        return Err(ContentError("Invalid Elementary Object".into()));
    };
    match tok {
        b'/' => Ok(Operand::Name(read_name(stream))),
        b'<' => {
            if stream.data[stream.pos..].starts_with(b"<<") {
                read_dictionary(stream, depth)
            } else {
                read_hex_string(stream)
            }
        }
        b'[' => read_array(stream, depth),
        b't' | b'f' => read_boolean(stream),
        b'(' => read_string(stream),
        b'n' => read_null(stream),
        b'%' => {
            while let Some(b) = stream.read1() {
                if b == b'\r' || b == b'\n' {
                    break;
                }
            }
            stream.skip_whitespace();
            read_object(stream, depth + 1)
        }
        b'0'..=b'9' | b'+' | b'-' | b'.' => Ok(read_number(stream)),
        other => {
            let message = format!(
                "Invalid Elementary Object starting with {:?} @{}",
                (other as char).to_string(),
                stream.pos
            );
            // pypdf reads on to the next whitespace before raising
            Err(ContentError(message))
        }
    }
}

fn read_name(stream: &mut Stream) -> String {
    // NameObject.read_from_stream: "/" followed by everything up to a delimiter, `#xx` decoded
    stream.read1();
    let start = stream.pos;
    let mut end = start;
    while end < stream.data.len() && !is_delimiter(stream.data[end]) {
        end += 1;
    }
    stream.pos = end;
    let raw = &stream.data[start..end];
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'#' {
            // `int(sin[i + 1:i + 3], 16)`: a single trailing digit counts too
            let hex = &raw[i + 1..raw.len().min(i + 3)];
            if let Some(v) = std::str::from_utf8(hex)
                .ok()
                .filter(|h| !h.is_empty() && h.bytes().all(|b| b.is_ascii_hexdigit()))
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(raw[i]);
        i += 1;
    }
    match String::from_utf8(out.clone()) {
        Ok(s) => s,
        Err(_) => out.iter().map(|&b| b as char).collect(),
    }
}

fn read_number(stream: &mut Stream) -> Operand {
    // NumberObject.NumberPattern = [^+-.0-9]: '+', ',', '-', '.' and the digits belong to the number
    let start = stream.pos;
    let mut end = start;
    while end < stream.data.len()
        && matches!(stream.data[end], b'+'..=b'.' | b'0'..=b'9')
        && end - start < 64
    {
        end += 1;
    }
    stream.pos = end;
    let text = &stream.data[start..end];
    let text = std::str::from_utf8(text).unwrap_or("");
    if text.contains('.') {
        Operand::Number(text.parse::<f64>().unwrap_or(0.0))
    } else {
        let digits = text.strip_prefix('+').unwrap_or(text);
        Operand::Number(digits.parse::<i128>().map(|n| n as f64).unwrap_or(0.0))
    }
}

fn read_boolean(stream: &mut Stream) -> Result<Operand, ContentError> {
    let rest = &stream.data[stream.pos..];
    if rest.starts_with(b"true") {
        stream.pos += 4;
        Ok(Operand::Bool(true))
    } else if rest.starts_with(b"false") {
        stream.pos += 5;
        Ok(Operand::Bool(false))
    } else {
        Err(ContentError("Could not read Boolean object".into()))
    }
}

fn read_null(stream: &mut Stream) -> Result<Operand, ContentError> {
    if stream.data[stream.pos..].starts_with(b"null") {
        stream.pos += 4;
        Ok(Operand::Null)
    } else {
        Err(ContentError("Could not read Null object".into()))
    }
}

fn read_array(stream: &mut Stream, depth: usize) -> Result<Operand, ContentError> {
    stream.read1(); // [
    let mut items = Vec::new();
    loop {
        let Some(tok) = stream.read_non_whitespace() else {
            return Err(ContentError("Stream has ended unexpectedly".into()));
        };
        if tok == b']' {
            break;
        }
        stream.unread();
        items.push(read_object(stream, depth + 1)?);
    }
    Ok(Operand::Array(items))
}

fn read_dictionary(stream: &mut Stream, depth: usize) -> Result<Operand, ContentError> {
    stream.pos += 2; // <<
    let mut entries: Vec<(String, Operand)> = Vec::new();
    loop {
        let Some(tok) = stream.read_non_whitespace() else {
            return Err(ContentError("Stream has ended unexpectedly".into()));
        };
        if tok == b'>' {
            stream.read1(); // the second '>'
            break;
        }
        stream.unread();
        if tok == b'%' {
            while let Some(b) = stream.read1() {
                if b == b'\r' || b == b'\n' {
                    break;
                }
            }
            continue;
        }
        let key = match read_object(stream, depth + 1)? {
            Operand::Name(name) => name,
            _ => return Err(ContentError("Dictionary key is not a name".into())),
        };
        stream.skip_whitespace();
        let value = read_object(stream, depth + 1)?;
        if let Some(existing) = entries.iter_mut().find(|(k, _)| *k == key) {
            existing.1 = value;
        } else {
            entries.push((key, value));
        }
    }
    Ok(Operand::Dict(entries))
}

fn read_hex_string(stream: &mut Stream) -> Result<Operand, ContentError> {
    stream.read1(); // <
    let mut out = Vec::new();
    let mut pending: Option<u8> = None;
    loop {
        let Some(tok) = stream.read_non_whitespace() else {
            return Err(ContentError("Stream has ended unexpectedly".into()));
        };
        if tok == b'>' {
            break;
        }
        let Some(digit) = (tok as char).to_digit(16) else {
            return Err(ContentError(format!(
                "Invalid hexadecimal character {:?} in hex string",
                (tok as char).to_string()
            )));
        };
        match pending.take() {
            Some(high) => out.push(high * 16 + digit as u8),
            None => pending = Some(digit as u8),
        }
    }
    if let Some(high) = pending {
        out.push(high * 16);
    }
    Ok(Operand::Bytes(out))
}

fn read_string(stream: &mut Stream) -> Result<Operand, ContentError> {
    stream.read1(); // (
    let mut parens = 1;
    let mut out = Vec::new();
    loop {
        let Some(tok) = stream.read1() else {
            return Err(ContentError("Stream has ended unexpectedly".into()));
        };
        match tok {
            b'(' => {
                parens += 1;
                out.push(tok);
            }
            b')' => {
                parens -= 1;
                if parens == 0 {
                    break;
                }
                out.push(tok);
            }
            b'\\' => {
                let Some(esc) = stream.read1() else {
                    // `stream.read(1)` is b"": not an escape, not an octal digit; `tok in b"\n\r"` is true for b"" ->
                    // reads again (b"") and `b"" not in b"\n\r"` is false: nothing appended, the loop ends with the error above
                    continue;
                };
                match esc {
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    b'b' => out.push(0x08),
                    b'f' => out.push(0x0c),
                    b'(' | b')' | b'/' | b'\\' | b' ' | b'%' | b'<' | b'>' | b'[' | b']' | b'#'
                    | b'_' | b'&' | b'$' => out.push(esc),
                    b'0'..=b'7' => {
                        let saved = stream.pos - 1;
                        let mut value = (esc - b'0') as u32;
                        let mut digits = 1;
                        for _ in 0..2 {
                            match stream.peek() {
                                Some(d @ b'0'..=b'7') => {
                                    stream.pos += 1;
                                    value = value * 8 + (d - b'0') as u32;
                                    digits += 1;
                                }
                                _ => break,
                            }
                        }
                        let _ = digits;
                        if value > 255 {
                            out.push(b'\\');
                            stream.pos = saved;
                        } else {
                            out.push(value as u8);
                        }
                    }
                    b'\n' | b'\r' => {
                        // an escaped line break: consumed (a CR LF pair as one), nothing appended
                        if let Some(next) = stream.peek() {
                            if next == b'\n' || next == b'\r' {
                                stream.pos += 1;
                            }
                        }
                    }
                    other => {
                        out.push(b'\\');
                        out.push(other);
                    }
                }
            }
            other => out.push(other),
        }
    }
    Ok(Operand::Bytes(out))
}

/// `ContentStream._read_inline_image` (after `BI`): the image is skipped; the data ends at `EI`.
fn inline_image(stream: &mut Stream) -> Result<(), ContentError> {
    let mut settings: Vec<(String, Operand)> = Vec::new();
    loop {
        let Some(tok) = stream.read_non_whitespace() else {
            return Err(ContentError("Unexpected end of stream.".into()));
        };
        stream.unread();
        if tok == b'I' {
            break;
        }
        let key = match read_object(stream, 0)? {
            Operand::Name(name) => name,
            _ => String::new(),
        };
        stream.skip_whitespace();
        let value = read_object(stream, 0)?;
        settings.push((key, value));
    }
    stream.pos = (stream.pos + 3).min(stream.data.len()); // "ID" and the one byte after it
    let get = |names: &[&str]| {
        settings
            .iter()
            .rev()
            .find(|(k, _)| names.contains(&k.as_str()))
            .map(|(_, v)| v.clone())
    };
    let filter = get(&["F", "Filter"]);
    let has_filter = match &filter {
        Some(Operand::Name(_)) => true,
        Some(Operand::Array(items)) => !items.is_empty(),
        _ => false,
    };
    let start = stream.pos;
    if !has_filter {
        let colour_space = match get(&["CS", "ColorSpace"]) {
            Some(Operand::Array(items)) => items.first().cloned(),
            other => other,
        };
        let cs = match colour_space {
            Some(Operand::Name(name)) => name,
            _ => String::new(),
        };
        let lcs: f64 = if cs.contains("RGB") {
            3.0
        } else if cs.contains("CMYK") {
            4.0
        } else {
            let bits = get(&["BPC", "BitsPerComponent"])
                .and_then(|v| v.as_float())
                .unwrap_or(
                    if matches!(cs.as_str(), "I" | "G" | "Indexed" | "DeviceGray") {
                        8.0
                    } else {
                        -1.0
                    },
                );
            if bits > 0.0 {
                bits / 8.0
            } else {
                -1.0
            }
        };
        let width = get(&["W", "Width"]).and_then(|v| v.as_float());
        let height = get(&["H", "Height"]).and_then(|v| v.as_float());
        if let (true, Some(w), Some(h)) = (lcs > 0.0, width, height) {
            let length = ((w * lcs).ceil() * h) as usize;
            stream.pos = (stream.pos + length).min(stream.data.len());
            stream.skip_whitespace();
            if end_marker(stream) {
                return Ok(());
            }
            stream.pos = start;
        }
    }
    default_scan(stream)?;
    Ok(())
}

/// Consumes `EI` + one whitespace byte when that is next.
fn end_marker(stream: &mut Stream) -> bool {
    let rest = &stream.data[stream.pos..];
    if rest.starts_with(b"EI") && (rest.len() == 2 || WHITESPACES.contains(&rest[2])) {
        stream.pos += 2;
        true
    } else {
        false
    }
}

/// `extract_inline_default`: reads to the `EI` that ends the image.
fn default_scan(stream: &mut Stream) -> Result<(), ContentError> {
    let data = stream.data;
    let mut i = stream.pos;
    loop {
        let Some(offset) = data[i..].iter().position(|&b| b == b'E') else {
            return Err(ContentError("Unexpected end of stream.".into()));
        };
        let e = i + offset;
        if data.get(e + 1) != Some(&b'I') {
            i = e + 1;
            continue;
        }
        match data.get(e + 2) {
            None => {
                stream.pos = e + 2;
                return Ok(());
            }
            Some(b) if !WHITESPACES.contains(b) => {
                i = e + 1;
                continue;
            }
            _ => {}
        }
        let mut j = e + 2;
        while j < data.len() && WHITESPACES.contains(&data[j]) {
            j += 1;
        }
        let before_is_whitespace = e > 0 && WHITESPACES.contains(&data[e - 1]);
        let next = data.get(j).copied();
        if !before_is_whitespace && !matches!(next, Some(b'Q') | Some(b'E')) {
            i = e + 1;
            continue;
        }
        if followed_by_binary_data(&data[j.min(data.len())..]) {
            i = e + 1;
            continue;
        }
        stream.pos = e + 2;
        return Ok(());
    }
}

fn followed_by_binary_data(rest: &[u8]) -> bool {
    let data = &rest[..rest.len().min(10)];
    if data.is_empty() {
        return false;
    }
    let mut start = None;
    let mut end = None;
    for (index, &byte) in data.iter().enumerate() {
        if byte < 32 && !WHITESPACES.contains(&byte) {
            return true;
        }
        let whitespace = WHITESPACES.contains(&byte);
        if start.is_none() && !whitespace {
            start = Some(index);
        }
        if start.is_some() && whitespace {
            end = Some(index);
            break;
        }
    }
    let Some(start) = start else { return false };
    let end = end.unwrap_or(10);
    let operator = &data[start..end.min(data.len())];
    let length = end - start;
    if operator.starts_with(b"/") && length > 1 {
        return false;
    }
    let stripped: Vec<u8> = operator.iter().copied().filter(|&b| b != b'.').collect();
    if !stripped.is_empty() && stripped.iter().all(u8::is_ascii_digit) {
        return false;
    }
    length > 3
}
