from __future__ import annotations

import httpx

from versevision.config import YouVersionProviderConfig
from versevision.credentials import provider_api_key
from versevision.errors import BibleProviderError
from versevision.references.models import VerseSpan
from versevision.versions.models import BibleVersion
from .models import BibleVerse, ProviderHealth


class YouVersionProvider:
    def __init__(self, name: str, config: YouVersionProviderConfig):
        self.provider_name = name
        self.config = config
        self._api_key = provider_api_key(config.api_key_env, "YOUVERSION_API_KEY")
        self._client = httpx.AsyncClient(base_url=config.base_url.rstrip("/"), timeout=config.timeout_seconds)
        self._versions: list[BibleVersion] | None = None

    async def aclose(self) -> None:
        await self._client.aclose()

    def _headers(self) -> dict[str, str]:
        if not self._api_key:
            raise BibleProviderError(
                f"YouVersion key is missing. Set the {self.config.api_key_env} environment variable."
            )
        return {"X-YVP-App-Key": self._api_key}

    async def _get(self, route: str, **params):
        try:
            response = await self._client.get(route, headers=self._headers(), params=params)
            response.raise_for_status()
            return response.json()
        except httpx.TimeoutException as exc:
            raise BibleProviderError("YouVersion request timed out; no presentation files changed") from exc
        except httpx.HTTPStatusError as exc:
            status = exc.response.status_code
            reason = "invalid API key or unavailable license" if status in {401, 403} else "rate limit exceeded" if status == 429 else f"HTTP {status}"
            raise BibleProviderError(f"YouVersion request failed: {reason}; no presentation files changed") from exc
        except (httpx.HTTPError, ValueError) as exc:
            raise BibleProviderError(f"YouVersion response failed: {exc}; no presentation files changed") from exc

    async def list_versions(self) -> list[BibleVersion]:
        if self._versions is not None:
            return self._versions
        payload = await self._get(
            "/bibles", **{"language_ranges[]": "en", "page_size": 99}
        )
        configured_aliases = {str(value): key.upper() for key, value in self.config.bible_ids.items()}
        versions: list[BibleVersion] = []
        for item in payload.get("data", []):
            identifier = item.get("id")
            abbreviation = configured_aliases.get(str(identifier)) or (
                item.get("localized_abbreviation") or item.get("abbreviation") or ""
            ).upper()
            if identifier is None or not abbreviation:
                continue
            versions.append(BibleVersion(
                abbreviation=abbreviation,
                name=item.get("localized_title") or item.get("title") or abbreviation,
                language=item.get("language_tag") or "en",
                provider=self.provider_name,
                provider_version_id=str(identifier),
                copyright=item.get("copyright"),
                attribution=item.get("promotional_content") or item.get("publisher_url"),
            ))
        self._versions = versions
        return versions

    async def _version(self, abbreviation: str) -> BibleVersion:
        matches = [item for item in await self.list_versions() if item.abbreviation.casefold() == abbreviation.casefold()]
        configured_id = next(
            (str(value) for key, value in self.config.bible_ids.items() if key.casefold() == abbreviation.casefold()),
            None,
        )
        if configured_id:
            configured = next((item for item in matches if item.provider_version_id == configured_id), None)
            if not configured:
                raise BibleProviderError(f"Configured YouVersion ID for {abbreviation} is not available to this key")
            return configured
        if not matches:
            raise BibleProviderError(f"YouVersion key does not provide translation {abbreviation}")
        if len(matches) > 1:
            raise BibleProviderError(
                f"YouVersion returned multiple Bible IDs for {abbreviation}; configure providers.{self.provider_name}.bible_ids.{abbreviation}"
            )
        return matches[0]

    async def fetch_passage(self, version: str, book_id: str, spans: list[VerseSpan]) -> list[BibleVerse]:
        selected = await self._version(version)
        verses: list[BibleVerse] = []
        for span in spans:
            end = span.end or span.start
            if end.chapter != span.start.chapter:
                raise BibleProviderError("Cross-chapter YouVersion ranges are not enabled yet; no files changed")
            for number in range(span.start.verse, end.verse + 1):
                verse_id = f"{book_id}.{span.start.chapter}.{number}"
                payload = await self._get(
                    f"/bibles/{selected.provider_version_id}/passages/{verse_id}",
                    format="text", include_headings="false", include_notes="false",
                )
                content = " ".join((payload.get("content") or "").split())
                if not content:
                    raise BibleProviderError(f"YouVersion returned no text for {verse_id}")
                verses.append(BibleVerse(
                    book_id=book_id, chapter=span.start.chapter, verse=number, text=content,
                    version=selected.abbreviation, copyright=selected.copyright,
                    attribution=selected.attribution,
                ))
        return verses

    async def health_check(self) -> ProviderHealth:
        if not self._api_key:
            return ProviderHealth(
                healthy=False, message=f"Environment variable {self.config.api_key_env} is not set",
                provider=self.provider_name,
            )
        try:
            versions = await self.list_versions()
            return ProviderHealth(
                healthy=bool(versions), message=f"YouVersion exposes {len(versions)} translation(s)",
                provider=self.provider_name, versions=[item.abbreviation for item in versions],
            )
        except BibleProviderError as exc:
            return ProviderHealth(healthy=False, message=str(exc), provider=self.provider_name)
