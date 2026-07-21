from pathlib import Path

from pydantic import BaseModel, Field

from versebridge.references.models import ParseIssue, PassageReference
from versebridge.slides.models import SlidePlan


class TargetResult(BaseModel):
    library_name: str
    presentation_name: str
    uuid: str
    path: Path


class DryRunResult(BaseModel):
    references: list[PassageReference] = Field(default_factory=list)
    issues: list[ParseIssue] = Field(default_factory=list)
    slide_plan: SlidePlan | None = None
    target: TargetResult | None = None
    fetched: bool = False
    files_changed: bool = False


class ImportResult(BaseModel):
    target: TargetResult
    reference_count: int
    slide_count: int
    versions: list[str] = Field(default_factory=list)
    backup_path: Path
    cue_count_before: int
    cue_count_after: int
    group_count_before: int
    group_count_after: int
    verification_result: str
    files_changed: bool = True
