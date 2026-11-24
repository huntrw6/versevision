//! The config model: the same sections, field names, defaults, enum words and ranges as `python/src/versevision/config.py`.
//!
//! The rules live in [`schema`] (data) and [`validate`] (pydantic's checks and messages); the structs here are what the
//! rest of the program reads. Every way in (a TOML file, `with_options`, `Default`) goes through the validator, so a value
//! outside its range cannot exist in an [`AppConfig`].

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

mod options;
mod pynum;
pub mod schema;
mod toml_io;
pub mod validate;

pub use options::{changed_fields, with_options, ConfigError};
pub use validate::ValidationError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReferenceLabelMode {
    #[serde(rename = "source")]
    Source,
    #[serde(rename = "segment")]
    Segment,
    #[serde(rename = "verse")]
    Verse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeType {
    #[serde(rename = "local")]
    Local,
    #[serde(rename = "hermes-remote")]
    HermesRemote,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Handling {
    #[serde(rename = "bullet")]
    Bullet,
    #[serde(rename = "review")]
    Review,
    #[serde(rename = "skip")]
    Skip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LineBreaks {
    #[serde(rename = "slide")]
    Slide,
    #[serde(rename = "paragraph")]
    Paragraph,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QuoteAboveReference {
    #[serde(rename = "skip")]
    Skip,
    #[serde(rename = "review")]
    Review,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReferenceInText {
    #[serde(rename = "review")]
    Review,
    #[serde(rename = "skip")]
    Skip,
    #[serde(rename = "verse")]
    Verse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WholeChapter {
    #[serde(rename = "review")]
    Review,
    #[serde(rename = "skip")]
    Skip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Duplicates {
    #[serde(rename = "keep")]
    Keep,
    #[serde(rename = "drop")]
    Drop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Headings {
    #[serde(rename = "slide")]
    Slide,
    #[serde(rename = "skip")]
    Skip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CapsLines {
    #[serde(rename = "heading")]
    Heading,
    #[serde(rename = "line")]
    Line,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WrappedLines {
    #[serde(rename = "join")]
    Join,
    #[serde(rename = "separate")]
    Separate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Numbering {
    #[serde(rename = "keep")]
    Keep,
    #[serde(rename = "strip")]
    Strip,
    #[serde(rename = "renumber")]
    Renumber,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CaseMode {
    #[serde(rename = "as_written")]
    AsWritten,
    #[serde(rename = "upper")]
    Upper,
    #[serde(rename = "lower")]
    Lower,
    #[serde(rename = "title")]
    Title,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VersionSource {
    #[serde(rename = "above")]
    Above,
    #[serde(rename = "default")]
    Default,
    #[serde(rename = "required")]
    Required,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VersionFromQuote {
    #[serde(rename = "use")]
    Use,
    #[serde(rename = "off")]
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CachePolicy {
    #[serde(rename = "allowed")]
    Allowed,
    #[serde(rename = "fresh")]
    Fresh,
}

/// `bible.source`: when the StagePilot Bible service may supply verse text (see `versevision-bible`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum BibleSource {
    /// A version uses the user's own provider key when there is one, else the service.
    #[default]
    #[serde(rename = "auto")]
    Auto,
    /// The service only (for the versions it offers).
    #[serde(rename = "stagepilot")]
    StagePilot,
    /// Never the service.
    #[serde(rename = "own_keys")]
    OwnKeys,
}

/// Where ProPresenter's network API listens.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProPresenterConfig {
    pub host: String,
    pub port: u16,
    pub timeout_seconds: f64,
}

impl Default for ProPresenterConfig {
    fn default() -> Self {
        section_default(&schema::PROPRESENTER)
    }
}

/// How verse slides are laid out (`[slides]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SlideConfig {
    pub break_on_new_verse: bool,
    pub max_characters: u64,
    pub max_verses_per_slide: u64,
    pub preserve_poetry_lines: bool,
    pub even_split_punctuation_tolerance: f64,
    pub even_split_tolerance: f64,
    pub include_verse_numbers: bool,
    pub reference_format: String,
    pub reference_label_mode: ReferenceLabelMode,
    pub max_body_lines: u32,
    pub line_width_units: f64,
    pub max_reference_lines: u32,
    pub reference_width_units: f64,
    pub prefer_punctuation_breaks: bool,
    pub repeat_reference_on_continuation: bool,
}

impl Default for SlideConfig {
    fn default() -> Self {
        section_default(&schema::SLIDES)
    }
}

/// `[presentation]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresentationConfig {
    pub backup_count: u32,
}

impl Default for PresentationConfig {
    fn default() -> Self {
        section_default(&schema::PRESENTATION)
    }
}

/// The template presentation the slides are made from (`[presentation_template]`, absent until a template is chosen).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresentationTemplateConfig {
    pub presentation_uuid: String,
    pub presentation_name: String,
    pub library_name: Option<String>,
    pub cue_uuid: String,
    pub reference_object: String,
    pub body_object: String,
    pub style_fingerprint: String,
    pub point_cue_uuid: Option<String>,
    pub point_object: String,
    pub point_style_fingerprint: Option<String>,
}

/// Where new presentations go (`[output]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputConfig {
    pub library: String,
    pub swap_keeps_id: bool,
}

impl Default for OutputConfig {
    fn default() -> Self {
        section_default(&schema::OUTPUT)
    }
}

/// Where ProPresenter runs (`[node]`): `local` is this computer, `hermes-remote` another Mac.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeConfig {
    #[serde(rename = "type")]
    pub kind: NodeType,
    pub name: Option<String>,
    pub command: String,
    pub app_path: String,
    pub process_pattern: String,
    pub process_name: String,
    pub backup_dir: String,
    pub library_root: String,
    pub helper_dir: String,
    pub theme_root: String,
    pub confirm_quit: bool,
    pub retries: u32,
    pub retry_delay_seconds: f64,
    pub quit_wait_seconds: f64,
    pub launch_wait_seconds: f64,
}

impl Default for NodeConfig {
    fn default() -> Self {
        section_default(&schema::NODE)
    }
}

/// How the notes are read (`[notes]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotesConfig {
    pub line_breaks: LineBreaks,
    pub unlabeled_sentence: Handling,
    pub unlabeled_short: Handling,
    pub before_first_item: Handling,
    pub stage_cues: Handling,
    pub extra_cue_words: Vec<String>,
    pub skip_patterns: Vec<String>,
    pub quote_above_reference: QuoteAboveReference,
    pub quote_alone: Handling,
    pub reference_in_text: ReferenceInText,
    pub whole_chapter: WholeChapter,
    pub duplicates: Duplicates,
    pub label_words: Vec<String>,
    pub label_own_slide: bool,
    pub header_max_words: u32,
    pub headings: Headings,
    pub caps_lines: CapsLines,
    pub wrapped_lines: WrappedLines,
    pub orphan_max_words: u32,
}

impl Default for NotesConfig {
    fn default() -> Self {
        section_default(&schema::NOTES)
    }
}

/// Sermon-point slides (`[points]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PointsConfig {
    pub numbered: bool,
    pub lettered: bool,
    pub bullets: bool,
    pub require_sequence: bool,
    pub numbering: Numbering,
    pub number_style: String,
    pub bullet_char: String,
    pub case: CaseMode,
    pub fix_label_typos: bool,
    pub strip_trailing_period: bool,
    pub balance_lines: bool,
    pub line_width_units: f64,
    pub line_tolerance: f64,
    pub line_tolerance_at_punctuation: f64,
    pub max_lines_per_slide: u32,
    pub max_lines: u32,
    pub shown_in_capitals: bool,
    pub theme: String,
    pub theme_slide: String,
}

impl Default for PointsConfig {
    fn default() -> Self {
        section_default(&schema::POINTS)
    }
}

/// Which version a verse gets (`[verses]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VersesConfig {
    pub version_source: VersionSource,
    pub version_from_quote: VersionFromQuote,
    pub quote_versions: Vec<String>,
    pub quote_match: f64,
    pub theme: String,
    pub theme_slide: String,
}

impl Default for VersesConfig {
    fn default() -> Self {
        section_default(&schema::VERSES)
    }
}

/// The default of one section: what the schema says, read back into its struct.
fn section_default<T: serde::de::DeserializeOwned>(model: &'static schema::Model) -> T {
    let defaults = validate::validate_model(model, &Value::Object(Map::new()), &mut Vec::new());
    match defaults.map(Value::Object).map(serde_json::from_value::<T>) {
        Ok(Ok(section)) => section,
        _ => unreachable!(
            "the defaults of {} are valid by construction (tests prove it)",
            model.name
        ),
    }
}

/// One `[bible.providers.NAME]` table; its `type` word picks the variant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ProviderConfig {
    #[serde(rename = "api_bible")]
    ApiBible(RemoteProviderConfig),
    #[serde(rename = "youversion")]
    YouVersion(RemoteProviderConfig),
    #[serde(rename = "esv")]
    Esv(EsvProviderConfig),
    #[serde(rename = "usfm")]
    Usfm(LocalUsfmProviderConfig),
}

/// API.Bible and YouVersion share their fields (their defaults differ, and the schema holds those).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteProviderConfig {
    pub api_key_env: String,
    pub base_url: String,
    pub timeout_seconds: f64,
    pub bible_ids: IndexMap<String, String>,
    pub cache_policy: CachePolicy,
}

/// ESV text is never cached (Crossway's terms), so its only cache policy is `fresh`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EsvProviderConfig {
    pub api_key_env: String,
    pub base_url: String,
    pub timeout_seconds: f64,
    pub cache_policy: CachePolicy,
}

/// A local USFM Bible (`path` is kept as written, normalised like `pathlib.Path`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalUsfmProviderConfig {
    pub version: String,
    pub path: String,
    pub name: Option<String>,
    pub copyright: Option<String>,
    pub attribution: Option<String>,
    pub cache_policy: CachePolicy,
}

/// `[bible]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BibleConfig {
    pub default_version: Option<String>,
    pub version_aliases: IndexMap<String, String>,
    pub version_routes: IndexMap<String, String>,
    pub providers: IndexMap<String, ProviderConfig>,
    pub source: BibleSource,
}

/// The versions the StagePilot Bible service offers, which provider serves each and the provider's own Bible id (a public
/// identifier, not a secret). The same table as the operator's VerseVision configuration and the service's `VERSION_ROUTES`.
/// `(version, provider name, bible id)`; ESV has no id.
pub const BUILTIN_VERSION_ROUTES: [(&str, &str, &str); 14] = [
    ("ESV", "esv", ""),
    ("NIV", "youversion", "111"),
    ("AMP", "youversion", "1588"),
    ("BSB", "youversion", "3034"),
    ("LSV", "youversion", "2660"),
    ("NIRV", "youversion", "110"),
    ("NASB1995", "youversion", "100"),
    ("NASB2020", "youversion", "2692"),
    ("TPT", "youversion", "1849"),
    ("NKJV", "api_bible", "63097d2a0a2f7db3-01"),
    ("NLT", "api_bible", "d6e14a625393b4da-01"),
    ("KJV", "api_bible", "de4e12af7f28f599-01"),
    ("MSG", "api_bible", "6f11a7de016f942e-01"),
    ("WEB", "api_bible", "9879dbb7cfe39e4d-01"),
];

impl BibleConfig {
    /// The providers a config gets when it defines no `[bible.providers.*]` of its own: API.Bible, YouVersion and the ESV API with
    /// their default settings, and a route for each version in [`BUILTIN_VERSION_ROUTES`]. Only `providers` and `version_routes`
    /// are filled. (An addition to Python, which has no built-in providers; see `bible.source`.)
    pub fn builtin_providers() -> BibleConfig {
        let mut routes = Map::new();
        let mut api = Map::new();
        let mut youversion = Map::new();
        for (version, provider, id) in BUILTIN_VERSION_ROUTES {
            routes.insert(version.to_string(), Value::String(provider.to_string()));
            let ids = match provider {
                "api_bible" => &mut api,
                "youversion" => &mut youversion,
                _ => continue,
            };
            ids.insert(version.to_string(), Value::String(id.to_string()));
        }
        let provider = |kind: &str, ids: Option<Map<String, Value>>| {
            let mut table = Map::new();
            table.insert("type".into(), Value::String(kind.to_string()));
            if let Some(ids) = ids {
                table.insert("bible_ids".into(), Value::Object(ids));
            }
            Value::Object(table)
        };
        let mut providers = Map::new();
        providers.insert("api_bible".into(), provider("api_bible", Some(api)));
        providers.insert("youversion".into(), provider("youversion", Some(youversion)));
        providers.insert("esv".into(), provider("esv", None));
        let mut bible = Map::new();
        bible.insert("providers".into(), Value::Object(providers));
        bible.insert("version_routes".into(), Value::Object(routes));
        let mut root = Map::new();
        root.insert("bible".into(), Value::Object(bible));
        AppConfig::from_value(&Value::Object(root))
            .expect("the built-in provider tables are valid")
            .bible
    }

    /// This config with the built-in providers and routes when it defines no provider table; the config itself otherwise. Routes the
    /// user wrote win over a built-in route for the same version (compared without regard to case).
    pub fn with_builtin_providers(&self, builtin: &BibleConfig) -> BibleConfig {
        if !self.providers.is_empty() {
            return self.clone();
        }
        let mut out = self.clone();
        out.providers = builtin.providers.clone();
        out.version_routes = builtin
            .version_routes
            .iter()
            .filter(|(version, _)| {
                !self
                    .version_routes
                    .keys()
                    .any(|own| crate::references::casefold(own) == crate::references::casefold(version))
            })
            .map(|(version, provider)| (version.clone(), provider.clone()))
            .chain(self.version_routes.iter().map(|(v, p)| (v.clone(), p.clone())))
            .collect();
        out
    }
}

impl Default for BibleConfig {
    fn default() -> Self {
        section_default(&schema::BIBLE)
    }
}

/// The whole config file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppConfig {
    pub propresenter: ProPresenterConfig,
    pub node: NodeConfig,
    pub slides: SlideConfig,
    pub notes: NotesConfig,
    pub points: PointsConfig,
    pub verses: VersesConfig,
    /// `[profiles.NAME.notes]` etc.: named sets of overrides for `run --profile NAME`. Kept as written.
    pub profiles: Map<String, Value>,
    pub presentation: PresentationConfig,
    pub presentation_template: Option<PresentationTemplateConfig>,
    pub output: OutputConfig,
    pub bible: BibleConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        section_default(&schema::APP)
    }
}

impl AppConfig {
    /// Validate a JSON/TOML-shaped value (missing keys take their defaults, unknown keys are ignored, like pydantic's default).
    pub fn from_value(input: &Value) -> Result<AppConfig, ValidationError> {
        let checked = validate::validate_model(&schema::APP, input, &mut Vec::new())?;
        match serde_json::from_value(Value::Object(checked)) {
            Ok(config) => Ok(config),
            Err(error) => Err(ValidationError {
                loc: Vec::new(),
                msg: format!("internal: {error}"),
            }),
        }
    }

    /// `model_dump(mode="json")`: every field, `null` for the empty optional ones, in model order.
    pub fn to_value(&self) -> Value {
        // the structs only hold JSON-friendly types, so this cannot fail
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    /// Read a config file's text. Nothing here touches a file: the caller reads it.
    pub fn from_toml_str(text: &str) -> Result<AppConfig, ConfigError> {
        let value = toml_io::parse(text).map_err(ConfigError::Toml)?;
        AppConfig::from_value(&value).map_err(ConfigError::Invalid)
    }

    /// The text of a config file: `model_dump(mode="json", exclude_none=True)` written as TOML.
    pub fn to_toml_string(&self) -> String {
        toml_io::write(&self.to_value())
    }

    /// The config with a named profile and/or `section.option=value` overrides applied (nothing is saved).
    pub fn with_options(
        &self,
        profile: Option<&str>,
        sets: &[&str],
    ) -> Result<AppConfig, ConfigError> {
        with_options(self, profile, sets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_are_what_python_has() {
        let config = AppConfig::default();
        assert_eq!(config.propresenter.port, 50001);
        assert_eq!(config.slides.max_body_lines, 5);
        assert_eq!(
            config.slides.reference_label_mode,
            ReferenceLabelMode::Verse
        );
        assert_eq!(config.points.line_width_units, 33.0);
        assert_eq!(config.notes.label_words.len(), 9);
        assert_eq!(
            config.bible.version_aliases.get("YLT").map(String::as_str),
            Some("NLT")
        );
        assert!(config.presentation_template.is_none());
        assert_eq!(config.node.kind, NodeType::Local);
        // every section's own Default is the section of the whole config
        assert_eq!(config.slides, SlideConfig::default());
        assert_eq!(config.bible, BibleConfig::default());
        assert_eq!(AppConfig::from_value(&json!({})).as_ref(), Ok(&config));
    }

    #[test]
    fn toml_in_and_out() {
        let config = AppConfig::from_toml_str("[slides]\nmax_body_lines = 3\nline_width_units = 40\n\n[notes]\nline_breaks = \"paragraph\"\n").unwrap();
        assert_eq!(config.slides.max_body_lines, 3);
        assert_eq!(config.slides.line_width_units, 40.0);
        assert_eq!(config.notes.line_breaks, LineBreaks::Paragraph);
        assert_eq!(
            AppConfig::from_toml_str(&config.to_toml_string()),
            Ok(config)
        );
        // unknown keys and sections are ignored on load, like pydantic's default (they are errors for `--set`)
        assert!(AppConfig::from_toml_str("[nothing]\nx = 1\n[slides]\nzzz = 2\n").is_ok());
        let error = AppConfig::from_toml_str("[slides]\nmax_body_lines = 0\n").unwrap_err();
        assert_eq!(
            error.to_string(),
            "Invalid option: Input should be greater than or equal to 1 at slides.max_body_lines"
        );
        assert!(matches!(
            AppConfig::from_toml_str("[slides"),
            Err(ConfigError::Toml(_))
        ));
    }

    #[test]
    fn options_follow_python() {
        let config = AppConfig::default();
        let changed = config
            .with_options(
                None,
                &[
                    "slides.max_body_lines=3",
                    "notes.extra_cue_words=[\"a\"]",
                    "slides.include_verse_numbers=yes",
                ],
            )
            .unwrap();
        assert_eq!(changed.slides.max_body_lines, 3);
        assert_eq!(changed.notes.extra_cue_words, vec!["a".to_string()]);
        assert!(changed.slides.include_verse_numbers);
        // a text option keeps its text: "1" is a slide name or number
        assert_eq!(
            config
                .with_options(None, &["points.theme_slide=1"])
                .unwrap()
                .points
                .theme_slide,
            "1"
        );
        assert_eq!(
            config
                .with_options(None, &["slides.nope=1"])
                .unwrap_err()
                .to_string(),
            "Unknown setting 'slides.nope'"
        );
        assert_eq!(
            config.with_options(Some("x"), &[]).unwrap_err().to_string(),
            "No profile named 'x' (none defined). Add [profiles.x.notes] etc. to the config."
        );
    }

    #[test]
    fn floats_are_written_like_python() {
        let written =
            toml_io::write(&json!({"a": 5.0, "b": 0.15, "c": 1e-5, "d": 1.5e16, "e": 123456.789}));
        assert_eq!(
            written,
            "a = 5.0\nb = 0.15\nc = 1e-05\nd = 1.5e+16\ne = 123456.789\n"
        );
    }
}
