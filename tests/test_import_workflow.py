import asyncio

import pytest

from versebridge.config import AppConfig, BibleConfig
from versebridge.errors import ConfigurationError
from versebridge.workflow import dry_run_import


def test_dry_run_applies_default_and_never_changes_files() -> None:
    result = asyncio.run(dry_run_import(
        "John 3:16-18 NIV\nPsalm 23:1",
        AppConfig(bible=BibleConfig(default_version="ESV")),
        validate_target=False,
    ))

    assert [reference.version for reference in result.references] == ["NIV", "ESV"]
    assert [slide.verse_keys[0] for slide in result.slide_plan.slides] == [
        "JHN.3.16", "JHN.3.17", "JHN.3.18", "PSA.23.1"
    ]
    assert result.fetched is False
    assert result.files_changed is False


def test_dry_run_requires_a_version_when_no_default_exists() -> None:
    with pytest.raises(ConfigurationError, match="no default is configured"):
        asyncio.run(dry_run_import("John 3:16", AppConfig(), validate_target=False))
