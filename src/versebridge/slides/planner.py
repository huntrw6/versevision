from __future__ import annotations

from versebridge.bibles.models import BibleVerse, ResolvedPassage
from versebridge.config import SlideConfig
from versebridge.references.models import PassageReference, VerseSpan
from .models import PlannedSlide, SlidePlan
from .fitting import split_verse_text


def _span_label(reference: PassageReference, span: VerseSpan) -> str:
    end = span.end
    value = f"{reference.book_display_name} {span.start.chapter}:{span.start.verse}"
    if end:
        suffix = str(end.verse) if end.chapter == span.start.chapter else f"{end.chapter}:{end.verse}"
        value += f"-{suffix}"
    return value


def _verse_reference(reference: PassageReference, verse: BibleVerse, mode: str) -> str:
    if mode == "source":
        base = reference.normalized_text
        if reference.version and base.endswith(" " + reference.version):
            base = base[: -(len(reference.version) + 1)]
        return base
    if mode == "segment":
        for span in reference.spans:
            end = span.end or span.start
            if (span.start.chapter, span.start.verse) <= (verse.chapter, verse.verse) <= (end.chapter, end.verse):
                return _span_label(reference, span)
    return f"{reference.book_display_name} {verse.chapter}:{verse.verse}"


def plan_slides(passages: list[ResolvedPassage], config: SlideConfig | None = None) -> SlidePlan:
    settings = config or SlideConfig()
    slides: list[PlannedSlide] = []
    for passage_index, passage in enumerate(passages):
        for verse in passage.verses:
            body = f"{verse.verse} {verse.text}".rstrip() if settings.include_verse_numbers else verse.text
            chunks = split_verse_text(
                body,
                max_lines=settings.max_body_lines,
                line_width_units=settings.line_width_units,
                prefer_punctuation=settings.prefer_punctuation_breaks,
            )
            reference_text = _verse_reference(passage.reference, verse, settings.reference_label_mode)
            for chunk_index, chunk in enumerate(chunks):
                slides.append(PlannedSlide(
                    body_text=chunk,
                    reference_text=reference_text if settings.repeat_reference_on_continuation or chunk_index == 0 else "",
                    version_text=passage.resolved_version,
                    copyright_text=verse.copyright or verse.attribution,
                    source_passage_index=passage_index,
                    verse_keys=[verse.key],
                ))
    return SlidePlan(slides=slides)


def placeholder_passages(references: list[PassageReference]) -> list[ResolvedPassage]:
    """Expand same/cross-chapter spans for --no-fetch planning.

    Cross-chapter intermediate verse counts require a provider, so only explicit
    endpoints are represented when the span crosses chapters.
    """
    result: list[ResolvedPassage] = []
    for reference in references:
        verses: list[BibleVerse] = []
        version = reference.version or ""
        for span in reference.spans:
            end = span.end or span.start
            if end.chapter == span.start.chapter:
                verses.extend(BibleVerse(book_id=reference.book_id, chapter=span.start.chapter, verse=value, text="", version=version)
                              for value in range(span.start.verse, end.verse + 1))
            else:
                verses.append(BibleVerse(book_id=reference.book_id, chapter=span.start.chapter, verse=span.start.verse, text="", version=version))
                verses.append(BibleVerse(book_id=reference.book_id, chapter=end.chapter, verse=end.verse, text="", version=version))
        result.append(ResolvedPassage(reference=reference, resolved_version=version, verses=verses))
    return result
