//! What a document reader can fail with: the exact message Python shows.

/// The Python side raises three kinds of failure and the web layer shows them differently
/// (`web/server.py::_spawn`): a `VerseVisionError` (message as is) or any other exception (`Unexpected problem: <message>`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DocumentError {
    /// `versevision.errors.DocumentError`: "Unsupported document type: .xyz", "No usable embedded text was found. OCR is not enabled.".
    #[error("{0}")]
    Document(String),
    /// `VerseVisionError("That file is too large (limit 25 MB)")` of the upload guard.
    #[error("That file is too large (limit 25 MB)")]
    TooLarge,
    /// Any other Python exception (`UnicodeDecodeError`, python-docx `PackageNotFoundError`, a `KeyError` inside pypdf ...):
    /// the text is `str(exc)`; the web page shows it as `Unexpected problem: <text>`.
    #[error("{0}")]
    Unexpected(String),
}

impl DocumentError {
    /// The message exactly as Python's `str(exc)`.
    pub fn message(&self) -> String {
        self.to_string()
    }

    /// `"document"` (a `VerseVisionError`) or `"unexpected"` (any other exception): the `kind` of the oracle's error files.
    pub fn kind(&self) -> &'static str {
        match self {
            DocumentError::Document(_) | DocumentError::TooLarge => "document",
            DocumentError::Unexpected(_) => "unexpected",
        }
    }
}
