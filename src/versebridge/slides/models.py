from pydantic import BaseModel, Field


class PlannedSlide(BaseModel):
    body_text: str
    reference_text: str
    version_text: str
    copyright_text: str | None = None
    source_passage_index: int
    verse_keys: list[str] = Field(default_factory=list)


class SlidePlan(BaseModel):
    slides: list[PlannedSlide] = Field(default_factory=list)
    attribution_slides: list[PlannedSlide] = Field(default_factory=list)

