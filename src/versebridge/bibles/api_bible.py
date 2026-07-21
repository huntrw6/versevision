from __future__ import annotations

import re

from bs4 import BeautifulSoup
import httpx

from versebridge.config import ApiBibleProviderConfig
from versebridge.credentials import provider_api_key
from versebridge.errors import BibleProviderError
from versebridge.references.models import VerseSpan
from versebridge.versions.models import BibleVersion
from .models import BibleVerse, ProviderHealth


_LEADING_VERSE_MARKER = re.compile(r"^\[\d+(?:-\d+)?\]\s*")


class ApiBibleProvider:
    def __init__(self, name: str, config: ApiBibleProviderConfig):
        self.provider_name = name
        self.config = config
        self._api_key = provider_api_key(config.api_key_env, "API_BIBLE_API_KEY")
        self._client = httpx.AsyncClient(base_url=config.base_url.rstrip("/"), timeout=config.timeout_seconds)
        self._versions: list[BibleVersion] | None = None

    async def aclose(self) -> None:
        await self._client.aclose()

    def _headers(self) -> dict[str, str]:
        if not self._api_key:
            raise BibleProviderError(
                f"API.Bible key is missing. Set the {self.config.api_key_env} environment variable."
            )
        return {"api-key": self._api_key}

    async def _get(self, route: str, **params):
        try:
            response = await self._client.get(route, headers=self._headers(), params=params)
            response.raise_for_status()
            return response.json()
        except httpx.TimeoutException as exc:
            raise BibleProviderError("API.Bible request timed out; no presentation files changed") from exc
        except httpx.HTTPStatusError as exc:
            status = exc.response.status_code
            reason = "invalid API key" if status in {401, 403} else "rate limit exceeded" if status == 429 else f"HTTP {status}"
            raise BibleProviderError(f"API.Bible request failed: {reason}; no presentation files changed") from exc
        except (httpx.HTTPError, ValueError) as exc:
            raise BibleProviderError(f"API.Bible response failed: {exc}; no presentation files changed") from exc

    async def list_versions(self) -> list[BibleVersion]:
        if self._versions is not None:
            return self._versions
        payload = await self._get("/bibles", language="eng")
        versions: list[BibleVersion] = []
        for item in payload.get("data", []):
            abbreviation = (item.get("abbreviationLocal") or item.get("abbreviation") or "").upper()
            if not abbreviation or not item.get("id"):
                continue
            language = item.get("language") or {}
            versions.append(BibleVersion(
                abbreviation=abbreviation,
                name=item.get("nameLocal") or item.get("name") or abbreviation,
                language=language.get("id") or "eng",
                provider=self.provider_name,
                provider_version_id=item["id"],
                copyright=item.get("copyright"),
                attribution=item.get("description"),
            ))
        self._versions = versions
        return versions

    async def _version(self, abbreviation: str) -> BibleVersion:
        matches = [item for item in await self.list_versions() if item.abbreviation.casefold() == abbreviation.casefold()]
        configured_id = next((value for key, value in self.config.bible_ids.items() if key.casefold() == abbreviation.casefold()), None)
        if configured_id:
            configured = next((item for item in matches if item.provider_version_id == configured_id), None)
            if not configured:
                raise BibleProviderError(f"Configured API.Bible ID for {abbreviation} is not available to this key")
            return configured
        if not matches:
            raise BibleProviderError(f"API.Bible key does not provide translation {abbreviation}")
        if len(matches) > 1:
            raise BibleProviderError(
                f"API.Bible returned multiple Bible IDs for {abbreviation}; configure providers.{self.provider_name}.bible_ids.{abbreviation}"
            )
        return matches[0]

    async def fetch_passage(self, version: str, book_id: str, spans: list[VerseSpan]) -> list[BibleVerse]:
        selected = await self._version(version)
        verses: list[BibleVerse] = []
        for span in spans:
            end = span.end or span.start
            if end.chapter != span.start.chapter:
                raise BibleProviderError("Cross-chapter API.Bible ranges are not enabled yet; no files changed")
            for number in range(span.start.verse, end.verse + 1):
                verse_id = f"{book_id}.{span.start.chapter}.{number}"
                payload = await self._get(
                    f"/bibles/{selected.provider_version_id}/verses/{verse_id}",
                    **{"content-type": "text", "include-notes": "false", "include-titles": "false"},
                )
                data = payload.get("data") or {}
                content = BeautifulSoup(data.get("content") or "", "html.parser").get_text(" ", strip=True)
                content = _LEADING_VERSE_MARKER.sub("", content)
                if not content:
                    raise BibleProviderError(f"API.Bible returned no text for {verse_id}")
                verses.append(BibleVerse(
                    book_id=book_id, chapter=span.start.chapter, verse=number, text=content, version=selected.abbreviation,
                    copyright=data.get("copyright") or selected.copyright,
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
                healthy=bool(versions), message=f"API.Bible exposes {len(versions)} translation(s)",
                provider=self.provider_name, versions=[item.abbreviation for item in versions],
            )
        except BibleProviderError as exc:
            return ProviderHealth(healthy=False, message=str(exc), provider=self.provider_name)
