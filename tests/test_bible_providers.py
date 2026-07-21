import asyncio
from pathlib import Path

import httpx
import pytest
import respx

from versebridge.bibles.api_bible import ApiBibleProvider
from versebridge.bibles.cache import ScriptureCache
from versebridge.bibles.local_usfm import LocalUsfmProvider
from versebridge.bibles.youversion import YouVersionProvider
from versebridge.config import ApiBibleProviderConfig, LocalUsfmProviderConfig, YouVersionProviderConfig
from versebridge.errors import BibleProviderError
from versebridge.references.models import VersePoint, VerseSpan


def _usfm(tmp_path: Path) -> Path:
    directory = tmp_path / "usfm"
    directory.mkdir()
    (directory / "JHN.usfm").write_text(
        "\\id JHN\n\\c 3\n\\p\n\\v 16 Synthetic verse sixteen.\n"
        "\\q1 \\v 17 Synthetic poetry line.\n\\v 18 Synthetic verse eighteen.\n",
        encoding="utf-8",
    )
    return directory


def test_local_usfm_fetches_ordered_metadata(tmp_path: Path) -> None:
    provider = LocalUsfmProvider("local_test", LocalUsfmProviderConfig(
        version="TST", path=_usfm(tmp_path), copyright="Synthetic test text",
    ))
    verses = asyncio.run(provider.fetch_passage("TST", "JHN", [VerseSpan(
        start=VersePoint(chapter=3, verse=16), end=VersePoint(chapter=3, verse=18),
    )]))

    assert [verse.verse for verse in verses] == [16, 17, 18]
    assert verses[0].paragraph_start is True
    assert verses[1].poetry_indent == 1
    assert all(verse.copyright == "Synthetic test text" for verse in verses)


def test_scripture_cache_round_trip(tmp_path: Path) -> None:
    provider = LocalUsfmProvider("local_test", LocalUsfmProviderConfig(version="TST", path=_usfm(tmp_path)))
    verse = asyncio.run(provider.fetch_passage("TST", "JHN", [VerseSpan(start=VersePoint(chapter=3, verse=16))]))[0]
    with ScriptureCache(tmp_path / "cache.sqlite3") as cache:
        cache.put("local_test", "fixture", verse)
        assert cache.get("local_test", "fixture", "JHN", 3, 16) == verse
        assert cache.clear() == 1
        assert cache.count() == 0


@respx.mock
def test_api_bible_lists_and_fetches_without_leaking_key(monkeypatch) -> None:
    monkeypatch.setenv("VERSEBRIDGE_TEST_BIBLE_KEY", "do-not-log-this")
    respx.get("https://example.test/v1/bibles").mock(return_value=httpx.Response(200, json={"data": [{
        "id": "bible-id", "abbreviation": "TST", "name": "Synthetic Bible", "language": {"id": "eng"},
        "copyright": "Synthetic copyright",
    }]}))
    respx.get("https://example.test/v1/bibles/bible-id/verses/JHN.3.16").mock(
        return_value=httpx.Response(200, json={"data": {"content": "<p>[16] Synthetic API verse.</p>"}})
    )
    provider = ApiBibleProvider("api", ApiBibleProviderConfig(
        api_key_env="VERSEBRIDGE_TEST_BIBLE_KEY", base_url="https://example.test/v1",
    ))

    async def run():
        try:
            return await provider.fetch_passage("TST", "JHN", [VerseSpan(start=VersePoint(chapter=3, verse=16))])
        finally:
            await provider.aclose()

    verses = asyncio.run(run())
    assert verses[0].text == "Synthetic API verse."
    assert verses[0].copyright == "Synthetic copyright"


def test_api_bible_missing_key_names_environment_variable_only() -> None:
    provider = ApiBibleProvider("api", ApiBibleProviderConfig(api_key_env="DEFINITELY_MISSING_VERSEBRIDGE_KEY"))
    async def run():
        try:
            await provider.list_versions()
        finally:
            await provider.aclose()
    with pytest.raises(BibleProviderError, match="DEFINITELY_MISSING_VERSEBRIDGE_KEY"):
        asyncio.run(run())


@respx.mock
def test_youversion_aliases_configured_id_and_fetches_text(monkeypatch) -> None:
    monkeypatch.setenv("VERSEBRIDGE_TEST_YV_KEY", "do-not-log-this")
    respx.get("https://youversion.test/v1/bibles").mock(return_value=httpx.Response(200, json={"data": [{
        "id": 111, "abbreviation": "NIV11", "localized_title": "New International Version",
        "language_tag": "en", "copyright": "Synthetic copyright",
    }]}))
    respx.get("https://youversion.test/v1/bibles/111/passages/JHN.3.16").mock(
        return_value=httpx.Response(200, json={"content": "Synthetic YouVersion verse."})
    )
    provider = YouVersionProvider("youversion", YouVersionProviderConfig(
        api_key_env="VERSEBRIDGE_TEST_YV_KEY", base_url="https://youversion.test/v1",
        bible_ids={"NIV": "111"},
    ))

    async def run():
        try:
            versions = await provider.list_versions()
            verses = await provider.fetch_passage(
                "NIV", "JHN", [VerseSpan(start=VersePoint(chapter=3, verse=16))]
            )
            return versions, verses
        finally:
            await provider.aclose()

    versions, verses = asyncio.run(run())
    assert versions[0].abbreviation == "NIV"
    assert versions[0].provider_version_id == "111"
    assert verses[0].text == "Synthetic YouVersion verse."
    assert verses[0].version == "NIV"


def test_youversion_missing_key_names_environment_variable_only() -> None:
    provider = YouVersionProvider(
        "youversion", YouVersionProviderConfig(api_key_env="DEFINITELY_MISSING_YOUVERSION_KEY")
    )

    async def run():
        try:
            await provider.list_versions()
        finally:
            await provider.aclose()

    with pytest.raises(BibleProviderError, match="DEFINITELY_MISSING_YOUVERSION_KEY"):
        asyncio.run(run())
