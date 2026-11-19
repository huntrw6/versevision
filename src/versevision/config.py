from __future__ import annotations

import os
from pathlib import Path
from typing import Annotated, Literal

from platformdirs import user_config_path
from pydantic import BaseModel, Field, ValidationError
import tomllib
import tomli_w

from .errors import ConfigurationError


class ProPresenterConfig(BaseModel):
    host: str = "127.0.0.1"
    port: int = Field(default=50001, ge=1, le=65535)
    timeout_seconds: float = Field(default=5.0, gt=0, le=120)


class SlideConfig(BaseModel):
    break_on_new_verse: bool = True
    max_characters: int = Field(default=350, ge=1)
    max_verses_per_slide: int = Field(default=1, ge=1)
    preserve_poetry_lines: bool = True
    include_verse_numbers: bool = True
    reference_format: str = "{book} {chapter}:{verses} {version}"
    reference_label_mode: Literal["source", "segment", "verse"] = "verse"
    max_body_lines: int = Field(default=5, ge=1, le=20)
    line_width_units: float = Field(default=48.0, gt=5, le=200)
    prefer_punctuation_breaks: bool = True
    repeat_reference_on_continuation: bool = True


class PresentationConfig(BaseModel):
    backup_count: int = Field(default=10, ge=1, le=100)


class PresentationTemplateConfig(BaseModel):
    presentation_uuid: str
    presentation_name: str
    library_name: str | None = None
    cue_uuid: str
    reference_object: str = "Reference"
    body_object: str = "Verse"
    style_fingerprint: str
    point_cue_uuid: str | None = None
    point_object: str = "Sermon Point"
    point_style_fingerprint: str | None = None


class OutputConfig(BaseModel):
    """Where new presentations go. The default is a NEW presentation in this library (no restart needed)."""
    library: str = "Verses"


class ApiBibleProviderConfig(BaseModel):
    type: Literal["api_bible"] = "api_bible"
    api_key_env: str = "API_BIBLE_API_KEY"
    base_url: str = "https://api.scripture.api.bible/v1"
    timeout_seconds: float = Field(default=15.0, gt=0, le=120)
    bible_ids: dict[str, str] = Field(default_factory=dict)
    cache_policy: Literal["allowed", "fresh"] = "allowed"


class YouVersionProviderConfig(BaseModel):
    type: Literal["youversion"] = "youversion"
    api_key_env: str = "YOUVERSION_API_KEY"
    base_url: str = "https://api.youversion.com/v1"
    timeout_seconds: float = Field(default=15.0, gt=0, le=120)
    bible_ids: dict[str, str] = Field(default_factory=dict)
    cache_policy: Literal["allowed", "fresh"] = "allowed"


class EsvProviderConfig(BaseModel):
    type: Literal["esv"] = "esv"
    api_key_env: str = "ESV_API_KEY"
    base_url: str = "https://api.esv.org/v3"
    timeout_seconds: float = Field(default=15.0, gt=0, le=120)
    # Crossway's terms limit local storage, so ESV text is never cached.
    cache_policy: Literal["fresh"] = "fresh"


class LocalUsfmProviderConfig(BaseModel):
    type: Literal["usfm"] = "usfm"
    version: str
    path: Path
    name: str | None = None
    copyright: str | None = None
    attribution: str | None = None
    cache_policy: Literal["allowed", "fresh"] = "allowed"


ProviderConfig = Annotated[
    ApiBibleProviderConfig | YouVersionProviderConfig | EsvProviderConfig | LocalUsfmProviderConfig,
    Field(discriminator="type"),
]


class BibleConfig(BaseModel):
    default_version: str | None = None
    version_aliases: dict[str, str] = Field(default_factory=lambda: {"YLT": "NLT"})
    version_routes: dict[str, str] = Field(default_factory=dict)
    providers: dict[str, ProviderConfig] = Field(default_factory=dict)


class NodeConfig(BaseModel):
    """Where ProPresenter runs. ``local`` is this computer; ``hermes-remote`` is another Mac."""
    type: Literal["local", "hermes-remote"] = "local"
    name: str | None = None
    command: str = "hermes-remote"
    app_path: str = "/Applications/ProPresenter.app"
    process_pattern: str = "MacOS/ProPresenter$"
    process_name: str = "ProPresenter"      # the name macOS shows for the running app
    backup_dir: str = "$HOME/VerseVision-backups"
    library_root: str = "$HOME/Documents/ProPresenter/Libraries"
    helper_dir: str = "$HOME/VerseVision-tools"
    confirm_quit: bool = True        # press OK on ProPresenter's "Are you sure you want to quit?" box (nothing else)
    retries: int = Field(default=2, ge=0, le=5)
    retry_delay_seconds: float = Field(default=20.0, ge=0, le=120)
    quit_wait_seconds: float = Field(default=25.0, ge=1, le=300)
    launch_wait_seconds: float = Field(default=90.0, ge=5, le=600)


Handling = Literal["bullet", "review", "skip"]


class NotesConfig(BaseModel):
    """How the notes are read. The defaults are Hunter's; change them when a document is laid out differently."""
    line_breaks: Literal["slide", "paragraph"] = "slide"
    """slide: every line break is a different slide (lines the page layout wrapped mid-sentence are still joined).
    paragraph: a plain sentence under a point joins that point's slide."""
    unlabeled_sentence: Handling = "bullet"        # a plain sentence after the first point: its own bullet slide | review | skip
    unlabeled_short: Handling = "bullet"           # a short line with no ending punctuation (may be a heading), same choices
    before_first_item: Literal["skip", "review", "bullet"] = "skip"    # lines before the first verse or point (title, date, speaker)
    stage_cues: Literal["skip", "review", "bullet"] = "skip"           # "Show slide of...", "(note)", URLs
    extra_cue_words: list[str] = Field(default_factory=list)           # more words that start a stage cue, e.g. ["pause", "sing"]
    skip_patterns: list[str] = Field(default_factory=list)             # regular expressions: a matching line is always left out
    quote_above_reference: Literal["skip", "review"] = "skip"          # verse text typed above its reference (the text comes from the Bible provider)
    quote_alone: Literal["review", "skip", "bullet"] = "review"        # a quotation with no reference anywhere near it
    reference_in_text: Literal["review", "skip", "verse"] = "review"   # "as Psalm 23:1 says, ..." (verse: also add that verse)
    whole_chapter: Literal["review", "skip"] = "review"                # "Psalm 23" with no verses
    duplicates: Literal["keep", "drop"] = "keep"                       # the same verse and version again later
    label_words: list[str] = Field(default_factory=lambda: ["conclusion", "application", "invitation", "challenge", "summary", "closing", "takeaway", "big idea", "main point"])
    label_own_slide: bool = True                                       # "Conclusion: ..." is its own slide instead of part of the point above
    header_max_words: int = Field(default=2, ge=0, le=6)               # capitalized lines this short are headings, not slides


class PointsConfig(BaseModel):
    """Sermon-point slides: what counts as a point, and how its text is written."""
    numbered: bool = True              # "1)", "1.", "#1:"
    lettered: bool = True              # "a)", "b."
    bullets: bool = True               # "•", "-", "*"
    require_sequence: bool = True      # numbers out of order (1, 2, 5) become plain list items
    numbering: Literal["keep", "strip", "renumber"] = "keep"          # keep "1)", drop the numbers, or number them 1, 2, 3 again
    number_style: str = "{n})"         # used by "renumber": "{n})", "{n}.", "#{n}:"
    bullet_char: str = "•"             # "" for no bullet mark on the slide
    case: Literal["as_written", "upper", "lower", "title"] = "as_written"


class VersesConfig(BaseModel):
    """Which version a verse gets, and how repeats are handled. Slide layout (verse numbers, reference format, splitting) is under [slides]."""
    version_source: Literal["above", "default", "required"] = "above"
    """above: the nearest version named above it, else the default. default: always the default version unless it names its own.
    required: a verse with no version is an error."""


class AppConfig(BaseModel):
    propresenter: ProPresenterConfig = Field(default_factory=ProPresenterConfig)
    node: NodeConfig = Field(default_factory=NodeConfig)
    slides: SlideConfig = Field(default_factory=SlideConfig)
    notes: NotesConfig = Field(default_factory=NotesConfig)
    points: PointsConfig = Field(default_factory=PointsConfig)
    verses: VersesConfig = Field(default_factory=VersesConfig)
    profiles: dict[str, dict] = Field(default_factory=dict)      # [profiles.NAME.notes] etc: named sets of overrides for `run --profile NAME`
    presentation: PresentationConfig = Field(default_factory=PresentationConfig)
    presentation_template: PresentationTemplateConfig | None = None
    output: OutputConfig = Field(default_factory=OutputConfig)
    bible: BibleConfig = Field(default_factory=BibleConfig)


def config_path() -> Path:
    override = os.getenv("VERSEVISION_CONFIG")
    return Path(override).expanduser() if override else user_config_path("versevision") / "config.toml"


def load_config(path: Path | None = None) -> AppConfig:
    selected = path or config_path()
    if not selected.exists():
        return AppConfig()
    try:
        with selected.open("rb") as stream:
            return AppConfig.model_validate(tomllib.load(stream))
    except (OSError, tomllib.TOMLDecodeError, ValidationError) as exc:
        raise ConfigurationError(f"Invalid configuration at {selected}: {exc}") from exc


def save_config(config: AppConfig, path: Path | None = None, *, overwrite: bool = False) -> Path:
    selected = path or config_path()
    if selected.exists() and not overwrite:
        raise ConfigurationError(f"Configuration already exists: {selected}")
    selected.parent.mkdir(parents=True, exist_ok=True)
    temporary = selected.with_suffix(selected.suffix + ".tmp")
    temporary.write_text(tomli_w.dumps(config.model_dump(mode="json", exclude_none=True)), encoding="utf-8")
    temporary.replace(selected)
    return selected


def _coerce(text: str):
    low = text.strip().casefold()
    if low in {"true", "yes", "on"}: return True
    if low in {"false", "no", "off"}: return False
    try: return int(text)
    except ValueError: pass
    try: return float(text)
    except ValueError: pass
    if text.strip().startswith("["):
        import json
        try: return json.loads(text)
        except ValueError: pass
    return text


def _merge(base: dict, extra: dict) -> dict:
    for key, value in extra.items():
        if isinstance(value, dict) and isinstance(base.get(key), dict):
            _merge(base[key], value)
        else:
            base[key] = value
    return base


def with_options(config: AppConfig, *, profile: str | None = None, sets: list[str] | None = None) -> AppConfig:
    """The config with a named profile and/or `section.option=value` overrides applied (for one run; nothing is saved)."""
    data = config.model_dump(mode="json")
    if profile:
        if profile not in config.profiles:
            known = ", ".join(sorted(config.profiles)) or "none defined"
            raise ConfigurationError(f"No profile named '{profile}' ({known}). Add [profiles.{profile}.notes] etc. to the config.")
        _merge(data, config.profiles[profile])
    for item in sets or []:
        key, sep, value = item.partition("=")
        parts = key.strip().split(".")
        if not sep or len(parts) < 2:
            raise ConfigurationError(f"Use section.option=value, for example notes.line_breaks=paragraph (got '{item}')")
        node = data
        for part in parts[:-1]:
            if not isinstance(node.get(part), dict):
                raise ConfigurationError(f"Unknown setting section '{key}'")
            node = node[part]
        if parts[-1] not in node:
            raise ConfigurationError(f"Unknown setting '{key}'")
        node[parts[-1]] = _coerce(value)
    try:
        return AppConfig.model_validate(data)
    except ValidationError as exc:
        raise ConfigurationError(f"Invalid option: {exc.errors()[0]['msg']} at {'.'.join(map(str, exc.errors()[0]['loc']))}") from exc
