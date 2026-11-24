from pathlib import Path

import pytest

from versevision.config import (
    AppConfig, BibleConfig, LocalUsfmProviderConfig, YouVersionProviderConfig, load_config, save_config,
)
from versevision.errors import ConfigurationError


def test_config_round_trip(tmp_path: Path) -> None:
    path = tmp_path / "versevision.toml"
    expected = AppConfig(bible=BibleConfig(default_version="ESV"))

    save_config(expected, path)

    assert load_config(path) == expected
    with pytest.raises(ConfigurationError, match="already exists"):
        save_config(expected, path)


def test_missing_config_returns_defaults(tmp_path: Path) -> None:
    assert load_config(tmp_path / "missing.toml") == AppConfig()


def test_provider_config_round_trip_preserves_paths(tmp_path: Path) -> None:
    path = tmp_path / "versevision.toml"
    expected = AppConfig(bible=BibleConfig(
        providers={"local": LocalUsfmProviderConfig(version="NKJV", path=tmp_path / "usfm")},
        version_routes={"NKJV": "local"},
    ))
    save_config(expected, path)
    assert load_config(path) == expected


def test_youversion_config_round_trip() -> None:
    config = YouVersionProviderConfig(bible_ids={"NIV": "111"})
    assert config.type == "youversion"
    assert config.bible_ids == {"NIV": "111"}
