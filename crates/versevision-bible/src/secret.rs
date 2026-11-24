//! `Secret`: a string that never prints itself.

use std::fmt;

/// An API key (or anything else that must not leak). `Debug` prints `Secret(***)`, there is no `Display`, no `Serialize`, and
/// the only way to read the text is the explicit [`Secret::expose`], which the providers call when they build a request header.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The text. Only for putting it in a request header (and the tests); never format it.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// `text` with every copy of the secret replaced by `***` (used on text that came from outside, such as a transport error).
    pub fn redact(&self, text: &str) -> String {
        if self.0.is_empty() {
            text.to_string()
        } else {
            text.replace(&self.0, "***")
        }
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for Secret {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}
