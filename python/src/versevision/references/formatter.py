from .models import PassageReference, VersePoint, VerseSpan


def _point(point: VersePoint, inherited_chapter: int | None) -> str:
    if inherited_chapter == point.chapter:
        return str(point.verse)
    return f"{point.chapter}:{point.verse}"


def format_reference(reference: PassageReference) -> str:
    pieces: list[str] = []
    chapter = None
    for span in reference.spans:
        start = _point(span.start, chapter)
        if chapter is None:
            start = f"{span.start.chapter}:{span.start.verse}"
        end = ""
        if span.end:
            end = "-" + _point(span.end, span.start.chapter)
        pieces.append(start + end)
        chapter = span.start.chapter
    value = f"{reference.book_display_name} {' & '.join(pieces)}"
    return f"{value} {reference.version}" if reference.version else value

