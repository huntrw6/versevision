from __future__ import annotations

import re
from pathlib import Path

from versebridge.config import LocalUsfmProviderConfig
from versebridge.errors import BibleProviderError
from versebridge.references.models import VerseSpan
from versebridge.versions.models import BibleVersion
from .models import BibleVerse, ProviderHealth


_MARKER = re.compile(r"\\(?:f|x)\s.*?\\(?:f|x)\*|\\[A-Za-z0-9-]+\*?\s*", re.DOTALL)


def _clean(value: str) -> str:
    return " ".join(_MARKER.sub(" ", value).split())


class LocalUsfmProvider:
    def __init__(self, name: str, config: LocalUsfmProviderConfig):
        self.provider_name = name
        self.config = config
        self.version = config.version.upper()
        self.provider_version_id = str(config.path.resolve())
        self._books: dict[str, dict[tuple[int, int], BibleVerse]] | None = None

    def _load(self) -> dict[str, dict[tuple[int, int], BibleVerse]]:
        if self._books is not None:
            return self._books
        if not self.config.path.is_dir():
            raise BibleProviderError(f"USFM directory does not exist: {self.config.path}")
        books: dict[str, dict[tuple[int, int], BibleVerse]] = {}
        for path in sorted(self.config.path.glob("*")):
            if not path.is_file() or path.suffix.casefold() not in {".usfm", ".sfm", ".txt"}:
                continue
            raw = path.read_text(encoding="utf-8-sig")
            identifier = re.search(r"(?m)^\\id\s+([A-Z0-9]{3})\b", raw)
            if not identifier:
                continue
            book_id = identifier.group(1)
            chapter = 0
            paragraph_start = False
            poetry_indent = 0
            verses: dict[tuple[int, int], BibleVerse] = {}
            current: BibleVerse | None = None
            for line in raw.splitlines():
                chapter_match = re.match(r"\\c\s+(\d+)", line)
                if chapter_match:
                    chapter = int(chapter_match.group(1))
                    continue
                if re.match(r"\\p(?:\s|$)", line):
                    paragraph_start = True
                    continue
                poetry = re.match(r"\\q(\d*)\s*", line)
                if poetry:
                    poetry_indent = int(poetry.group(1) or 1)
                    line = line[poetry.end():]
                verse_match = re.match(r"\\v\s+(\d+)(?:-[0-9]+)?\s*(.*)", line)
                if verse_match and chapter:
                    number = int(verse_match.group(1))
                    current = BibleVerse(
                        book_id=book_id, chapter=chapter, verse=number, text=_clean(verse_match.group(2)),
                        version=self.version, paragraph_start=paragraph_start, poetry_indent=poetry_indent,
                        copyright=self.config.copyright, attribution=self.config.attribution,
                    )
                    verses[(chapter, number)] = current
                    paragraph_start = False
                    poetry_indent = 0
                elif current and line and not line.startswith("\\"):
                    current.text = _clean(f"{current.text} {line}")
            books[book_id] = verses
        self._books = books
        return books

    async def list_versions(self) -> list[BibleVersion]:
        return [BibleVersion(
            abbreviation=self.version, name=self.config.name or self.version, provider=self.provider_name,
            provider_version_id=self.provider_version_id, copyright=self.config.copyright,
            attribution=self.config.attribution,
        )]

    async def fetch_passage(self, version: str, book_id: str, spans: list[VerseSpan]) -> list[BibleVerse]:
        if version.upper() != self.version:
            raise BibleProviderError(f"Provider {self.provider_name} does not supply {version}")
        book = self._load().get(book_id)
        if not book:
            raise BibleProviderError(f"USFM book {book_id} was not found in {self.config.path}")
        selected: list[BibleVerse] = []
        for span in spans:
            end = span.end or span.start
            keys = sorted(key for key in book if (span.start.chapter, span.start.verse) <= key <= (end.chapter, end.verse))
            if not keys or keys[0] != (span.start.chapter, span.start.verse) or keys[-1] != (end.chapter, end.verse):
                raise BibleProviderError(f"USFM passage is incomplete for {book_id} {span.start.chapter}:{span.start.verse}")
            selected.extend(book[key].model_copy(deep=True) for key in keys)
        return selected

    async def health_check(self) -> ProviderHealth:
        try:
            count = len(self._load())
            return ProviderHealth(healthy=count > 0, message=f"Loaded {count} USFM book(s)", provider=self.provider_name, versions=[self.version])
        except BibleProviderError as exc:
            return ProviderHealth(healthy=False, message=str(exc), provider=self.provider_name)
