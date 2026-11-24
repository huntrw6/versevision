//! VerseVision document readers: the bytes of a .txt, .md, .html, .htm, .docx or .pdf to exactly the text Python's
//! `versevision.documents.extract_text` returns (S4), with the `extract` trace record and the error messages.
//! Pure Rust: no network, no system libraries, no external programs.

pub mod docx;
pub mod error;
pub mod html;
pub mod pdf;
pub mod text;

pub use error::DocumentError;

use versevision_core::pystr;
use versevision_core::references::pytext::is_alpha;

/// The web layer's upload limit (`MAX_UPLOAD` of `web/server.py`): 25 MB.
pub const MAX_UPLOAD_BYTES: usize = 25 * 1024 * 1024;

/// The file types the web layer accepts (`ALLOWED` of `web/server.py`).
pub const ALLOWED_SUFFIXES: [&str; 6] = [".pdf", ".docx", ".txt", ".md", ".html", ".htm"];

/// The message the web layer shows for an upload of another type (`web/server.py::_plan_work`).
pub fn unsupported_upload_message(suffix: &str) -> String {
    let shown = if suffix.is_empty() { "(none)" } else { suffix };
    format!("Unsupported file type {shown}. Use PDF, Word (.docx), text, Markdown or HTML.")
}

/// How the document was read: the fields of the run trace's `extract` record (`documents/base.py::extract_text`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractInfo {
    /// The lower-cased file extension with its dot (`.pdf`).
    pub doc_type: String,
    pub method: String,
    pub characters: usize,
    pub lines: usize,
    pub blank_lines: usize,
    /// Up to 40 characters above U+007F that are not letters, in code point order.
    pub odd_characters: Vec<char>,
}

impl ExtractInfo {
    /// The record as the oracle writes it (`seq` and `stage` are the trace's own).
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "type": self.doc_type,
            "method": self.method,
            "characters": self.characters,
            "lines": self.lines,
            "blank_lines": self.blank_lines,
            "odd_characters": self.odd_characters.iter().map(char::to_string).collect::<Vec<_>>(),
        })
    }
}

/// `Path(name).suffix.casefold()` (CPython 3.13: a name that starts or ends with its only dot has no suffix).
pub fn suffix_of(file_name: &str) -> String {
    let trimmed = file_name.trim_end_matches('/');
    let name = trimmed.rsplit('/').next().unwrap_or("");
    match name.rfind('.') {
        Some(i) if i > 0 && i < name.len() - 1 => {
            versevision_core::references::pytext::casefold(&name[i..])
        }
        _ => String::new(),
    }
}

/// `documents.extract_text(path)` for the bytes of the file `file_name`: the text and how it was read. Over the 25 MB upload limit
/// the web layer refuses the file before any reader runs.
pub fn extract_text(bytes: &[u8], file_name: &str) -> Result<(String, ExtractInfo), DocumentError> {
    if bytes.len() > MAX_UPLOAD_BYTES {
        return Err(DocumentError::TooLarge);
    }
    let suffix = suffix_of(file_name);
    let raw = match suffix.as_str() {
        ".txt" | ".md" => text::read_text(bytes)?,
        ".html" | ".htm" => html::extract_html(&text::read_text(bytes)?),
        ".docx" => docx::extract_docx(bytes)?,
        ".pdf" => pdf::extract_pdf(bytes)?,
        other => {
            let shown = if other.is_empty() { "(none)" } else { other };
            return Err(DocumentError::Document(format!(
                "Unsupported document type: {shown}"
            )));
        }
    };
    let text = translate_ligatures(&raw);
    let lines = pystr::splitlines(&text);
    let method = match suffix.as_str() {
        ".txt" | ".md" => "plain text",
        ".html" => "BeautifulSoup get_text (one line per block)",
        ".htm" => "BeautifulSoup get_text",
        ".docx" => {
            "python-docx: one line per Word paragraph (tables, headers and text boxes are not read)"
        }
        _ => "pypdf extract_text (the text layer; lines are cut where the PDF cut them)",
    };
    let mut odd: Vec<char> = text
        .chars()
        .filter(|&c| (c as u32) > 127 && !is_alpha(c))
        .collect();
    odd.sort_unstable();
    odd.dedup();
    odd.truncate(40);
    let info = ExtractInfo {
        doc_type: suffix,
        method: method.to_string(),
        characters: text.chars().count(),
        lines: lines.len(),
        blank_lines: lines.iter().filter(|l| pystr::strip(l).is_empty()).count(),
        odd_characters: odd,
    };
    Ok((text, info))
}

/// `str.translate(_LIGATURES)`: "PDFs store \"fi\" as one character".
fn translate_ligatures(text: &str) -> String {
    if !text.chars().any(|c| ('\u{fb00}'..='\u{fb06}').contains(&c)) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\u{fb00}' => out.push_str("ff"),
            '\u{fb01}' => out.push_str("fi"),
            '\u{fb02}' => out.push_str("fl"),
            '\u{fb03}' => out.push_str("ffi"),
            '\u{fb04}' => out.push_str("ffl"),
            '\u{fb05}' | '\u{fb06}' => out.push_str("st"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix_follows_pathlib() {
        assert_eq!(suffix_of("Notes.PDF"), ".pdf");
        assert_eq!(suffix_of("a/b/c.tar.GZ"), ".gz");
        assert_eq!(suffix_of(".bashrc"), "");
        assert_eq!(suffix_of("name."), "");
        assert_eq!(suffix_of("dir.d/name"), "");
        assert_eq!(suffix_of(""), "");
    }

    #[test]
    fn unsupported_and_too_large_messages() {
        assert_eq!(
            extract_text(b"x", "a.rtf").unwrap_err().message(),
            "Unsupported document type: .rtf"
        );
        assert_eq!(
            extract_text(b"x", "noext").unwrap_err().message(),
            "Unsupported document type: (none)"
        );
        let big = vec![b'a'; MAX_UPLOAD_BYTES + 1];
        let error = extract_text(&big, "a.txt").unwrap_err();
        assert_eq!(error.message(), "That file is too large (limit 25 MB)");
        assert_eq!(error.kind(), "document");
        assert_eq!(
            unsupported_upload_message(""),
            "Unsupported file type (none). Use PDF, Word (.docx), text, Markdown or HTML."
        );
    }

    #[test]
    fn text_info_counts_like_python() {
        let (text, info) = extract_text(
            "a\r\n\u{fb01}ne \u{2014} caf\u{e9}\n  \n".as_bytes(),
            "x.TXT",
        )
        .unwrap();
        assert_eq!(text, "a\nfine \u{2014} caf\u{e9}\n  \n");
        assert_eq!((info.characters, info.lines, info.blank_lines), (17, 3, 1));
        assert_eq!(info.doc_type, ".txt");
        assert_eq!(info.odd_characters, vec!['\u{2014}']);
        assert_eq!(info.to_json()["method"], "plain text");
    }
}
