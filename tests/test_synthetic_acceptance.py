import asyncio
from pathlib import Path

from versevision.bibles.cache import ScriptureCache
from versevision.bibles.registry import ProviderRegistry
from versevision.config import load_config
from versevision.workflow import dry_run_import


SOURCE = """Luke 11:1 NKJV
Luke 11:2a NKJV
Matthew 6:9 NKJV
Romans 8:15 NIV
Matthew 6:9 NKJV
Revelation 4:8-11 NIV
Psalms 103:1 NKJV
Matthew 6:10 NKJV
Matthew 6:11 NKJV
Philippians 4:19 NIV
Matthew 6:12 NKJV
Matthew 18:24, 27-28, 32-35 NKJV
Matthew 6:13 NKJV
Luke 22:31-32 NIV
1 Corinthians 10:13 NKJV
John 17:15 NKJV
Matthew 6:14-15 NKJV"""


def test_real_document_reference_set_fetches_synthetic_slides_with_continuation(tmp_path: Path) -> None:
    root = Path(__file__).parents[1]
    config = load_config(root / "tests" / "fixtures" / "synthetic-provider.toml")
    registry = ProviderRegistry(config.bible, ScriptureCache(tmp_path / "cache.sqlite3"))

    async def run():
        try:
            return await dry_run_import(
                SOURCE, config, no_fetch=False, provider_registry=registry,
                validate_target=False, non_interactive=True,
            )
        finally:
            await registry.aclose()

    result = asyncio.run(run())
    assert len(result.references) == 17
    assert len(result.slide_plan.slides) > 28
    continuation = [slide for slide in result.slide_plan.slides if slide.verse_keys == ["1CO.10.13"]]
    assert len(continuation) > 1
    assert all(slide.reference_text == "1 Corinthians 10:13" for slide in continuation)
    assert all("synthetic" in slide.body_text.casefold() for slide in result.slide_plan.slides)
    assert result.fetched is True
    assert result.files_changed is False
