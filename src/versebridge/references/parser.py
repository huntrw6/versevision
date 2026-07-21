import re

from .books import BOOKS, Book, recognize_book
from .formatter import format_reference
from .models import ParseIssue, ParseResult, PassageReference, VersePoint, VerseSpan
from .normalizer import normalize_fragment
from .tokenizer import Token, tokenize
from .validator import validate_spans
from versebridge.versions.registry import canonical_version


class _Cursor:
    def __init__(self, tokens: list[Token]): self.tokens, self.i, self.warnings = tokens, 0, []
    def peek(self, *kinds: str) -> Token | None:
        return self.tokens[self.i] if self.i < len(self.tokens) and self.tokens[self.i].kind in kinds else None
    def take(self, *kinds: str) -> Token | None:
        token = self.peek(*kinds)
        if token: self.i += 1
        return token
    def word(self, *values: str) -> Token | None:
        token = self.peek("WORD")
        if token and token.value.casefold() in values: self.i += 1; return token
        return None


def _version(cursor: _Cursor) -> str | None:
    opening = cursor.take("(", "[")
    token = cursor.peek("WORD")
    version = canonical_version(token.value) if token else None
    if not version:
        if opening: cursor.i -= 1
        return None
    cursor.i += 1
    if opening:
        expected = ")" if opening.kind == "(" else "]"
        if not cursor.take(expected): cursor.i -= 2; return None
    return version


def _consume_verse_suffix(cursor: _Cursor, number: Token) -> None:
    suffix = cursor.peek("WORD")
    if suffix and suffix.start == number.end and suffix.value.casefold() in {"a", "b", "c"}:
        cursor.i += 1
        cursor.warnings.append(
            f"verse-part suffix '{suffix.value.casefold()}' detected; the provider will retrieve the full verse"
        )


def _span(cursor: _Cursor, chapter: int | None) -> tuple[VerseSpan, int] | None:
    first = cursor.take("NUMBER")
    if not first: return None
    if cursor.take(":"):
        chapter = int(first.value)
        verse_token = cursor.take("NUMBER")
        if not verse_token: return None
        verse = int(verse_token.value)
        _consume_verse_suffix(cursor, verse_token)
    elif chapter is not None:
        verse = int(first.value)
        _consume_verse_suffix(cursor, first)
    else:
        # A bare number is a whole chapter. Represent it from verse 1 and warn later.
        chapter, verse = int(first.value), 1
    start = VersePoint(chapter=chapter, verse=verse)
    if cursor.take("-") or cursor.word("to", "through"):
        end_first = cursor.take("NUMBER")
        if not end_first: return None
        if cursor.take(":"):
            end_chapter = int(end_first.value)
            end_verse_token = cursor.take("NUMBER")
            if not end_verse_token: return None
            end_verse = int(end_verse_token.value)
            _consume_verse_suffix(cursor, end_verse_token)
        else:
            end_chapter, end_verse = chapter, int(end_first.value)
            _consume_verse_suffix(cursor, end_first)
        return VerseSpan(start=start, end=VersePoint(chapter=end_chapter, verse=end_verse)), chapter
    return VerseSpan(start=start), chapter


def _parse_after_book(text: str, source: str, book: Book, book_start: int, content_start: int) -> tuple[list[PassageReference], list[ParseIssue], int]:
    cursor = _Cursor(tokenize(text[content_start:], content_start))
    passages: list[PassageReference] = []
    issues: list[ParseIssue] = []
    # In one-chapter books, the conventional bare number is a verse (Jude 5),
    # while in multi-chapter books it denotes the whole chapter (Psalm 23).
    chapter = 1 if book.chapters == 1 else None
    logical_start = book_start
    while True:
        warning_start = len(cursor.warnings)
        spans: list[VerseSpan] = []
        parsed = _span(cursor, chapter)
        if not parsed: break
        span, chapter = parsed
        spans.append(span)
        while cursor.take("&", ",") or cursor.word("and"):
            checkpoint = cursor.i
            parsed = _span(cursor, chapter)
            if not parsed:
                cursor.i = checkpoint - 1
                break
            span, chapter = parsed
            spans.append(span)
        version = _version(cursor)
        end = cursor.tokens[cursor.i - 1].end if cursor.i else content_start
        raw = source[logical_start:end]
        reference = PassageReference(book_id=book.id, book_display_name=book.name, spans=spans, version=version,
            source_text=raw, source_start=logical_start, source_end=end, normalized_text="")
        reference.warnings.extend(cursor.warnings[warning_start:])
        errors = validate_spans(book, spans)
        if errors:
            issues.append(ParseIssue(source_text=raw, source_start=logical_start, source_end=end, message="; ".join(errors)))
        else:
            if book.chapters > 1 and len(spans) == 1 and spans[0].start.verse == 1 and ":" not in raw:
                reference.warnings.append("whole chapter reference; provider must expand chapter verses")
            reference.normalized_text = format_reference(reference)
            passages.append(reference)
        if not cursor.take(";"): break
        logical_start = cursor.tokens[cursor.i].start if cursor.i < len(cursor.tokens) else end
        # A named book after a semicolon is handled by the outer scanner.
        if cursor.peek("WORD"): break
    consumed = cursor.tokens[cursor.i - 1].end if cursor.i else content_start
    return passages, issues, consumed


def parse_references(source: str) -> ParseResult:
    normalized = normalize_fragment(source)
    result = ParseResult()
    occupied_until = 0
    for match in re.finditer(
        r"(?<![A-Za-z])(?:(?:[1-3]|I{1,3}|First|Second|Third)[ \t]*)?[A-Za-z]",
        normalized,
        re.IGNORECASE,
    ):
        start = match.start()
        if start < occupied_until: continue
        recognized = recognize_book(normalized, start)
        if not recognized: continue
        book, after = recognized
        refs, issues, consumed = _parse_after_book(normalized, source, book, start, after)
        if refs or issues:
            result.references.extend(refs); result.issues.extend(issues); occupied_until = consumed
    return result
