from typing import Protocol

from versevision.references.models import VerseSpan
from versevision.versions.models import BibleVersion
from .models import BibleVerse, ProviderHealth


class BibleProvider(Protocol):
    async def list_versions(self) -> list[BibleVersion]: ...
    async def fetch_passage(self, version: str, book_id: str, spans: list[VerseSpan]) -> list[BibleVerse]: ...
    async def health_check(self) -> ProviderHealth: ...

