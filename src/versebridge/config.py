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
    backup_dir: str = "$HOME/VerseBridge-backups"
    library_root: str = "$HOME/Documents/ProPresenter/Libraries"
    helper_dir: str = "$HOME/VerseBridge-tools"
    confirm_quit: bool = True        # press OK on ProPresenter's "Are you sure you want to quit?" box (nothing else)
    retries: int = Field(default=2, ge=0, le=5)
    retry_delay_seconds: float = Field(default=20.0, ge=0, le=120)
    quit_wait_seconds: float = Field(default=25.0, ge=1, le=300)
    launch_wait_seconds: float = Field(default=90.0, ge=5, le=600)


class AppConfig(BaseModel):
    propresenter: ProPresenterConfig = Field(default_factory=ProPresenterConfig)
    node: NodeConfig = Field(default_factory=NodeConfig)
    slides: SlideConfig = Field(default_factory=SlideConfig)
    presentation: PresentationConfig = Field(default_factory=PresentationConfig)
    presentation_template: PresentationTemplateConfig | None = None
    output: OutputConfig = Field(default_factory=OutputConfig)
    bible: BibleConfig = Field(default_factory=BibleConfig)


def config_path() -> Path:
    override = os.getenv("VERSEBRIDGE_CONFIG")
    return Path(override).expanduser() if override else user_config_path("versebridge") / "config.toml"


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
