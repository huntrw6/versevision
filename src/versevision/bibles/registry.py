from __future__ import annotations

from versevision.config import ApiBibleProviderConfig, BibleConfig, EsvProviderConfig, LocalUsfmProviderConfig, YouVersionProviderConfig
from versevision.errors import BibleProviderError, ConfigurationError
from versevision.references.models import PassageReference

from .api_bible import ApiBibleProvider
from .esv import EsvProvider
from .base import BibleProvider
from .cache import ScriptureCache
from .local_usfm import LocalUsfmProvider
from .youversion import YouVersionProvider
from .models import ProviderHealth, ResolvedPassage


class ProviderRegistry:
    def __init__(self, config: BibleConfig, cache: ScriptureCache | None = None):
        self.config = config
        self.cache = cache or ScriptureCache()
        self.providers: dict[str, BibleProvider] = {}
        for name, provider_config in config.providers.items():
            if isinstance(provider_config, ApiBibleProviderConfig):
                self.providers[name] = ApiBibleProvider(name, provider_config)
            elif isinstance(provider_config, YouVersionProviderConfig):
                self.providers[name] = YouVersionProvider(name, provider_config)
            elif isinstance(provider_config, EsvProviderConfig):
                self.providers[name] = EsvProvider(name, provider_config)
            elif isinstance(provider_config, LocalUsfmProviderConfig):
                self.providers[name] = LocalUsfmProvider(name, provider_config)

    async def aclose(self) -> None:
        for provider in self.providers.values():
            close = getattr(provider, "aclose", None)
            if close:
                await close()
        self.cache.close()

    def _provider(self, version: str) -> tuple[str, BibleProvider]:
        route = next((provider for key, provider in self.config.version_routes.items() if key.casefold() == version.casefold()), None)
        if not route:
            raise ConfigurationError(f"No Bible provider route is configured for {version}")
        if route not in self.providers:
            raise ConfigurationError(f"Bible provider route {version} points to unknown provider {route}")
        return route, self.providers[route]

    async def fetch(self, reference: PassageReference) -> ResolvedPassage:
        import time as _time
        from versevision import runlog
        started = _time.monotonic()
        label = getattr(reference, "normalized_text", "") or str(reference)
        info = {"reference": label, "version": reference.version}
        try:
            passage, provider_name, source = await self._fetch(reference)
        except Exception as exc:
            runlog.event("bible_fetch", level="warn", error=str(exc), ms=round((_time.monotonic() - started) * 1000), **info)
            raise
        runlog.event("bible_fetch", provider=provider_name, source=source, verses=len(passage.verses),
                     ms=round((_time.monotonic() - started) * 1000), **info)
        return passage

    async def _fetch(self, reference: PassageReference) -> tuple[ResolvedPassage, str, str]:
        if not reference.version:
            raise BibleProviderError("Cannot fetch a reference without a resolved version")
        provider_name, provider = self._provider(reference.version)
        versions = await provider.list_versions()
        metadata = next((item for item in versions if item.abbreviation.casefold() == reference.version.casefold()), None)
        if not metadata or not metadata.provider_version_id:
            raise BibleProviderError(f"Provider {provider_name} does not expose {reference.version}")

        expected: list[tuple[int, int]] = []
        cacheable = True
        for span in reference.spans:
            end = span.end or span.start
            if end.chapter != span.start.chapter:
                cacheable = False
                break
            expected.extend((span.start.chapter, number) for number in range(span.start.verse, end.verse + 1))
        provider_config = self.config.providers[provider_name]
        cached = [
            self.cache.get(provider_name, metadata.provider_version_id, reference.book_id, chapter, verse)
            for chapter, verse in expected
        ] if cacheable and provider_config.cache_policy == "allowed" else []
        if cacheable and cached and all(cached):
            return ResolvedPassage(reference=reference, resolved_version=metadata.abbreviation, verses=cached), provider_name, "cache"

        verses = await provider.fetch_passage(reference.version, reference.book_id, reference.spans)
        for verse in verses if getattr(provider, "cacheable", True) else []:
            self.cache.put(provider_name, metadata.provider_version_id, verse)
        return ResolvedPassage(reference=reference, resolved_version=metadata.abbreviation, verses=verses), provider_name, (
            "live (not stored)" if not getattr(provider, "cacheable", True) else "live")

    async def health_checks(self) -> list[ProviderHealth]:
        if not self.providers:
            return [ProviderHealth(healthy=False, message="No Bible providers are configured")]
        return [await provider.health_check() for provider in self.providers.values()]
