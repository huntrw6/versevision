//! `.pdf`: `"\n".join(page.extract_text() or "" for page in PdfReader(path).pages)`, `extract_text` of pypdf 6.14 (plain mode).
//!
//! The file structure (cross-reference tables and streams, object streams, stream filters, encryption with an empty password) is read by
//! `lopdf`; everything that decides the text (the content-stream tokenizer, fonts and encodings, `/ToUnicode` CMaps, the positioning
//! rules that insert spaces and line breaks) is a port of pypdf, in `content`, `font` and `extract`.
use lopdf::{Dictionary, Document, Object, Stream};

use crate::DocumentError;

mod content;
mod data;
mod extract;
mod font;

static NULL: Object = Object::Null;

/// A loaded document with pypdf's way of following references (a reference to nothing is `NullObject`).
pub struct Pdf<'a> {
    doc: &'a Document,
}

impl<'a> Pdf<'a> {
    pub fn deref(&self, object: &'a Object) -> &'a Object {
        match self.doc.dereference(object) {
            Ok((_, resolved)) => resolved,
            Err(_) => &NULL,
        }
    }

    /// `dict[key]`: the value, references followed (`None`: the key is not there).
    pub fn get(&self, dict: &'a Dictionary, key: &str) -> Option<&'a Object> {
        dict.get(key.as_bytes()).ok().map(|o| self.deref(o))
    }

    pub fn has(&self, dict: &Dictionary, key: &str) -> bool {
        dict.get(key.as_bytes()).is_ok()
    }

    /// A name value as text without its slash.
    pub fn name_of(&self, dict: &'a Dictionary, key: &str) -> Option<String> {
        match self.get(dict, key) {
            Some(Object::Name(name)) => Some(String::from_utf8_lossy(name).into_owned()),
            _ => None,
        }
    }

    /// `get_data()` of a stream: its filters applied in order (`filters.decode_stream_data`).
    pub fn stream_data(&self, stream: &Stream) -> Result<Vec<u8>, String> {
        let filters: Vec<&Object> = match stream.dict.get(b"Filter").ok().map(|f| self.deref(f)) {
            None | Some(Object::Null) => return Ok(stream.content.clone()),
            Some(Object::Array(items)) => items.iter().map(|o| self.deref(o)).collect(),
            Some(single) => vec![single],
        };
        let parms: Vec<Option<&Dictionary>> =
            match stream.dict.get(b"DecodeParms").ok().map(|p| self.deref(p)) {
                Some(Object::Array(items)) => items
                    .iter()
                    .map(|o| match self.deref(o) {
                        Object::Dictionary(d) => Some(d),
                        _ => None,
                    })
                    .collect(),
                Some(Object::Dictionary(d)) => vec![Some(d)],
                _ => Vec::new(),
            };
        let mut data = stream.content.clone();
        if data.is_empty() {
            return Ok(data);
        }
        for (index, filter) in filters.iter().enumerate() {
            let Object::Name(name) = filter else {
                return Err(format!("unsupported filter {filter:?}"));
            };
            data = match name.as_slice() {
                b"ASCIIHexDecode" | b"AHx" => ascii_hex_decode(&data)?,
                b"ASCII85Decode" | b"A85" => ascii85_decode(&data)?,
                b"RunLengthDecode" | b"RL" => run_length_decode(&data),
                b"FlateDecode" | b"Fl" | b"LZWDecode" | b"LZW" => {
                    let canonical: &[u8] = if name.starts_with(b"F") {
                        b"FlateDecode"
                    } else {
                        b"LZWDecode"
                    };
                    let mut dict = Dictionary::new();
                    dict.set("Filter", Object::Name(canonical.to_vec()));
                    if let Some(Some(p)) = parms.get(index) {
                        let mut resolved = Dictionary::new();
                        for (key, value) in p.iter() {
                            resolved.set(key.clone(), self.deref(value).clone());
                        }
                        dict.set("DecodeParms", Object::Dictionary(resolved));
                    }
                    Stream::new(dict, data)
                        .decompressed_content()
                        .map_err(|e| e.to_string())?
                }
                other => {
                    return Err(format!(
                        "unsupported filter /{}",
                        String::from_utf8_lossy(other)
                    ))
                }
            };
        }
        Ok(data)
    }
}

const PDF_WHITESPACE: &[u8] = b"\x00\t\n\x0c\r ";

/// `ASCIIHexDecode.decode`: pairs of hex digits up to `>`; a last single digit is dropped; no `>` is an error.
fn ascii_hex_decode(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut pair = String::new();
    for &b in data {
        if b == b'>' {
            return Ok(out);
        }
        if b.is_ascii_whitespace() {
            continue;
        }
        pair.push(b as char);
        if pair.len() == 2 {
            out.push(
                u8::from_str_radix(&pair, 16)
                    .map_err(|_| format!("invalid literal for int() with base 16: {pair:?}"))?,
            );
            pair.clear();
        }
    }
    Err("Unexpected end of data in ASCIIHexDecode".to_string())
}

/// `ASCII85Decode.decode` (`base64.a85decode(adobe=True)`, an end marker that is missing is tolerated).
fn ascii85_decode(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut data = {
        let start = data
            .iter()
            .position(|b| !PDF_WHITESPACE.contains(b))
            .unwrap_or(data.len());
        let end = data
            .iter()
            .rposition(|b| !PDF_WHITESPACE.contains(b))
            .map_or(start, |e| e + 1);
        data[start..end].to_vec()
    };
    if data.len() > 2 && data.ends_with(b">") {
        let last = data.pop().unwrap_or(b'>');
        while data.last().is_some_and(|b| PDF_WHITESPACE.contains(b)) {
            data.pop();
        }
        data.push(last);
    }
    let body: &[u8] = if data.ends_with(b"~>") {
        let inner = &data[..data.len() - 2];
        inner.strip_prefix(b"<~").unwrap_or(inner)
    } else {
        &data
    };
    let mut out = Vec::new();
    let mut group: Vec<u8> = Vec::with_capacity(5);
    for &b in body {
        if PDF_WHITESPACE.contains(&b) {
            continue;
        }
        if b == b'z' && group.is_empty() {
            out.extend_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        if !(b'!'..=b'u').contains(&b) {
            return Err(format!("Non-Ascii85 digit found: {}", b as char));
        }
        group.push(b);
        if group.len() == 5 {
            let value = group
                .iter()
                .fold(0u64, |acc, &d| acc * 85 + (d - b'!') as u64);
            out.extend_from_slice(&(value as u32).to_be_bytes());
            group.clear();
        }
    }
    if !group.is_empty() {
        let n = group.len();
        while group.len() < 5 {
            group.push(b'u');
        }
        let value = group
            .iter()
            .fold(0u64, |acc, &d| acc * 85 + (d - b'!') as u64);
        out.extend_from_slice(&(value as u32).to_be_bytes()[..n - 1]);
    }
    Ok(out)
}

/// `RunLengthDecode.decode`
fn run_length_decode(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let length = data[i];
        i += 1;
        match length {
            128 => break,
            0..=127 => {
                let end = (i + length as usize + 1).min(data.len());
                out.extend_from_slice(&data[i..end]);
                i = end;
            }
            _ => {
                if let Some(&byte) = data.get(i) {
                    out.extend(std::iter::repeat_n(byte, 257 - length as usize));
                }
                i += 1;
            }
        }
    }
    out
}

/// `PdfReader(path).pages`, each `extract_text()`, joined by `\n`; an empty text layer is Python's `DocumentError`.
pub fn extract_pdf(bytes: &[u8]) -> Result<String, DocumentError> {
    if bytes.is_empty() {
        return Err(DocumentError::Unexpected(
            "Cannot read an empty file".to_string(),
        ));
    }
    // pypdf says "Stream has ended unexpectedly" for every file whose cross-reference table it cannot find (text, truncated, not a PDF)
    let doc = Document::load_mem(bytes)
        .map_err(|_| DocumentError::Unexpected("Stream has ended unexpectedly".to_string()))?;
    if doc.is_encrypted() {
        return Err(DocumentError::Unexpected(
            "File has not been decrypted".to_string(),
        ));
    }
    let pdf = Pdf { doc: &doc };
    let pages = page_list(&pdf, &doc)?;
    let mut texts = Vec::with_capacity(pages.len());
    for page in pages {
        texts.push(page_text(&pdf, page).map_err(|e| DocumentError::Unexpected(e.0))?);
    }
    let text = texts.join("\n");
    if text.chars().all(is_python_space) {
        return Err(DocumentError::Document(
            "No usable embedded text was found. OCR is not enabled.".to_string(),
        ));
    }
    Ok(text)
}

/// `str.isspace()` characters (what `str.strip()` removes).
fn is_python_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

fn page_list<'a>(pdf: &Pdf<'a>, doc: &'a Document) -> Result<Vec<&'a Dictionary>, DocumentError> {
    let catalog = doc
        .catalog()
        .map_err(|_| DocumentError::Unexpected("Cannot find Root object in pdf".to_string()))?;
    let Some(Object::Dictionary(root)) = pdf.get(catalog, "Pages") else {
        return Err(DocumentError::Unexpected("'/Pages'".to_string()));
    };
    let mut pages = Vec::new();
    let mut seen = Vec::new();
    flatten(pdf, root, &mut pages, &mut seen, 0);
    Ok(pages)
}

fn flatten<'a>(
    pdf: &Pdf<'a>,
    node: &'a Dictionary,
    pages: &mut Vec<&'a Dictionary>,
    seen: &mut Vec<*const Dictionary>,
    depth: usize,
) {
    if depth > 64 || seen.contains(&(node as *const Dictionary)) {
        return;
    }
    seen.push(node as *const Dictionary);
    let kind = match pdf.name_of(node, "Type") {
        Some(kind) => kind,
        None if pdf.has(node, "Kids") => "Pages".to_string(),
        None => "Page".to_string(),
    };
    match kind.as_str() {
        "Pages" => {
            if let Some(Object::Array(kids)) = pdf.get(node, "Kids") {
                for kid in kids {
                    if let Object::Dictionary(kid) = pdf.deref(kid) {
                        flatten(pdf, kid, pages, seen, depth + 1);
                    }
                }
            }
        }
        "Page" => pages.push(node),
        _ => {}
    }
}

/// `get_inherited(key)`: the page's own value or its nearest ancestor's.
fn inherited<'a>(pdf: &Pdf<'a>, page: &'a Dictionary, key: &str) -> Option<&'a Object> {
    let mut current = page;
    for _ in 0..64 {
        if let Some(value) = pdf.get(current, key) {
            return Some(value);
        }
        match pdf.get(current, "Parent") {
            Some(Object::Dictionary(parent)) => current = parent,
            _ => return None,
        }
    }
    None
}

fn page_text(pdf: &Pdf, page: &Dictionary) -> Result<String, extract::PageError> {
    let Some(Object::Dictionary(resources)) = inherited(pdf, page, "Resources") else {
        return Ok(String::new());
    };
    if resources.is_empty() {
        return Ok(String::new());
    }
    let Some(data) = page_content(pdf, page).map_err(extract::PageError)? else {
        return Ok(String::new());
    };
    extract::extract(pdf, resources, &data, 0)
}

/// `ContentStream(page["/Contents"], pdf, "bytes")`: one stream, or an array of streams joined by line breaks.
fn page_content(pdf: &Pdf, page: &Dictionary) -> Result<Option<Vec<u8>>, String> {
    let Some(contents) = pdf.get(page, "Contents") else {
        return Ok(None);
    };
    match contents {
        Object::Stream(stream) => Ok(Some(pdf.stream_data(stream)?)),
        Object::Array(items) => {
            let mut data = Vec::new();
            for item in items {
                match pdf.deref(item) {
                    Object::Null => continue,
                    Object::Stream(stream) => data.extend_from_slice(&pdf.stream_data(stream)?),
                    _ => {}
                }
                if data.last() != Some(&b'\n') {
                    data.push(b'\n');
                }
            }
            Ok(Some(data))
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_page(content: &str) -> Vec<u8> {
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_string(),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (n, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", n + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
        );
        for offset in offsets {
            out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        out
    }

    #[test]
    fn lines_and_spaces_follow_pypdf() {
        let pdf = one_page("BT /F1 12 Tf 14 TL 50 700 Td (first line) Tj T* (second) Tj ( word) Tj 0 -1 Td [(kern) -300 (ed)] TJ ET");
        assert_eq!(extract_pdf(&pdf).unwrap(), "first line\nsecond wordkern ed");
    }

    #[test]
    fn errors_use_pypdf_messages() {
        assert_eq!(
            extract_pdf(b"").unwrap_err().message(),
            "Cannot read an empty file"
        );
        assert_eq!(
            extract_pdf(b"not a pdf").unwrap_err().message(),
            "Stream has ended unexpectedly"
        );
        let blank = one_page("0.5 g 0 0 10 10 re f");
        let error = extract_pdf(&blank).unwrap_err();
        assert_eq!(
            (error.kind(), error.message().as_str()),
            (
                "document",
                "No usable embedded text was found. OCR is not enabled."
            )
        );
    }

    #[test]
    fn filters_match_python() {
        assert_eq!(ascii_hex_decode(b"48 65 6C6c 6F>").unwrap(), b"Hello");
        assert!(ascii_hex_decode(b"4865").is_err());
        assert_eq!(
            ascii85_decode(b"<~87cURD]i,\"Ebo80~>").unwrap(),
            b"Hello World!"
        );
        assert_eq!(
            run_length_decode(&[2, b'a', b'b', b'c', 254, b'x', 128]),
            b"abcxxx"
        );
    }
}
