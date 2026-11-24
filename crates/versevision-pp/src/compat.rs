//! `presentation/compatibility.py`: writing is allowed only on ProPresenter 19.0.1 with a document version that was proven on a real file.

use crate::error::{PpError, Result};
use crate::pb::Message;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatibilityPolicy {
    pub application_major: u32,
    pub application_minor: u32,
    pub application_patch: u32,
    /// (document major, document build) pairs verified against ProPresenter 19.0.1:
    /// 10 / 26200 (Windows spike, 2026-07-18) and 15 / "" (macOS 15.7.9, the JPL Graphics iMac).
    pub document_versions: Vec<(u32, String)>,
}

impl Default for CompatibilityPolicy {
    fn default() -> Self {
        CompatibilityPolicy {
            application_major: 19,
            application_minor: 0,
            application_patch: 1,
            document_versions: vec![(10, "26200".to_string()), (15, String::new())],
        }
    }
}

impl CompatibilityPolicy {
    /// `validate(presentation)`: `Err` with Python's `ValueError` text when the file is from another ProPresenter.
    pub fn validate(&self, presentation: &Message) -> Result<()> {
        let info = presentation.msg("application_info");
        let app = info.and_then(|i| i.msg("application_version"));
        let document = info.and_then(|i| i.msg("platform_version"));
        let get = |m: Option<&Message>, name: &str| m.map(|m| m.u32(name)).unwrap_or(0);
        let version = (
            get(app, "major_version"),
            get(app, "minor_version"),
            get(app, "patch_version"),
        );
        if version
            != (
                self.application_major,
                self.application_minor,
                self.application_patch,
            )
        {
            return Err(PpError::value(format!(
                "Unsupported ProPresenter version {}.{}.{}",
                version.0, version.1, version.2
            )));
        }
        let major = get(document, "major_version");
        let build = document.map(|d| d.str("build")).unwrap_or("");
        if !self
            .document_versions
            .iter()
            .any(|(m, b)| *m == major && b == build)
        {
            return Err(PpError::value(format!(
                "Unsupported ProPresenter document version {major}/{build}"
            )));
        }
        Ok(())
    }
}
