from pathlib import Path
from typing import Protocol

from pydantic import BaseModel


class PresentationInfo(BaseModel):
    path: Path
    name: str
    uuid: str
    cue_count: int
    group_count: int
    application_version: str
    document_version: str


class WriteResult(BaseModel):
    path: Path
    backup_path: Path
    cue_uuid: str
    cue_count_before: int
    cue_count_after: int
    output_sha256: str


class PlanWriteResult(BaseModel):
    path: Path
    backup_path: Path
    cue_uuids: list[str]
    cue_count_before: int
    cue_count_after: int
    group_count_before: int
    group_count_after: int
    output_sha256: str
    original_sha256: str


class VerificationResult(BaseModel):
    valid: bool
    message: str


class PresentationWriter(Protocol):
    def inspect(self, path: Path) -> PresentationInfo: ...
    def append_synthetic_slide(self, path: Path, body_text: str, label: str) -> WriteResult: ...
    def verify(self, path: Path, expected: WriteResult) -> VerificationResult: ...
