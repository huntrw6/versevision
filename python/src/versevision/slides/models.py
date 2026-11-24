from typing import Any, Literal

from pydantic import BaseModel, Field


class PlannedSlide(BaseModel):
    kind: Literal["verse", "point"] = "verse"
    body_text: str
    reference_text: str
    version_text: str
    copyright_text: str | None = None
    source_passage_index: int
    verse_keys: list[str] = Field(default_factory=list)


class SlidePlan(BaseModel):
    slides: list[PlannedSlide] = Field(default_factory=list)
    attribution_slides: list[PlannedSlide] = Field(default_factory=list)
    looks: Any = Field(default=None, exclude=True)      # theme slides chosen for verses/points (not part of the saved plan)

