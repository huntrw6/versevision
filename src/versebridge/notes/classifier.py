"""Split a sermon document into verse slides, sermon-point slides and skipped notes.

The classifier is deliberately conservative: a line only becomes a slide on an
explicit signal (a reference, a numbered/lettered marker, or a ``[slide]``
tag). Everything else is reported as skipped or as needing review, never
guessed into a slide.
"""
from __future__ import annotations

import re
from typing import Literal

from pydantic import BaseModel, Field

from versebridge.references import parse_references
from versebridge.references.models import ParseIssue, PassageReference
from versebridge.versions.registry import is_real_version

# A numbered / lettered marker at the start of a line: "1)", "1.", "#1:", "a)", "2:".
_MARKER = re.compile(r"^\s*(?P<hash>#\s*)?(?P<label>\d{1,3}|[A-Za-z](?=[).]\s))\s*(?P<punct>[).:])\s*(?P<rest>.*)$")
_BULLET = re.compile(r"^\s*[•▪◦●‣*\-–—]\s+(?P<rest>\S.*)$")
_SLIDE_TAG = re.compile(r"^\s*\[\s*slide\s*\]\s*[-–—:]*\s*(?P<rest>.*)$", re.IGNORECASE)
_STAGE = re.compile(
    r"^\s*(show|play|put|start|cue|fade|transition|bring up|display|run|roll|when i (?:mention|say|get|talk|read|come)|"
    r"after (?:this|that)|at the end|end with|stop)\b",
    re.IGNORECASE,
)
_NOTE = re.compile(r"^\s*(\(.*\)\s*$|\[(?!\s*slide)[^\]]*\]\s*$|(note|notes|todo|to do|reminder|remember|ask|tell|check|fix|tbd|fyi)\b\s*[:\-]?)", re.IGNORECASE)
_URL = re.compile(r"https?://|www\.", re.IGNORECASE)
_MEDIA_WORDS = re.compile(r"\b(slide|photo|picture|image|video|clip|screen|portrait|graphic|manuscript)\b", re.IGNORECASE)
_RECAP = re.compile(r"^\s*recap\b", re.IGNORECASE)
_QUOTE_OPEN = ("“", '"', "‘", "'", "«")
_SENTENCE_END = (".", "?", "!", "”", '"', ":", ";")


class VerseItem(BaseModel):
    kind: Literal["verse"] = "verse"
    line: int
    reference: PassageReference
    warnings: list[str] = Field(default_factory=list)


class PointItem(BaseModel):
    kind: Literal["point"] = "point"
    line: int
    role: Literal["point", "list_item", "slide_note"]
    label: str            # "1)", "#2:", "a)", or "•"
    text: str
    warnings: list[str] = Field(default_factory=list)
    detail_end: int = 0

    @property
    def display(self) -> str:
        return f"{self.label} {self.text}".strip()


class SkippedItem(BaseModel):
    kind: Literal["skipped"] = "skipped"
    line: int
    category: Literal["stage_note", "header", "media_marker", "quoted_text"]
    text: str
    reason: str


class ReviewItem(BaseModel):
    kind: Literal["review"] = "review"
    line: int
    text: str
    reason: str


DocumentItem = VerseItem | PointItem | SkippedItem | ReviewItem


class NoteDocument(BaseModel):
    items: list[DocumentItem] = Field(default_factory=list)
    issues: list[ParseIssue] = Field(default_factory=list)

    @property
    def verses(self) -> list[VerseItem]:
        return [item for item in self.items if isinstance(item, VerseItem)]

    @property
    def points(self) -> list[PointItem]:
        return [item for item in self.items if isinstance(item, PointItem)]

    @property
    def skipped(self) -> list[SkippedItem]:
        return [item for item in self.items if isinstance(item, SkippedItem)]

    @property
    def review(self) -> list[ReviewItem]:
        return [item for item in self.items if isinstance(item, ReviewItem)]


def _lead_references(text: str, aliases) -> tuple[list[PassageReference], list[ParseIssue]]:
    """References that open a line (and any chained with ';' or ',' directly after)."""
    parsed = parse_references(text, aliases)
    taken: list[PassageReference] = []
    end = 0
    for reference in parsed.references:
        if not taken and reference.source_start > 2:
            break
        if taken and reference.source_start - end > 3:
            break
        taken.append(reference)
        end = reference.source_end
    return taken, parsed.issues


def _trailing_references(text: str, aliases) -> list[PassageReference]:
    """A reference at the END of a quoted block (``“verse text” John 3:16 NIV``)."""
    parsed = parse_references(text, aliases)
    if not parsed.references:
        return []
    last = parsed.references[-1]
    if len(text) - last.source_end > 4:
        return []
    chain = [last]
    for earlier in reversed(parsed.references[:-1]):
        if chain[0].source_start - earlier.source_end <= 3:
            chain.insert(0, earlier)
        else:
            break
    return chain


class _Para:
    def __init__(self, line: int, text: str, last_len: int) -> None:
        self.line, self.end, self.text, self.last_len = line, line, text.strip(), last_len
        self.kind = "other"
        self.refs: list[PassageReference] = []
        self.extra: dict = {}
        self.first = False

    def append(self, text: str, last_len: int, line: int = 0) -> None:
        self.text = f"{self.text} {text.strip()}".strip()
        self.last_len = last_len
        self.end = line or self.end


_EMAIL_HEADER = re.compile(r"^\s*(from|to|cc|bcc|subject|date|sent)\s*:|<[^>\s]+@[^>\s]+>|^\[quoted text hidden\]$", re.IGNORECASE)


def _header_like(words: list[str]) -> bool:
    return bool(words) and all(w[:1].isupper() or w[:1].isdigit() for w in words)


_DATE = re.compile(r"\d{1,2}/\d{1,2}/\d{2,4}")
_CLOSERS = ("”", '"', "’", "'")


def _marker(stripped: str):
    """A numbered/lettered point marker, or None. Rejects '12: 9 NKJV'-style verse fragments."""
    match = _MARKER.match(stripped)
    if not match or not match.group("rest").strip():
        return None
    if match.group("punct") == ":" and match.group("rest").lstrip()[:1].isdigit():
        return None
    return match


def _wrap_geometry(lines: list[str]) -> tuple[bool, int]:
    """Detect hard-wrapped text (PDF/email) and the width at which a line counts as full."""
    lengths = sorted(len(line.rstrip()) for line in lines if line.strip())
    if len(lengths) < 8:
        return False, 0
    p95 = lengths[max(0, int(len(lengths) * 0.95) - 1)]
    near = sum(1 for n in lengths if 0.86 * p95 <= n <= 1.05 * p95)
    return (60 <= p95 <= 140 and near / len(lengths) >= 0.15), int(0.86 * p95)


def classify_document(text: str, aliases: dict[str, str] | None = None) -> NoteDocument:
    """Classify a document into verse / point / skipped / review items, in document order."""
    lines = text.replace("\r\n", "\n").replace("\r", "\n").replace("\xa0", " ").split("\n")
    hard_wrap, threshold = _wrap_geometry(lines)

    def starts_new(stripped: str, current: _Para) -> bool:
        if _SLIDE_TAG.match(stripped) or _STAGE.match(stripped) or _URL.search(stripped) or _RECAP.match(stripped) or _NOTE.match(stripped):
            return True
        if _BULLET.match(stripped):
            return True
        if _marker(stripped) and not _is_reference_start(stripped, aliases):
            return True
        if stripped.startswith(_QUOTE_OPEN) and current.text.endswith(_CLOSERS):
            return True
        first = re.match(r"[\[(]?([A-Za-z0-9]+)", stripped)
        if _trailing_references(current.text, aliases) and not (first and is_real_version(first.group(1))):
            return True          # a reference at the end of a line closes that entry
        if _is_reference_start(stripped, aliases):
            # a reference line either finishes a quote above it, or starts a new verse entry
            return _is_reference_start(current.text, aliases) or bool(_trailing_references(current.text, aliases))
        return False

    paras: list[_Para] = []
    current: _Para | None = None
    for number, raw in enumerate(lines, 1):
        line = raw.rstrip()
        stripped = line.strip()
        if not stripped:
            current = None
            continue
        if (
            current is not None and hard_wrap and current.last_len >= threshold and not starts_new(stripped, current)
            and not (_marker(current.text) and current.text.endswith(_SENTENCE_END[:5]) and stripped[:1].isupper())
        ):
            current.append(stripped, len(line), number)
            continue
        current = _Para(number, stripped, len(line))
        paras.append(current)

    if paras:
        paras[0].first = True
    for para in paras:
        _label(para, aliases)
    return _build(paras, aliases)


def _label(para: _Para, aliases) -> None:
    text = para.text
    tag = _SLIDE_TAG.match(text)
    if tag:
        para.kind, para.text = "slide_tag", tag.group("rest").strip()
        return
    if _STAGE.match(text) or _URL.search(text) or _NOTE.match(text):
        para.kind = "stage"
        return
    bullet = _BULLET.match(text)
    if bullet and not _is_reference_start(bullet.group("rest"), aliases):
        para.kind, para.text = "marker", bullet.group("rest").strip()
        para.extra.update(label="•", number=None, bullet=True)
        para.refs = parse_references(para.text, aliases).references
        return
    if _RECAP.match(text):
        para.kind, para.extra["why"] = "header", "recap heading"
        return
    if _DATE.search(text) and not re.search(r"\d:\d", text):
        para.kind, para.extra["why"] = "header", "title / date line"
        return
    marker = _marker(text)
    if marker:
        inner, _ = _lead_references(marker.group("rest").strip(), aliases)
        if inner and not _is_reference_start(text, aliases):
            para.kind, para.refs = "verse", inner
            para.extra["numbered_verse"] = True
            return
    if marker and not _is_reference_start(text, aliases):
        label = marker.group("label")
        display = f"#{label}{marker.group('punct')}" if marker.group("hash") else f"{label}{marker.group('punct')}"
        para.kind = "marker"
        para.text = marker.group("rest").strip()
        para.extra.update(label=display, number=int(label) if label.isdigit() else None)
        para.refs = parse_references(para.text, aliases).references
        return
    leads, _ = _lead_references(text, aliases)
    if leads:
        para.kind, para.refs = "verse", leads
        return
    trailing = _trailing_references(text, aliases)
    if trailing:
        para.kind, para.refs = "verse", trailing
        return
    found = parse_references(text, aliases)
    if found.references:
        para.kind, para.refs = "midref", found.references
        return
    para.kind = "quote" if text.startswith(_QUOTE_OPEN) else "other"


def _is_reference_start(text: str, aliases) -> bool:
    leads, _ = _lead_references(text, aliases)
    return bool(leads)


def _build(paras: list[_Para], aliases) -> NoteDocument:
    doc = NoteDocument()
    last_point = 0
    seen_content = False
    for index, para in enumerate(paras):
        para.extra["seen_content"] = seen_content
        if para.kind in {"verse", "marker", "slide_tag", "midref"}:
            seen_content = True
        kind, line, text = para.kind, para.line, para.text
        if kind == "slide_tag":
            if not text:
                doc.items.append(ReviewItem(line=line, text="[slide]", reason="[slide] tag with no text"))
            elif _MEDIA_WORDS.search(text) and len(text) < 60:
                doc.items.append(SkippedItem(line=line, category="media_marker", text=text,
                                             reason="[slide] tag that points at an existing slide or media item"))
            else:
                doc.items.append(PointItem(line=line, role="slide_note", label="•", text=text, detail_end=line))
        elif kind == "stage":
            doc.items.append(SkippedItem(line=line, category="stage_note", text=text, reason="stage/production cue"))
        elif kind == "header":
            doc.items.append(SkippedItem(line=line, category="header", text=text, reason=para.extra.get("why", "heading")))
        elif kind == "marker":
            number, label = para.extra["number"], para.extra["label"]
            if number is None:
                role = "list_item"            # bullets and lettered items
            elif number == 1 or number == last_point + 1:
                role, last_point = "point", number
            else:
                role = "list_item"
            warnings = []
            if role == "list_item" and number is not None:
                warnings.append(f"numbered out of sequence (after point {last_point}); treated as a list item")
            for ref in para.refs:
                warnings.append(f"text mentions {ref.normalized_text}; not added as a verse slide")
            doc.items.append(PointItem(line=line, role=role, label=label, text=text, warnings=warnings, detail_end=line))
        elif kind == "verse":
            doc.issues.extend(_lead_references(text, aliases)[1])
            _emit_verses(doc, para, para.refs)
            if para.extra.get("numbered_verse"):
                doc.items[-1].warnings.append("numbered line holds a reference; treated as a verse, not a sermon point")
        elif kind == "midref":
            names = ", ".join(ref.normalized_text for ref in para.refs[:3])
            doc.items.append(ReviewItem(
                line=line, text=text,
                reason=f"mentions {names} inside text; put the reference on its own line to add it as a slide",
            ))
        elif kind == "quote":
            nxt = paras[index + 1] if index + 1 < len(paras) else None
            if nxt is not None and nxt.kind == "verse" and nxt.line - para.end <= 3:
                doc.items.append(SkippedItem(line=line, category="quoted_text", text=text,
                                             reason="verse text quoted above its reference; text is fetched from the Bible provider"))
            else:
                doc.items.append(ReviewItem(line=line, text=text, reason="quoted text with no reference"))
        else:
            item = _other(para, text)
            prior = doc.items[-1] if doc.items else None
            if (
                isinstance(item, ReviewItem) and isinstance(prior, PointItem) and prior.role in {"point", "list_item"}
                and line - prior.detail_end <= 2 and text.rstrip().endswith((".", "?", "!", "”", '"'))
            ):
                prior.text += "\n" + text
                prior.detail_end = line
                note = "joined unlabeled detail line(s) under this point"
                if note not in prior.warnings:
                    prior.warnings.append(note)
            else:
                doc.items.append(item)
    return doc


def _emit_verses(doc: NoteDocument, para: _Para, refs: list[PassageReference]) -> None:
    for ref in refs:
        if any("whole chapter" in warning for warning in ref.warnings):
            doc.items.append(ReviewItem(
                line=para.line, text=ref.source_text,
                reason="whole-chapter reference; add verse numbers to include it",
            ))
            continue
        doc.items.append(VerseItem(line=para.line, reference=ref, warnings=list(ref.warnings)))


def _other(para: _Para, text: str):
    line = para.line
    if re.match(r"^(part|week|session|series)\s*\d+\s*[:.-]", text, re.IGNORECASE) or text.endswith(":"):
        return SkippedItem(line=line, category="header", text=text, reason="title / heading line")
    words = text.split()
    if _EMAIL_HEADER.search(text):
        return SkippedItem(line=line, category="header", text=text, reason="email header line")
    if para.first and not text.endswith((".", "?", "!")):
        return SkippedItem(line=line, category="header", text=text, reason="document title (first line)")
    if len(words) <= 2 and _header_like(words) and not text.endswith((".", "?", "!")):
        return SkippedItem(line=line, category="header", text=text, reason="one or two capitalized words: looks like a heading")
    if len(text) <= 80 and not text.endswith((".", "?", "!")):
        if not para.extra.get("seen_content"):
            return SkippedItem(line=line, category="header", text=text, reason="title/preamble before the first verse or point")
        return ReviewItem(line=line, text=text, reason="short unlabeled line: a heading, or a point missing its number? add a number or bullet to make it a slide")
    return ReviewItem(line=line, text=text, reason="unlabeled line; not a reference, point, or known cue")
