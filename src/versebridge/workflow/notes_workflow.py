from __future__ import annotations

from pydantic import BaseModel, Field

from versebridge.bibles import ProviderRegistry
from versebridge.config import AppConfig
from versebridge.errors import VerseBridgeError
from versebridge.notes import NoteDocument, PointItem, classify_document
from versebridge.propresenter.api_client import ProPresenterApiClient
from versebridge.propresenter.selection import select_unique_presentation
from versebridge.references.models import PassageReference
from versebridge.slides.models import PlannedSlide, SlidePlan
from versebridge.slides.planner import placeholder_passages, plan_slides
from versebridge.versions.registry import canonical_version
from .result import TargetResult


class ProblemEntry(BaseModel):
    line: int
    text: str
    problem: str


class NotesImportResult(BaseModel):
    slide_plan: SlidePlan = Field(default_factory=SlidePlan)
    verse_count: int = 0
    point_count: int = 0
    skipped: list[ProblemEntry] = Field(default_factory=list)     # intentionally left out
    review: list[ProblemEntry] = Field(default_factory=list)      # unclear lines, need a decision
    failed: list[ProblemEntry] = Field(default_factory=list)      # wanted but could not be added
    notices: list[ProblemEntry] = Field(default_factory=list)     # corrections the tool made
    target: TargetResult | None = None
    fetched: bool = False
    files_changed: bool = False

    @property
    def needs_attention(self) -> bool:
        return bool(self.review or self.failed)


def resolve_note_versions(
    document: NoteDocument, *, default: str | None
) -> tuple[list[tuple[int, PassageReference | None, str]], list[ProblemEntry]]:
    """Give every verse a version: its own, else the nearest explicit one above, else the default."""
    notices: list[ProblemEntry] = []
    resolved: list[tuple[int, PassageReference | None, str]] = []
    last_explicit: str | None = None
    default_name = canonical_version(default) if default else None
    for item in document.verses:
        reference = item.reference
        for warning in item.warnings:
            if "read as" in warning or "unrecognized" in warning:
                notices.append(ProblemEntry(line=item.line, text=reference.source_text.strip(), problem=warning))
        if reference.version:
            last_explicit = reference.version
            resolved.append((item.line, reference, ""))
            continue
        chosen = last_explicit or default_name
        if not chosen:
            resolved.append((item.line, None, "no version on this verse, none above it, and no default version is set"))
            continue
        source = "nearest version above" if last_explicit else "default version"
        notices.append(ProblemEntry(line=item.line, text=reference.normalized_text, problem=f"no version listed; used {chosen} ({source})"))
        resolved.append((item.line, reference.model_copy(update={"version": chosen}), ""))
    return resolved, notices


async def plan_notes(
    source: str,
    config: AppConfig,
    *,
    default_version: str | None = None,
    no_fetch: bool = False,
    target_uuid: str | None = None,
    target_name: str | None = None,
    library: str | None = None,
    provider_registry: ProviderRegistry | None = None,
) -> NotesImportResult:
    document = classify_document(source, config.bible.version_aliases)
    result = NotesImportResult(fetched=not no_fetch)
    for item in document.skipped:
        result.skipped.append(ProblemEntry(line=item.line, text=item.text, problem=f"{item.category}: {item.reason}"))
    for item in document.review:
        result.review.append(ProblemEntry(line=item.line, text=item.text, problem=item.reason))
    for issue in document.issues:
        result.failed.append(ProblemEntry(line=0, text=issue.source_text, problem=f"invalid reference: {issue.message}"))

    default = default_version or config.bible.default_version
    if default and not canonical_version(default):
        raise VerseBridgeError(f"Unknown Bible version: {default}")
    versions, notices = resolve_note_versions(document, default=default)
    result.notices = notices
    resolved_refs = {id(item): entry for item, entry in zip(document.verses, versions)}

    registry = provider_registry
    owns = False
    if not no_fetch and registry is None:
        registry = ProviderRegistry(config.bible)
        owns = True
    slides: list[PlannedSlide] = []
    group_index = 0
    try:
        for item in document.items:
            if item.kind == "point":
                assert isinstance(item, PointItem)
                slides.append(PlannedSlide(
                    kind="point", body_text=item.display, reference_text="", version_text="",
                    source_passage_index=group_index,
                ))
                group_index += 1
                result.point_count += 1
                for warning in item.warnings:
                    result.notices.append(ProblemEntry(line=item.line, text=item.display[:60], problem=warning))
            elif item.kind == "verse":
                line, reference, why = resolved_refs[id(item)]
                if reference is None:
                    result.failed.append(ProblemEntry(line=line, text=item.reference.normalized_text, problem=why))
                    continue
                try:
                    passage = (await registry.fetch(reference)) if registry else placeholder_passages([reference])[0]
                    if not passage.verses:
                        raise VerseBridgeError("provider returned no verses")
                except Exception as exc:  # one unavailable verse never stops the rest
                    result.failed.append(ProblemEntry(
                        line=line, text=reference.normalized_text,
                        problem=f"not added: {exc}",
                    ))
                    continue
                planned = plan_slides([passage], config.slides).slides
                for slide in planned:
                    slide.source_passage_index = group_index
                slides.extend(planned)
                group_index += 1
                result.verse_count += 1
    finally:
        if owns and registry:
            await registry.aclose()
    result.slide_plan = SlidePlan(slides=slides)

    if target_uuid or target_name:
        pp = config.propresenter
        async with ProPresenterApiClient(pp.host, pp.port, pp.timeout_seconds) as client:
            selected = select_unique_presentation(await client.presentations(), uuid=target_uuid, name=target_name, library=library)
            metadata = await client.presentation(selected.uuid)
            result.target = TargetResult(
                library_name=selected.library_name or "", presentation_name=selected.name,
                uuid=selected.uuid, path=metadata.presentation_path,
            )
    return result
