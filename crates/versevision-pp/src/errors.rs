//! `errors.py`: the errors of a run, with the exit codes the command line gives them. The text of each message is what the Python app
//! shows; the kind says which Python exception it was (`ProPresenterNotFound` is a kind of connection error, `NotLoaded` a kind of
//! verification error, as there).

use std::fmt;

use crate::error::PpError;

/// `ExitCode` of `errors.py`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ExitCode {
    Success = 0,
    GeneralFailure = 1,
    InvalidArguments = 2,
    ParsingOrValidation = 3,
    BibleProvider = 4,
    ProPresenterConnection = 5,
    UnsupportedProPresenter = 6,
    PresentationWrite = 7,
    VerificationRollback = 8,
    UserCancellation = 9,
    CompletedWithFlags = 10,
}

/// Which Python exception this was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `VerseVisionError` itself.
    General,
    Document,
    ReferenceValidation,
    Configuration,
    BibleProvider,
    ProPresenterConnection,
    /// `ProPresenterNotFound(ProPresenterConnectionError)`: ProPresenter answered 404.
    ProPresenterNotFound,
    Unsupported,
    PresentationWrite,
    VerificationRollback,
    /// `_NotLoaded(VerificationRollbackError)`: ProPresenter lists a presentation but answers 404 for it.
    NotLoaded,
    /// `ThemeError(VerseVisionError)`.
    Theme,
    /// A Python exception that is not a `VerseVisionError` (`ValueError`, `OSError`, ...): exit code 1.
    Other(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VvError {
    pub kind: Kind,
    pub message: String,
}

pub type VvResult<T> = Result<T, VvError>;

impl VvError {
    pub fn new(kind: Kind, message: impl Into<String>) -> Self {
        VvError {
            kind,
            message: message.into(),
        }
    }
    pub fn general(message: impl Into<String>) -> Self {
        Self::new(Kind::General, message)
    }
    pub fn configuration(message: impl Into<String>) -> Self {
        Self::new(Kind::Configuration, message)
    }
    pub fn connection(message: impl Into<String>) -> Self {
        Self::new(Kind::ProPresenterConnection, message)
    }
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(Kind::ProPresenterNotFound, message)
    }
    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(Kind::Unsupported, message)
    }
    pub fn write(message: impl Into<String>) -> Self {
        Self::new(Kind::PresentationWrite, message)
    }
    pub fn rollback(message: impl Into<String>) -> Self {
        Self::new(Kind::VerificationRollback, message)
    }
    pub fn not_loaded(message: impl Into<String>) -> Self {
        Self::new(Kind::NotLoaded, message)
    }
    /// `isinstance(error, ProPresenterConnectionError)`.
    pub fn is_connection(&self) -> bool {
        matches!(
            self.kind,
            Kind::ProPresenterConnection | Kind::ProPresenterNotFound
        )
    }
    pub fn is_not_found(&self) -> bool {
        self.kind == Kind::ProPresenterNotFound
    }
    pub fn is_configuration(&self) -> bool {
        self.kind == Kind::Configuration
    }
    /// `exit_code` of the Python class (1 for an exception that is not a `VerseVisionError`).
    pub fn exit_code(&self) -> ExitCode {
        match self.kind {
            Kind::Configuration => ExitCode::InvalidArguments,
            Kind::ReferenceValidation => ExitCode::ParsingOrValidation,
            Kind::BibleProvider => ExitCode::BibleProvider,
            Kind::ProPresenterConnection | Kind::ProPresenterNotFound => {
                ExitCode::ProPresenterConnection
            }
            Kind::Unsupported => ExitCode::UnsupportedProPresenter,
            Kind::PresentationWrite => ExitCode::PresentationWrite,
            Kind::VerificationRollback | Kind::NotLoaded => ExitCode::VerificationRollback,
            _ => ExitCode::GeneralFailure,
        }
    }
    /// The Python class name (`type(exc).__name__`).
    pub fn python_name(&self) -> &'static str {
        match self.kind {
            Kind::General => "VerseVisionError",
            Kind::Document => "DocumentError",
            Kind::ReferenceValidation => "ReferenceValidationError",
            Kind::Configuration => "ConfigurationError",
            Kind::BibleProvider => "BibleProviderError",
            Kind::ProPresenterConnection => "ProPresenterConnectionError",
            Kind::ProPresenterNotFound => "ProPresenterNotFound",
            Kind::Unsupported => "UnsupportedProPresenterError",
            Kind::PresentationWrite => "PresentationWriteError",
            Kind::VerificationRollback => "VerificationRollbackError",
            Kind::NotLoaded => "_NotLoaded",
            Kind::Theme => "ThemeError",
            Kind::Other(name) => name,
        }
    }
    /// `"<ExceptionName>: <message>"`, the form the oracle records.
    pub fn python_form(&self) -> String {
        format!("{}: {}", self.python_name(), self.message)
    }
}

impl fmt::Display for VvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for VvError {}

impl From<PpError> for VvError {
    fn from(error: PpError) -> Self {
        let message = error.to_string();
        match error {
            PpError::Configuration(_) => VvError::configuration(message),
            PpError::Theme(_) => VvError::new(Kind::Theme, message),
            PpError::Value(_) | PpError::Pb(_) => VvError::new(Kind::Other("ValueError"), message),
            PpError::Io(_) => VvError::new(Kind::Other("OSError"), message),
        }
    }
}

impl From<std::io::Error> for VvError {
    fn from(error: std::io::Error) -> Self {
        VvError::new(Kind::Other("OSError"), error.to_string())
    }
}

impl From<versevision_core::plan::PlanError> for VvError {
    fn from(error: versevision_core::plan::PlanError) -> Self {
        use versevision_core::plan::PlanError;
        match &error {
            PlanError::UnknownVersion(_) => VvError::general(error.to_string()),
            PlanError::Classify(_) => VvError::new(Kind::Other("ValueError"), error.to_string()),
            _ => VvError::new(Kind::Other("ValueError"), error.to_string()),
        }
    }
}
