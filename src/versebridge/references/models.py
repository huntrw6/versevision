from pydantic import BaseModel, Field


class VersePoint(BaseModel):
    chapter: int = Field(gt=0)
    verse: int = Field(gt=0)


class VerseSpan(BaseModel):
    start: VersePoint
    end: VersePoint | None = None


class PassageReference(BaseModel):
    book_id: str
    book_display_name: str
    spans: list[VerseSpan]
    version: str | None = None
    source_text: str
    source_start: int
    source_end: int
    normalized_text: str
    warnings: list[str] = Field(default_factory=list)


class ParseIssue(BaseModel):
    source_text: str
    source_start: int
    source_end: int
    message: str
    suggestion: str | None = None


class ParseResult(BaseModel):
    references: list[PassageReference] = Field(default_factory=list)
    issues: list[ParseIssue] = Field(default_factory=list)

