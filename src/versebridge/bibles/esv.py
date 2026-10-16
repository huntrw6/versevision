from __future__ import annotations

import httpx

from versebridge.config import EsvProviderConfig
from versebridge.credentials import provider_api_key
from versebridge.errors import BibleProviderError
from versebridge.references.books import BOOKS
from versebridge.references.models import VerseSpan
from versebridge.versions.models import BibleVersion
from .models import BibleVerse, ProviderHealth

_BOOK_NAMES = {book.id: book.name for book in BOOKS}
COPYRIGHT = (
    "Scripture quotations are from the ESV® Bible (The Holy Bible, English Standard Version®), "
    "© 2001 by Crossway, a publishing ministry of Good News Publishers. Used by permission. All rights reserved. "
    "www.esv.org"
)


class EsvProvider:
    """Crossway ESV API. Results are never cached: the terms limit local storage."""

    cacheable = False

    def __init__(self, name: str, config: EsvProviderConfig):
        self.provider_name = name
        self.config = config
        self._api_key = provider_api_key(config.api_key_env, "ESV_API_KEY")
        self._client = httpx.AsyncClient(base_url=config.base_url.rstrip("/"), timeout=config.timeout_seconds)

    async def aclose(self) -> None:
        await self._client.aclose()

    def _headers(self) -> dict[str, str]:
        if not self._api_key:
            raise BibleProviderError(f"ESV key is missing. Set the {self.config.api_key_env} environment variable.")
        return {"Authorization": f"Token {self._api_key}"}

    async def _get(self, route: str, **params):
        try:
            response = await self._client.get(route, headers=self._headers(), params=params)
            response.raise_for_status()
            return response.json()
        except httpx.TimeoutException as exc:
            raise BibleProviderError("ESV request timed out; no presentation files changed") from exc
        except httpx.HTTPStatusError as exc:
            status = exc.response.status_code
            reason = "invalid API key" if status in {401, 403} else "rate limit exceeded" if status == 429 else f"HTTP {status}"
            raise BibleProviderError(f"ESV request failed: {reason}; no presentation files changed") from exc
        except (httpx.HTTPError, ValueError) as exc:
            raise BibleProviderError(f"ESV response failed: {exc}; no presentation files changed") from exc

    async def list_versions(self) -> list[BibleVersion]:
        return [BibleVersion(
            abbreviation="ESV", name="English Standard Version", provider=self.provider_name,
            provider_version_id="ESV", copyright=COPYRIGHT,
        )]

    async def _verse(self, book_id: str, chapter: int, verse: int) -> str:
        name = _BOOK_NAMES.get(book_id)
        if not name:
            raise BibleProviderError(f"ESV: unknown book {book_id}")
        payload = await self._get(
            "/passage/text/", q=f"{name} {chapter}:{verse}",
            **{
                "include-passage-references": "false", "include-verse-numbers": "false",
                "include-first-verse-numbers": "false", "include-footnotes": "false",
                "include-footnote-body": "false", "include-headings": "false",
                "include-short-copyright": "false", "include-copyright": "false",
                "include-passage-horizontal-lines": "false", "include-heading-horizontal-lines": "false",
                "indent-poetry": "false", "indent-paragraphs": "0", "indent-declares": "0",
                "indent-psalm-doxology": "0", "line-length": "0",
            },
        )
        passages = payload.get("passages") or []
        return " ".join(" ".join(passages).split())

    async def fetch_passage(self, version: str, book_id: str, spans: list[VerseSpan]) -> list[BibleVerse]:
        if version.casefold() != "esv":
            raise BibleProviderError(f"ESV provider does not serve {version}")
        verses: list[BibleVerse] = []
        for span in spans:
            end = span.end or span.start
            if end.chapter != span.start.chapter:
                raise BibleProviderError("Cross-chapter ESV ranges are not enabled yet; no files changed")
            for number in range(span.start.verse, end.verse + 1):
                text = await self._verse(book_id, span.start.chapter, number)
                if not text:
                    raise BibleProviderError(f"ESV returned no text for {book_id} {span.start.chapter}:{number}")
                verses.append(BibleVerse(
                    book_id=book_id, chapter=span.start.chapter, verse=number, text=text,
                    version="ESV", copyright=COPYRIGHT,
                ))
        return verses

    async def health_check(self) -> ProviderHealth:
        if not self._api_key:
            return ProviderHealth(healthy=False, message=f"Environment variable {self.config.api_key_env} is not set", provider=self.provider_name)
        try:
            await self._verse("JHN", 3, 16)
            return ProviderHealth(healthy=True, message="ESV API reachable", provider=self.provider_name, versions=["ESV"])
        except BibleProviderError as exc:
            return ProviderHealth(healthy=False, message=str(exc), provider=self.provider_name)
