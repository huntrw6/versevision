//! The shape of the config, written once as data: field names, types, ranges and defaults, in the order of
//! `python/src/versevision/config.py`. The order matters: pydantic validates the fields of a model in this order and
//! `with_options` reports only the first error.

pub struct Model {
    /// The Python class name (it is in the "valid dictionary or instance of X" message).
    pub name: &'static str,
    pub fields: &'static [Field],
}

pub struct Field {
    pub name: &'static str,
    pub ty: Ty,
    pub default: Def,
}

#[derive(Clone, Copy)]
pub enum Ty {
    Bool,
    Int {
        ge: Option<i64>,
        le: Option<i64>,
    },
    /// `gt`, `ge`, `le` bounds (`lt` is not used by the config).
    Float {
        gt: Option<f64>,
        ge: Option<f64>,
        le: Option<f64>,
    },
    Str,
    OptStr,
    Path,
    Literal(&'static [&'static str]),
    StrList,
    StrMap,
    Model(&'static Model),
    OptModel(&'static Model),
    /// `dict[str, ProviderConfig]`, a union discriminated by its `type`.
    Providers,
    /// `dict[str, dict]`
    Profiles,
}

#[derive(Clone, Copy)]
pub enum Def {
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(&'static str),
    Null,
    StrList(&'static [&'static str]),
    StrMap(&'static [(&'static str, &'static str)]),
    EmptyMap,
    /// The default of a nested model: the defaults of its own fields.
    Model,
    Required,
}

const fn f(name: &'static str, ty: Ty, default: Def) -> Field {
    Field { name, ty, default }
}
const fn flag(name: &'static str, default: bool) -> Field {
    f(name, Ty::Bool, Def::Bool(default))
}
const fn int(name: &'static str, default: i64, ge: Option<i64>, le: Option<i64>) -> Field {
    f(name, Ty::Int { ge, le }, Def::Int(default))
}
const fn num(
    name: &'static str,
    default: f64,
    gt: Option<f64>,
    ge: Option<f64>,
    le: Option<f64>,
) -> Field {
    f(name, Ty::Float { gt, ge, le }, Def::Float(default))
}
const fn text(name: &'static str, default: &'static str) -> Field {
    f(name, Ty::Str, Def::Str(default))
}
const fn choice(
    name: &'static str,
    values: &'static [&'static str],
    default: &'static str,
) -> Field {
    f(name, Ty::Literal(values), Def::Str(default))
}
const fn opt_text(name: &'static str) -> Field {
    f(name, Ty::OptStr, Def::Null)
}
const fn words(name: &'static str, default: &'static [&'static str]) -> Field {
    f(name, Ty::StrList, Def::StrList(default))
}

const HANDLING: &[&str] = &["bullet", "review", "skip"];
const SKIP_REVIEW_BULLET: &[&str] = &["skip", "review", "bullet"];
const ALLOWED_FRESH: &[&str] = &["allowed", "fresh"];

pub static PROPRESENTER: Model = Model {
    name: "ProPresenterConfig",
    fields: &[
        text("host", "127.0.0.1"),
        int("port", 50001, Some(1), Some(65535)),
        num("timeout_seconds", 5.0, Some(0.0), None, Some(120.0)),
    ],
};

pub static SLIDES: Model = Model {
    name: "SlideConfig",
    fields: &[
        flag("break_on_new_verse", true),
        int("max_characters", 350, Some(1), None),
        int("max_verses_per_slide", 1, Some(1), None),
        flag("preserve_poetry_lines", true),
        num(
            "even_split_punctuation_tolerance",
            0.25,
            None,
            Some(0.0),
            Some(1.0),
        ),
        num("even_split_tolerance", 0.15, None, Some(0.0), Some(1.0)),
        flag("include_verse_numbers", false),
        text("reference_format", "{book} {chapter}:{verses} {version}"),
        choice(
            "reference_label_mode",
            &["source", "segment", "verse"],
            "verse",
        ),
        int("max_body_lines", 5, Some(1), Some(20)),
        num("line_width_units", 48.0, Some(5.0), None, Some(200.0)),
        int("max_reference_lines", 1, Some(1), Some(3)),
        num("reference_width_units", 46.0, Some(5.0), None, Some(200.0)),
        flag("prefer_punctuation_breaks", true),
        flag("repeat_reference_on_continuation", true),
    ],
};

pub static PRESENTATION: Model = Model {
    name: "PresentationConfig",
    fields: &[int("backup_count", 10, Some(1), Some(100))],
};

pub static PRESENTATION_TEMPLATE: Model = Model {
    name: "PresentationTemplateConfig",
    fields: &[
        f("presentation_uuid", Ty::Str, Def::Required),
        f("presentation_name", Ty::Str, Def::Required),
        opt_text("library_name"),
        f("cue_uuid", Ty::Str, Def::Required),
        text("reference_object", "Reference"),
        text("body_object", "Verse"),
        f("style_fingerprint", Ty::Str, Def::Required),
        opt_text("point_cue_uuid"),
        text("point_object", "Sermon Point"),
        opt_text("point_style_fingerprint"),
    ],
};

pub static OUTPUT: Model = Model {
    name: "OutputConfig",
    fields: &[text("library", "Verses"), flag("swap_keeps_id", false)],
};

pub static API_BIBLE: Model = Model {
    name: "ApiBibleProviderConfig",
    fields: &[
        choice("type", &["api_bible"], "api_bible"),
        text("api_key_env", "API_BIBLE_API_KEY"),
        text("base_url", "https://api.scripture.api.bible/v1"),
        num("timeout_seconds", 15.0, Some(0.0), None, Some(120.0)),
        f("bible_ids", Ty::StrMap, Def::EmptyMap),
        choice("cache_policy", ALLOWED_FRESH, "allowed"),
    ],
};

pub static YOUVERSION: Model = Model {
    name: "YouVersionProviderConfig",
    fields: &[
        choice("type", &["youversion"], "youversion"),
        text("api_key_env", "YOUVERSION_API_KEY"),
        text("base_url", "https://api.youversion.com/v1"),
        num("timeout_seconds", 15.0, Some(0.0), None, Some(120.0)),
        f("bible_ids", Ty::StrMap, Def::EmptyMap),
        choice("cache_policy", ALLOWED_FRESH, "allowed"),
    ],
};

pub static ESV: Model = Model {
    name: "EsvProviderConfig",
    fields: &[
        choice("type", &["esv"], "esv"),
        text("api_key_env", "ESV_API_KEY"),
        text("base_url", "https://api.esv.org/v3"),
        num("timeout_seconds", 15.0, Some(0.0), None, Some(120.0)),
        choice("cache_policy", &["fresh"], "fresh"),
    ],
};

pub static USFM: Model = Model {
    name: "LocalUsfmProviderConfig",
    fields: &[
        choice("type", &["usfm"], "usfm"),
        f("version", Ty::Str, Def::Required),
        f("path", Ty::Path, Def::Required),
        opt_text("name"),
        opt_text("copyright"),
        opt_text("attribution"),
        choice("cache_policy", ALLOWED_FRESH, "allowed"),
    ],
};

/// The provider models, in the order the Python union lists them (it is in the "tag does not match" message).
pub static PROVIDERS: [&Model; 4] = [&API_BIBLE, &YOUVERSION, &ESV, &USFM];

pub static BIBLE: Model = Model {
    name: "BibleConfig",
    fields: &[
        opt_text("default_version"),
        f(
            "version_aliases",
            Ty::StrMap,
            Def::StrMap(&[("YLT", "NLT")]),
        ),
        f("version_routes", Ty::StrMap, Def::EmptyMap),
        f("providers", Ty::Providers, Def::EmptyMap),
        choice("source", &["auto", "stagepilot", "own_keys"], "auto"),
    ],
};

pub static NODE: Model = Model {
    name: "NodeConfig",
    fields: &[
        choice("type", &["local", "hermes-remote"], "local"),
        opt_text("name"),
        text("command", "hermes-remote"),
        text("app_path", "/Applications/ProPresenter.app"),
        text("process_pattern", "MacOS/ProPresenter$"),
        text("process_name", "ProPresenter"),
        text("backup_dir", "$HOME/VerseVision-backups"),
        text("library_root", "$HOME/Documents/ProPresenter/Libraries"),
        text("helper_dir", "$HOME/VerseVision-tools"),
        text("theme_root", "$HOME/Documents/ProPresenter/Themes"),
        flag("confirm_quit", true),
        int("retries", 2, Some(0), Some(5)),
        num("retry_delay_seconds", 20.0, None, Some(0.0), Some(120.0)),
        num("quit_wait_seconds", 25.0, None, Some(1.0), Some(300.0)),
        num("launch_wait_seconds", 90.0, None, Some(5.0), Some(600.0)),
    ],
};

pub static NOTES: Model = Model {
    name: "NotesConfig",
    fields: &[
        choice("line_breaks", &["slide", "paragraph"], "slide"),
        choice("unlabeled_sentence", HANDLING, "bullet"),
        choice("unlabeled_short", HANDLING, "bullet"),
        choice("before_first_item", SKIP_REVIEW_BULLET, "skip"),
        choice("stage_cues", SKIP_REVIEW_BULLET, "skip"),
        words("extra_cue_words", &[]),
        words("skip_patterns", &[]),
        choice("quote_above_reference", &["skip", "review"], "skip"),
        choice("quote_alone", &["review", "skip", "bullet"], "review"),
        choice("reference_in_text", &["review", "skip", "verse"], "review"),
        choice("whole_chapter", &["review", "skip"], "review"),
        choice("duplicates", &["keep", "drop"], "keep"),
        words(
            "label_words",
            &[
                "conclusion",
                "application",
                "invitation",
                "challenge",
                "summary",
                "closing",
                "takeaway",
                "big idea",
                "main point",
            ],
        ),
        flag("label_own_slide", true),
        int("header_max_words", 2, Some(0), Some(6)),
        choice("headings", &["slide", "skip"], "slide"),
        choice("caps_lines", &["heading", "line"], "heading"),
        choice("wrapped_lines", &["join", "separate"], "join"),
        int("orphan_max_words", 3, Some(0), Some(8)),
    ],
};

pub static POINTS: Model = Model {
    name: "PointsConfig",
    fields: &[
        flag("numbered", true),
        flag("lettered", true),
        flag("bullets", true),
        flag("require_sequence", true),
        choice("numbering", &["keep", "strip", "renumber"], "keep"),
        text("number_style", "{n})"),
        text("bullet_char", "•"),
        choice(
            "case",
            &["as_written", "upper", "lower", "title"],
            "as_written",
        ),
        flag("fix_label_typos", true),
        flag("strip_trailing_period", true),
        flag("balance_lines", true),
        num("line_width_units", 33.0, Some(5.0), None, Some(200.0)),
        num("line_tolerance", 0.15, None, Some(0.0), Some(1.0)),
        num(
            "line_tolerance_at_punctuation",
            0.25,
            None,
            Some(0.0),
            Some(1.0),
        ),
        int("max_lines_per_slide", 0, Some(0), Some(12)),
        int("max_lines", 4, Some(1), Some(12)),
        flag("shown_in_capitals", true),
        text("theme", ""),
        text("theme_slide", ""),
    ],
};

pub static VERSES: Model = Model {
    name: "VersesConfig",
    fields: &[
        choice("version_source", &["above", "default", "required"], "above"),
        choice("version_from_quote", &["use", "off"], "use"),
        words(
            "quote_versions",
            &["KJV", "NKJV", "NIV", "NASB1995", "NLT", "ESV"],
        ),
        num("quote_match", 0.85, None, Some(0.5), Some(1.0)),
        text("theme", ""),
        text("theme_slide", ""),
    ],
};

/// `AppConfig`, the whole file.
pub static APP: Model = Model {
    name: "AppConfig",
    fields: &[
        f("propresenter", Ty::Model(&PROPRESENTER), Def::Model),
        f("node", Ty::Model(&NODE), Def::Model),
        f("slides", Ty::Model(&SLIDES), Def::Model),
        f("notes", Ty::Model(&NOTES), Def::Model),
        f("points", Ty::Model(&POINTS), Def::Model),
        f("verses", Ty::Model(&VERSES), Def::Model),
        f("profiles", Ty::Profiles, Def::EmptyMap),
        f("presentation", Ty::Model(&PRESENTATION), Def::Model),
        f(
            "presentation_template",
            Ty::OptModel(&PRESENTATION_TEMPLATE),
            Def::Null,
        ),
        f("output", Ty::Model(&OUTPUT), Def::Model),
        f("bible", Ty::Model(&BIBLE), Def::Model),
    ],
};
