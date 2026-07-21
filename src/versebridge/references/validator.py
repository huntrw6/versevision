from .books import Book
from .models import VerseSpan

# Complete versification is deliberately a data concern. These bounds cover the
# acceptance corpus and prevent the dangerous silent acceptance examples.
KNOWN_VERSE_COUNTS = {
    ("JHN", 1): 51, ("JHN", 3): 36, ("JHN", 4): 54,
    ("ROM", 8): 39, ("PSA", 23): 6, ("1JN", 4): 21,
    ("2CO", 5): 21, ("SNG", 2): 17, ("JUD", 1): 25,
}


def validate_spans(book: Book, spans: list[VerseSpan]) -> list[str]:
    errors: list[str] = []
    previous_end = None
    for span in spans:
        end = span.end or span.start
        points = ((span.start, "starting"),) if span.end is None else ((span.start, "starting"), (end, "ending"))
        for point, label in points:
            if point.chapter > book.chapters:
                errors.append(f"{book.name} has no chapter {point.chapter}")
            maximum = KNOWN_VERSE_COUNTS.get((book.id, point.chapter))
            if maximum and point.verse > maximum:
                errors.append(f"{book.name} {point.chapter} has no verse {point.verse}")
        if (end.chapter, end.verse) < (span.start.chapter, span.start.verse):
            errors.append("range end precedes range start")
        current_start = (span.start.chapter, span.start.verse)
        if previous_end and current_start <= previous_end:
            errors.append("duplicate or overlapping spans")
        previous_end = (end.chapter, end.verse)
    return errors
