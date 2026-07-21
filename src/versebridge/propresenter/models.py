from __future__ import annotations

from pathlib import Path

from pydantic import BaseModel, ConfigDict, Field


class ProPresenterVersion(BaseModel):
    name: str
    platform: str
    os_version: str
    host_description: str
    api_version: str


class Library(BaseModel):
    uuid: str
    name: str
    index: int


class PresentationSummary(BaseModel):
    uuid: str
    name: str
    index: int
    library_uuid: str | None = None
    library_name: str | None = None


class LibraryContents(BaseModel):
    update_type: str | None = None
    items: list[PresentationSummary] = Field(default_factory=list)


class Identifier(BaseModel):
    uuid: str
    name: str
    index: int = 0


class SlideSize(BaseModel):
    width: float
    height: float


class ApiSlide(BaseModel):
    enabled: bool = True
    notes: str = ""
    text: str = ""
    label: str = ""
    size: SlideSize | None = None


class ApiGroup(BaseModel):
    name: str
    color: dict | None = None
    slides: list[ApiSlide] = Field(default_factory=list)


class PresentationMetadata(BaseModel):
    id: Identifier
    groups: list[ApiGroup] = Field(default_factory=list)
    has_timeline: bool = False
    presentation_path: Path
    destination: str | None = None
    has_timeline_items: bool = False


class PresentationResponse(BaseModel):
    presentation: PresentationMetadata


class PlaylistItem(BaseModel):
    model_config = ConfigDict(extra="allow")

    id: Identifier | None = None
    field_type: str | None = None
    children: list["PlaylistItem"] = Field(default_factory=list)
