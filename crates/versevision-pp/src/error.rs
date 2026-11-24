//! The errors of this crate. The text is what the Python app shows (`ValueError`, `ConfigurationError`, `ThemeError`), so a
//! failure reads the same on both sides.

use thiserror::Error;

use crate::pb::PbError;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PpError {
    /// Python `ValueError(message)`.
    #[error("{0}")]
    Value(String),
    /// `ConfigurationError(message)`.
    #[error("{0}")]
    Configuration(String),
    /// `ThemeError(message)`.
    #[error("{0}")]
    Theme(String),
    #[error("{0}")]
    Pb(#[from] PbError),
    #[error("{0}")]
    Io(String),
}

impl PpError {
    pub fn value(message: impl Into<String>) -> Self {
        PpError::Value(message.into())
    }
    pub fn configuration(message: impl Into<String>) -> Self {
        PpError::Configuration(message.into())
    }
    pub fn theme(message: impl Into<String>) -> Self {
        PpError::Theme(message.into())
    }
    pub fn io(error: impl std::fmt::Display) -> Self {
        PpError::Io(error.to_string())
    }
}

impl PpError {
    /// `"<Python exception name>: <message>"`, the form the oracle records.
    pub fn python_form(&self) -> String {
        let name = match self {
            PpError::Value(_) | PpError::Pb(_) | PpError::Io(_) => "ValueError",
            PpError::Configuration(_) => "ConfigurationError",
            PpError::Theme(_) => "ThemeError",
        };
        format!("{name}: {self}")
    }
}

pub type Result<T> = std::result::Result<T, PpError>;
