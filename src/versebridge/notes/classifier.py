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

# A numbered / lettered marker at the start of a line: "1)", "1.", "#1:", "a)", "2:".
_MARKER = re.compile(r"^\s*(?P<hash>#\s*)?(?P<label>\d{1,3}|[A-Za-z](?=\)))\s*(?P<punct>[).:])\s*(?P<rest>.*)$")
_SLIDE_TAG = re.compile(r"^\s*\[\s*slide\s*\]\s*[-–—:]*\s*(?P<rest>.*)$", re.IGNORECASE)
_STAGE = re.compile(
    r"^\s*(show|play|put|start|cue|fade|transition|bring up|display|run|roll|when i (?:mention|say|get|talk|read|come)|"
    r"after (?:this|that)|at the end|end with|stop)\b",
    re.IGNORECASE,
)
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


class _Block:
    def __init__(self, kind: str, line: int, text: str, **extra) -> None:
        self.kind, self.line, self.text, self.extra = kind, line, text.strip(), extra
        self.last_line_len = len(text)

    def append(self, text: str) -> None:
        self.text = f"{self.text} {text.strip()}".strip()
        self.last_line_len = len(text)


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


def _line_width(lines: list[str]) -> int:
    lengths = sorted(len(line.rstrip()) for line in lines if line.strip())
    if not lengths:
        return 0
    return lengths[int(len(lengths) * 0.9)]


def classify_document(text: str, aliases: dict[str, str] | None = None) -> NoteDocument:
    """Classify every line of a document, preserving document order."""
    lines = text.replace("\r\n", "\n").replace("\r", "\n").split("\n")
    width = _line_width(lines)
    wrap_threshold = max(40, int(width * 0.8))

    blocks: list[_Block] = []
    current: _Block | None = None

    def wrapped(block: _Block | None) -> bool:
        """The previous line ran to the right margin, so the next one continues it."""
        return bool(block) and block.last_line_len >= wrap_threshold and not block.text.rstrip().endswith(_SENTENCE_END[:3])

    for number, raw in enumerate(lines, 1):
        line = raw.rstrip()
        if not line.strip():
            current = None
            continue
        stripped = line.strip()

        tag = _SLIDE_TAG.match(stripped)
        if tag:
            current = _Block("slide_tag", number, tag.group("rest"))
            blocks.append(current)
            continue
        if _STAGE.match(stripped) or _URL.search(stripped):
            current = _Block("stage", number, stripped)
            blocks.append(current)
            continue
        if _RECAP.match(stripped):
            current = _Block("header", number, stripped, why="recap heading")
            blocks.append(current)
            continue
        marker = _MARKER.match(stripped)
        if marker and not _is_reference_start(stripped, aliases):
            label = marker.group("label")
            punct = marker.group("punct")
            display = f"#{label}{punct}" if marker.group("hash") else f"{label}{punct}"
            current = _Block("marker", number, marker.group("rest"), label=display, number=int(label) if label.isdigit() else None,
                             letter=label if not label.isdigit() else None)
            blocks.append(current)
            continue
        leads, _ = _lead_references(stripped, aliases)
        if leads:
            current = _Block("verse", number, stripped)
            blocks.append(current)
            continue
        if current is not None and wrapped(current):
            current.append(stripped)
            continue
        if stripped.startswith(_QUOTE_OPEN):
            current = _Block("quote", number, stripped)
            blocks.append(current)
            continue
        current = _Block("other", number, stripped)
        blocks.append(current)

    return _build(blocks, aliases)


def _is_reference_start(text: str, aliases) -> bool:
    leads, _ = _lead_references(text, aliases)
    return bool(leads)


def _build(blocks: list[_Block], aliases) -> NoteDocument:
    doc = NoteDocument()
    last_point = 0
    seen_points = False
    index = 0
    while index < len(blocks):
        block = blocks[index]
        index += 1
        if block.kind == "slide_tag":
            if not block.text:
                doc.items.append(ReviewItem(line=block.line, text="[slide]", reason="[slide] tag with no text"))
            elif _MEDIA_WORDS.search(block.text) and len(block.text) < 60:
                doc.items.append(SkippedItem(
                    line=block.line, category="media_marker", text=block.text,
                    reason="[slide] tag that points at an existing slide or media item",
                ))
            else:
                doc.items.append(PointItem(line=block.line, role="slide_note", label="•", text=block.text))
        elif block.kind == "stage":
            doc.items.append(SkippedItem(line=block.line, category="stage_note", text=block.text, reason="stage/production cue"))
        elif block.kind == "header":
            doc.items.append(SkippedItem(line=block.line, category="header", text=block.text, reason=block.extra.get("why", "heading")))
        elif block.kind == "marker":
            number = block.extra["number"]
            label = block.extra["label"]
            if number is None:                      # lettered item, e.g. "a)"
                role = "list_item"
            elif number == 1 or number == last_point + 1:
                role = "point"
            else:
                role = "list_item"
            if role == "point":
                last_point, seen_points = number, True
            warnings = []
            if role == "list_item" and number is not None:
                warnings.append(f"numbered out of sequence (after point {last_point}); treated as a list item")
            if not block.text:
                doc.items.append(ReviewItem(line=block.line, text=label, reason="marker with no text"))
            else:
                doc.items.append(PointItem(line=block.line, role=role, label=label, text=block.text, warnings=warnings, detail_end=block.line))
        elif block.kind == "verse":
            refs, issues = _lead_references(block.text, aliases)
            doc.issues.extend(issues)
            _emit_verses(doc, block, refs)
        elif block.kind == "quote":
            refs = _trailing_references(block.text, aliases)
            nxt = blocks[index] if index < len(blocks) else None
            if refs:
                _emit_verses(doc, block, refs)
            elif nxt is not None and nxt.kind == "verse" and nxt.line - block.line <= 6:
                doc.items.append(SkippedItem(line=block.line, category="quoted_text", text=block.text, reason="verse text quoted above its reference; text is fetched from the Bible provider"))
            else:
                doc.items.append(ReviewItem(line=block.line, text=block.text, reason="quoted text with no reference"))
        else:
            item = _other(block, block.text)
            prior = doc.items[-1] if doc.items else None
            if (
                isinstance(item, ReviewItem) and isinstance(prior, PointItem) and prior.role in {"point", "list_item"}
                and block.line - prior.line <= 4 and prior.detail_end >= block.line - 1
            ):
                prior.text += "\n" + block.text
                prior.detail_end = block.line
                note = "joined unlabeled detail line(s) under this point"
                if note not in prior.warnings:
                    prior.warnings.append(note)
            else:
                doc.items.append(item)
    return doc


def _emit_verses(doc: NoteDocument, block: _Block, refs: list[PassageReference]) -> None:
    for ref in refs:
        if ref.warnings and any("whole chapter" in warning for warning in ref.warnings):
            doc.items.append(ReviewItem(
                line=block.line, text=ref.source_text,
                reason="whole-chapter reference; add verse numbers to include it",
            ))
            continue
        doc.items.append(VerseItem(line=block.line, reference=ref, warnings=list(ref.warnings)))


def _other(block: _Block, text: str):
    if re.match(r"^(part|week|session|series)\s*\d+\s*[:.-]", text, re.IGNORECASE) or re.match(r"^\d{1,2}/\d{1,2}/\d{2,4}\b", text) or text.endswith(":"):
        return SkippedItem(line=block.line, category="header", text=text, reason="title / heading line")
    if len(text) <= 80 and not text.endswith((".", "?", "!")):
        return SkippedItem(line=block.line, category="header", text=text, reason="unlabeled short line treated as a heading")
    return ReviewItem(line=block.line, text=text, reason="unlabeled line; not a reference, point, or known cue")
