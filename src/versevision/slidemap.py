"""A map of which Bible verses are already slides in a ProPresenter presentation, and where.

Looking a verse up in ProPresenter's Bible takes about a second. A slide that is already in the presentation can be put on screen
in about 0.1 s through the API (``/v1/presentation/<uuid>/<slide index>/trigger``). So before looking anything up, ask this map.

The map is built from the slides' own text, never from what we think we loaded: every Bible-lookup slide starts with its reference
("James 1:3 (ESV)"), so rebuilding it after any change (preloading, a lookup, the operator editing) is always correct.
"""
from __future__ import annotations

import re
from dataclasses import dataclass, field

from .references.parser import parse_references

_FIRST_LINE = re.compile(r"^\s*([1-3]?\s?[A-Za-z][A-Za-z .]*?\s+\d{1,3}:\d{1,3})\s*(?:[-–]\s*\d+\s*)?\(([A-Za-z0-9 ]{2,12})\)")


@dataclass(frozen=True)
class Entry:
    index: int            # slide index inside the presentation: what the trigger URL needs
    version: str          # as printed on the slide, e.g. "ESV"


def _norm_version(v: str) -> str:
    v = v.upper().replace(" ", "")
    return {"NASB2020": "NASB", "NASB1995": "NASB"}.get(v, v)


def parse_slide(text: str) -> tuple[str, int, int, str] | None:
    """(book id, chapter, verse, version) if this slide's text starts with a verse reference like "James 1:3 (ESV)", else None."""
    m = _FIRST_LINE.match((text or "").strip())
    if not m:
        return None
    parsed = parse_references(f"{m.group(1)} ({m.group(2)})")
    refs = getattr(parsed, "references", None) or []
    if not refs or not refs[0].spans:
        return None
    span = refs[0].spans[0]
    return refs[0].book_id, span.start.chapter, span.start.verse, _norm_version(m.group(2))


@dataclass
class SlideMap:
    """Verse slides of ONE presentation."""
    uuid: str = ""
    count: int = 0
    verses: dict[tuple[str, int, int], list[Entry]] = field(default_factory=dict)

    @classmethod
    def from_texts(cls, uuid: str, texts: list[str]) -> "SlideMap":
        m = cls(uuid=uuid, count=len(texts))
        for i, text in enumerate(texts):
            hit = parse_slide(text)
            if hit:
                book, chapter, verse, version = hit
                m.verses.setdefault((book, chapter, verse), []).append(Entry(i, version))
        return m

    def find(self, book_id: str, chapter: int, verse: int, version: str = "", prefer: str = "") -> Entry | None:
        """The slide for this verse. With ``version`` only that Bible counts. Otherwise the ``prefer``red one (the Bible last used),
        else the most recently added."""
        entries = self.verses.get((book_id, chapter, verse), [])
        if not entries:
            return None
        if version:
            want = _norm_version(version)
            same = [e for e in entries if e.version == want]
            return same[-1] if same else None
        if prefer:
            pref = [e for e in entries if e.version == _norm_version(prefer)]
            if pref:
                return pref[-1]
        return entries[-1]

    def ahead(self, book_id: str, chapter: int, verse: int, n: int, version: str = "", prefer: str = "") -> tuple[list[int], list[int]]:
        """Of the ``n`` verses after ``verse``: (those that are slides already, those that are not)."""
        have, missing = [], []
        for v in range(verse + 1, verse + 1 + n):
            (have if self.find(book_id, chapter, v, version, prefer) else missing).append(v)
        return have, missing
