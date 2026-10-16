from __future__ import annotations

from pydantic import BaseModel, Field

from versevision.references.models import PassageReference


class BibleVerse(BaseModel):
    book_id: str
    chapter: int
    verse: int
    text: str
    version: str
    paragraph_start: bool = False
    poetry_indent: int = 0
    copyright: str | None = None
    attribution: str | None = None

    @property
    def key(self) -> str:
        return f"{self.book_id}.{self.chapter}.{self.verse}"


class ResolvedPassage(BaseModel):
    reference: PassageReference
    resolved_version: str
    verses: list[BibleVerse]


class ProviderHealth(BaseModel):
    healthy: bool
    message: str
    provider: str | None = None
    versions: list[str] = Field(default_factory=list)
