import asyncio

from versebridge.bibles.cache import ScriptureCache
from versebridge.bibles.registry import ProviderRegistry
from versebridge.config import AppConfig, BibleConfig, LocalUsfmProviderConfig
from versebridge.workflow import dry_run_import


def test_fetched_dry_run_places_text_and_attribution_on_slides(tmp_path) -> None:
    usfm = tmp_path / "usfm"
    usfm.mkdir()
    (usfm / "JHN.usfm").write_text(
        "\\id JHN\n\\c 3\n\\v 16 Synthetic body text.\n", encoding="utf-8"
    )
    config = AppConfig(bible=BibleConfig(
        providers={"local_test": LocalUsfmProviderConfig(
            version="NKJV", path=usfm, copyright="Synthetic fixture copyright",
        )},
        version_routes={"NKJV": "local_test"},
    ))
    registry = ProviderRegistry(config.bible, ScriptureCache(tmp_path / "cache.sqlite3"))

    async def run():
        try:
            return await dry_run_import(
                "John 3:16 NKJV", config, no_fetch=False, provider_registry=registry,
                validate_target=False, non_interactive=True,
            )
        finally:
            await registry.aclose()

    result = asyncio.run(run())
    assert result.fetched is True
    assert result.files_changed is False
    assert result.slide_plan.slides[0].body_text == "16 Synthetic body text."
    assert result.slide_plan.slides[0].copyright_text == "Synthetic fixture copyright"
