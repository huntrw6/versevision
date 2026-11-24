//! `propresenter/{api_client,models}.py`: the typed client of ProPresenter's network API (version, libraries, presentations, playlists).
//! Used by the older `import` command and the dry run; the swap pipeline talks to the API through [`crate::transport::Transport`].
//!
//! Python validates every answer with pydantic and shows its error text ("Malformed library response: 1 validation error for ...").
//! The wording after the colon is serde's here, the rest of each message is the same.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::errors::{VvError, VvResult};
use crate::http::HttpApi;

/// `urllib.parse.quote(text)` (safe: `/`).
pub fn quote(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"_.-~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProPresenterVersion {
    pub name: String,
    pub platform: String,
    pub os_version: String,
    pub host_description: String,
    pub api_version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Library {
    pub uuid: String,
    pub name: String,
    pub index: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresentationSummary {
    pub uuid: String,
    pub name: String,
    pub index: i64,
    #[serde(default)]
    pub library_uuid: Option<String>,
    #[serde(default)]
    pub library_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LibraryContents {
    #[serde(default)]
    pub update_type: Option<String>,
    #[serde(default)]
    pub items: Vec<PresentationSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Identifier {
    pub uuid: String,
    pub name: String,
    #[serde(default)]
    pub index: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SlideSize {
    pub width: f64,
    pub height: f64,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiSlide {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub size: Option<SlideSize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiGroup {
    pub name: String,
    #[serde(default)]
    pub color: Option<Map<String, Value>>,
    #[serde(default)]
    pub slides: Vec<ApiSlide>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresentationMetadata {
    pub id: Identifier,
    #[serde(default)]
    pub groups: Vec<ApiGroup>,
    #[serde(default)]
    pub has_timeline: bool,
    pub presentation_path: PathBuf,
    #[serde(default)]
    pub destination: Option<String>,
    #[serde(default)]
    pub has_timeline_items: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct PresentationResponse {
    presentation: PresentationMetadata,
}

/// A playlist or a folder of playlists; any other key ProPresenter sends is kept (`extra="allow"`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaylistItem {
    #[serde(default)]
    pub id: Option<Identifier>,
    #[serde(default)]
    pub field_type: Option<String>,
    #[serde(default)]
    pub children: Vec<PlaylistItem>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `ProPresenterApiClient`.
#[derive(Debug, Clone)]
pub struct ProPresenterApiClient {
    api: HttpApi,
}

fn malformed<T>(what: &str, error: serde_json::Error) -> VvResult<T> {
    Err(VvError::connection(format!("Malformed {what}: {error}")))
}

impl ProPresenterApiClient {
    pub fn new(host: &str, port: u16, timeout_seconds: f64) -> Self {
        ProPresenterApiClient {
            api: HttpApi::new(host, port, timeout_seconds),
        }
    }

    pub fn base_url(&self) -> String {
        self.api.base_url()
    }

    pub fn version(&self) -> VvResult<ProPresenterVersion> {
        let raw = self.api.client_get("/version")?;
        let version: ProPresenterVersion = match serde_json::from_value(raw) {
            Ok(v) => v,
            Err(e) => return malformed("ProPresenter version response", e),
        };
        if version.api_version.to_lowercase() != "v1" {
            return Err(VvError::unsupported(format!(
                "Unsupported ProPresenter API version: {}. No files were changed.",
                version.api_version
            )));
        }
        Ok(version)
    }

    pub fn libraries(&self) -> VvResult<Vec<Library>> {
        let raw = self.api.client_get("/v1/libraries")?;
        match serde_json::from_value(raw) {
            Ok(v) => Ok(v),
            Err(e) => malformed("library response", e),
        }
    }

    pub fn library_presentations(&self, library: &Library) -> VvResult<Vec<PresentationSummary>> {
        let raw = self
            .api
            .client_get(&format!("/v1/library/{}", quote(&library.uuid)))?;
        let contents: LibraryContents = match serde_json::from_value(raw) {
            Ok(v) => v,
            Err(e) => return malformed(&format!("presentation listing for {}", library.name), e),
        };
        Ok(contents
            .items
            .into_iter()
            .map(|mut item| {
                item.library_uuid = Some(library.uuid.clone());
                item.library_name = Some(library.name.clone());
                item
            })
            .collect())
    }

    pub fn presentations(&self) -> VvResult<Vec<PresentationSummary>> {
        let mut found = Vec::new();
        for library in self.libraries()? {
            found.extend(self.library_presentations(&library)?);
        }
        Ok(found)
    }

    pub fn presentation(&self, uuid: &str) -> VvResult<PresentationMetadata> {
        let raw = self
            .api
            .client_get(&format!("/v1/presentation/{}", quote(uuid)))?;
        match serde_json::from_value::<PresentationResponse>(raw) {
            Ok(v) => Ok(v.presentation),
            Err(e) => malformed(&format!("presentation response for {uuid}"), e),
        }
    }

    pub fn playlists(&self) -> VvResult<Vec<PlaylistItem>> {
        let raw = self.api.client_get("/v1/playlists")?;
        match serde_json::from_value(raw) {
            Ok(v) => Ok(v),
            Err(e) => malformed("playlist response", e),
        }
    }

    pub fn playlist(&self, uuid: &str) -> VvResult<PlaylistItem> {
        let raw = self
            .api
            .client_get(&format!("/v1/playlist/{}", quote(uuid)))?;
        match serde_json::from_value(raw) {
            Ok(v) => Ok(v),
            Err(e) => malformed(&format!("playlist response for {uuid}"), e),
        }
    }

    /// Replace a playlist with a complete API-derived model, then verify it.
    pub fn replace_playlist(&self, uuid: &str, playlist: &PlaylistItem) -> VvResult<PlaylistItem> {
        let body = serde_json::to_value(playlist).unwrap_or(Value::Null);
        self.api
            .client_put(&format!("/v1/playlist/{}", quote(uuid)), &body)?;
        self.playlist(uuid)
    }
}
