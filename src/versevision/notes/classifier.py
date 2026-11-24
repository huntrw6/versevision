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

from versevision import runlog
from versevision.config import NotesConfig, PointsConfig
from versevision.references import parse_references
from versevision.references.models import ParseIssue, PassageReference
from versevision.versions.registry import is_real_version

# A numbered / lettered marker at the start of a line: "1)", "1.", "#1:", "a)", "2:".
_MARKER = re.compile(r"^\s*(?P<hash>#\s*)?(?P<label>\d{1,3}|[A-Za-z](?=[).]\s))\s*(?P<punct>[).:])\s*(?P<rest>.*)$")
_BULLET = re.compile(r"^\s*[•▪◦●‣*\-–—]\s+(?P<rest>\S.*)$")
_SLIDE_TAG = re.compile(r"^\s*\[\s*slide\s*\]\s*[-–—:]*\s*(?P<rest>.*)$", re.IGNORECASE)
_STAGE_WORDS = (r"show|play|put|start|cue|fade|transition|bring up|display|run|roll|when i (?:mention(?:ed)?|sa(?:y|id)|get|got|talk(?:ed)?|read|come|came)|"
                r"after (?:this|that)|at the end|end with|stop")
_STAGE = re.compile(r"^\s*(" + _STAGE_WORDS + r")\b", re.IGNORECASE)


class _Rules:
    """The options a run uses, compiled once."""
    def __init__(self, notes: NotesConfig, points: PointsConfig) -> None:
        self.notes, self.points = notes, points
        self.aliases: dict | None = None
        extra = "|".join(re.escape(w.strip()) for w in notes.extra_cue_words if w.strip())
        self.stage = re.compile(r"^\s*(" + _STAGE_WORDS + (("|" + extra) if extra else "") + r")\b", re.IGNORECASE)
        self.skip = [re.compile(x, re.IGNORECASE) for x in notes.skip_patterns]
        words = "|".join(re.escape(w.strip()) for w in notes.label_words if w.strip())
        self.labelled = re.compile(r"^\s*(" + words + r")\s*[:\-–—]\s*\S", re.IGNORECASE) if words else None
_NOTE = re.compile(r"^\s*(\(.*\)\s*$|\[(?!\s*slide)[^\]]*\]\s*$|(note|notes|todo|to do|reminder|remember|ask|tell|check|fix|tbd|fyi)\b\s*[:\-]?)", re.IGNORECASE)
_URL = re.compile(r"https?://|www\.", re.IGNORECASE)
_MEDIA_WORDS = re.compile(r"\b(slide|photo|picture|image|video|clip|screen|portrait|graphic|manuscript)\b", re.IGNORECASE)
_LABELLED = re.compile(
    r"^\s*(conclusion|application|invitation|challenge|summary|closing|takeaway|big idea|main point)\s*[:\-–—]\s*\S", re.IGNORECASE)
_RECAP = re.compile(r"^\s*recap\b", re.IGNORECASE)
_QUOTE_OPEN = ("“", '"', "‘", "'", "«")
_SENTENCE_END = (".", "?", "!", "”", '"', ":", ";")


class VerseItem(BaseModel):
    kind: Literal["verse"] = "verse"
    line: int
    reference: PassageReference
    warnings: list[str] = Field(default_factory=list)
    quoted: str = ""          # the verse text the notes quote next to this reference (used to tell which Bible version it is)


class PointItem(BaseModel):
    kind: Literal["point"] = "point"
    line: int
    role: Literal["point", "list_item", "slide_note"]
    label: str            # "1)", "#2:", "a)", or "•"
    text: str
    warnings: list[str] = Field(default_factory=list)
    detail_end: int = 0
    label_fixed_from: str = ""        # the label as written, when the importer corrected it to match the other points

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


def _marker(stripped: str, points: PointsConfig | None = None):
    """A numbered/lettered point marker, or None. Rejects '12: 9 NKJV'-style verse fragments."""
    match = _MARKER.match(stripped)
    if not match or not match.group("rest").strip():
        return None
    if points is not None and not (points.numbered if match.group("label").isdigit() else points.lettered):
        return None
    if match.group("punct") == ":" and match.group("rest").lstrip()[:1].isdigit():
        return None
    return match


_KEYED = re.compile(r"^\s*(key|point|step|principle|reason|way|tip|lesson|truth)\s*#?\s*\d+\s*[).:\-]", re.IGNORECASE)    # "KEY #3) ..." starts a new point
_KEYED_POINT = re.compile(r"^(?P<label>\s*(?P<word>key|point|step|principle|reason|way|tip|lesson|truth)\s*#?\s*(?P<n>\d{1,3})\s*[).:\-])\s*(?P<rest>\S.*)$", re.IGNORECASE)


def _is_caps(text: str) -> bool:
    letters = [c for c in text if c.isalpha()]
    return len(letters) >= 3 and all(c.isupper() for c in letters)


_NARROW = set("iltfjI.,;:'’!|() ")


def _width(text: str) -> int:
    """How wide a line looks on the page, in plain-letter units: capitals are wider and i, l, t, punctuation and spaces are narrower.
    A line of capitals fills the page with far fewer characters, so measuring characters misses wrapped headings."""
    return round(sum(1.3 if c.isupper() else 0.6 if c in _NARROW else 1.0 for c in text.rstrip()))


def _wrap_geometry(lines: list[str]) -> tuple[bool, int]:
    """Detect hard-wrapped text (PDF/email) and the width at which a line counts as full."""
    lengths = sorted(_width(line) for line in lines if line.strip())
    if len(lengths) < 8:
        return False, 0
    p95 = lengths[max(0, int(len(lengths) * 0.95) - 1)]
    near = sum(1 for n in lengths if 0.86 * p95 <= n <= 1.05 * p95)
    return (50 <= p95 <= 140 and near / len(lengths) >= 0.15), int(0.86 * p95)


def _wrap_reason(current: "_Para", stripped: str, threshold: int, notes: NotesConfig, points: PointsConfig, is_reference: bool = False) -> str | None:
    """Why this line is the rest of the line above (the page wrapped it mid-sentence), or None when it is a new line of its own.

    1. A sentence that has already ended (. ! ?) is never continued, however long the line was: the next line is something new
       (unless the sentence sits inside a quotation that is still open, or the next line starts in lower case).
    2. Otherwise it is the rest of the line above when that line is full width, or when it is a tail of a few words and the line above
       is at least over half full."""
    above = current.text.rstrip()
    ended = above.rstrip("”\"’'").endswith((".", "?", "!"))
    in_open_quote = above.startswith(_QUOTE_OPEN) and not above.endswith(_CLOSERS)
    lower = stripped[:1].islower()
    if _marker(above, points) and above.endswith(_SENTENCE_END[:5]) and stripped[:1].isupper():
        return None                      # a numbered point that reads as complete: the next line is something new
    if ended and not in_open_quote and not lower and not is_reference:
        return None          # (a reference on the next line belongs to the verse text above it, so that case still joins)
    words = len(stripped.split())
    wide = current.last_len
    if _is_caps(above) != _is_caps(stripped) and words > notes.orphan_max_words:
        return None                      # a heading in capitals is not continued by a sentence in lower case, or the other way round
    if is_reference and wide < threshold:
        return None                      # a reference line is never a "tail": it only joins the verse text above it when that line was full width
    if wide >= threshold:
        return f"the line above looks full width ({wide} of {threshold}) and its sentence has not ended, so this line is the rest of it"
    if wide >= 0.55 * threshold and words <= notes.orphan_max_words:
        return f"this is a {words}-word tail and the line above (width {wide} of {threshold}) does not end its sentence, so it finishes that sentence"
    if wide >= 0.55 * threshold and lower:
        return "this line starts in lower case, so it carries on the sentence above"
    return None


def classify_document(
    text: str, aliases: dict[str, str] | None = None, notes: NotesConfig | None = None, points: PointsConfig | None = None,
    *, line_mode: bool | None = None, unlabeled_bullets: bool | None = None,
) -> NoteDocument:
    """Classify a document into verse / point / skipped / review items, in document order.

    Every choice (what a line break means, what to do with plain lines, cues, quotes...) comes from the options; the defaults are the
    house style. line_mode / unlabeled_bullets are older shortcuts: line_mode=False is line_breaks="paragraph", unlabeled_bullets=False
    sends plain lines to review."""
    notes = (notes or NotesConfig()).model_copy()
    points = points or PointsConfig()
    if line_mode is False:
        notes.line_breaks = "paragraph"
    if unlabeled_bullets is False:
        notes.unlabeled_sentence = notes.unlabeled_short = "review"
    rules = _Rules(notes, points)
    lines = text.replace("\r\n", "\n").replace("\r", "\n").replace("\xa0", " ").split("\n")
    hard_wrap, threshold = _wrap_geometry(lines)
    runlog.trace("parse_start", lines=len(lines), non_blank=sum(1 for x in lines if x.strip()), hard_wrapped=hard_wrap,
                 full_line_width=threshold, wrapped_lines=notes.wrapped_lines, orphan_max_words=notes.orphan_max_words,
                 note="hard_wrapped means the text looks like a PDF/email with lines broken mid-sentence. Widths are measured as they look on the page "
                 "(capitals are wider). A line is joined to the one above when that line has not ended its sentence and is full width, "
                 "or when it is a short tail (orphan_max_words or fewer) after a line that is over half full")

    def starts_new(stripped: str, current: _Para) -> bool:
        if _SLIDE_TAG.match(stripped) or rules.stage.match(stripped) or _URL.search(stripped) or _RECAP.match(stripped) or any(x.search(stripped) for x in rules.skip):
            return True
        if _KEYED.match(stripped):
            return True
        if stripped.startswith(_QUOTE_OPEN) and not (current.text.startswith(_QUOTE_OPEN) and not current.text.rstrip().endswith(_CLOSERS)):
            return True          # a line that opens a quotation is a new item, unless the quotation above is still open
        if _NOTE.match(stripped):
            inside_quote = current.text.startswith(_QUOTE_OPEN) and not current.text.rstrip().endswith(_CLOSERS)
            if inside_quote and not re.match(r"^\s*\w+\s*[:\-]", stripped) and not stripped.startswith(("(", "[")):
                return False          # a wrapped line of a quote ("remember me when You come into Your kingdom")
            return True
        if points.bullets and _BULLET.match(stripped):
            return True
        if _marker(stripped, points) and not _is_reference_start(stripped, aliases):
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
        why = None
        if current is not None and hard_wrap and notes.wrapped_lines == "join" and not starts_new(stripped, current):
            why = _wrap_reason(current, stripped, threshold, notes, points, _is_reference_start(stripped, aliases))
        if why:
            runlog.trace("line_join", line=number, into_paragraph=current.line, text=stripped, why=why)
            current.append(stripped, _width(line), number)
            continue
        current = _Para(number, stripped, _width(line))
        paras.append(current)

    if paras:
        paras[0].first = True
    for para in paras:
        _label(para, aliases, rules)
        runlog.trace("paragraph", line=para.line, end_line=para.end,
                     raw_lines=[lines[i].rstrip() for i in range(para.line - 1, min(para.end, len(lines))) if lines[i].strip()],
                     text=para.text, first_in_document=para.first, label=para.kind, rule=para.extra.get("rule", ""),
                     why=para.extra.get("why", ""), marker=para.extra.get("label"),
                     references=[{"as_written": r.source_text.strip(), "normalized": r.normalized_text, "version_written": r.version,
                                  "warnings": r.warnings} for r in para.refs])
    return _build(paras, aliases, rules, duplicates=notes.duplicates)


def _label(para: _Para, aliases, rules: _Rules) -> None:
    text = para.text
    points = rules.points
    tag = _SLIDE_TAG.match(text)
    if tag:
        para.kind, para.text = "slide_tag", tag.group("rest").strip()
        para.extra["rule"] = "[slide] tag"
        return
    paren = _paren_reference(text, aliases)
    if paren:
        para.kind, para.refs = "verse", paren
        para.extra["rule"] = "whole line is a reference in brackets"
        return
    skip_hit = next((x.pattern for x in rules.skip if x.search(text)), None)
    if skip_hit:
        para.kind, para.extra["why"] = "stage", "matches a skip pattern"
        para.extra["rule"] = f"matches your skip pattern {skip_hit!r}"
        return
    if rules.stage.match(text) or _URL.search(text) or _NOTE.match(text):
        para.kind = "stage"
        para.extra["rule"] = ("starts with a stage/cue word" if rules.stage.match(text) else "contains a web address" if _URL.search(text)
                              else "reads like a note to self (brackets, 'note:', 'remember'...)")
        return
    bullet = _BULLET.match(text) if points.bullets else None
    if bullet and not _is_reference_start(bullet.group("rest"), aliases):
        para.kind, para.text = "marker", bullet.group("rest").strip()
        para.extra.update(label="•", number=None, bullet=True, rule="bullet character at the start of the line")
        para.refs = parse_references(para.text, aliases).references
        return
    if _RECAP.match(text):
        para.kind, para.extra["why"] = "header", "recap heading"
        para.extra["rule"] = "line starts with 'Recap'"
        return
    if _DATE.search(text) and not re.search(r"\d:\d", text):
        para.kind, para.extra["why"] = "header", "title / date line"
        para.extra["rule"] = "contains a date (d/m/y) and no verse-like d:d"
        return
    keyed = _KEYED_POINT.match(text) if points.numbered else None
    if keyed:
        para.kind, para.text = "marker", keyed.group("rest").strip()
        para.extra.update(label=" ".join(keyed.group("label").split()), number=int(keyed.group("n")), rule=f"'{keyed.group('word').upper()} #n)' is the point's own header")
        para.refs = parse_references(para.text, aliases).references
        return
    marker = _marker(text, points)
    if marker:
        inner, _ = _lead_references(marker.group("rest").strip(), aliases)
        if inner and not _is_reference_start(text, aliases):
            para.kind, para.refs = "verse", inner
            para.extra["numbered_verse"] = True
            para.extra["rule"] = "numbered line whose text starts with a Bible reference"
            return
    if marker and not _is_reference_start(text, aliases):
        label = marker.group("label")
        display = f"#{label}{marker.group('punct')}" if marker.group("hash") else f"{label}{marker.group('punct')}"
        para.kind = "marker"
        para.text = marker.group("rest").strip()
        para.extra.update(label=display, number=int(label) if label.isdigit() else None, rule="numbered/lettered marker at the start of the line")
        para.refs = parse_references(para.text, aliases).references
        return
    leads, _ = _lead_references(text, aliases)
    if leads:
        para.kind, para.refs = "verse", leads
        para.extra["rule"] = "Bible reference at the start of the line"
        return
    trailing = _trailing_references(text, aliases)
    if trailing:
        para.kind, para.refs = "verse", trailing
        para.extra["rule"] = "Bible reference at the end of a quoted block"
        return
    found = parse_references(text, aliases)
    if found.references:
        para.kind, para.refs = "midref", found.references
        para.extra["rule"] = "Bible reference found inside a sentence"
        return
    leading = next((i for i in found.issues if i.source_start <= 2), None)
    if leading is not None:                    # it reads like a verse at the start of the line, but it is not a real one
        para.kind, para.extra["why"] = "badref", f"{leading.source_text.strip()}: {leading.message}"
        para.extra["rule"] = "looks like a reference at the start of the line, but is not a real verse"
        return
    para.kind = "quote" if text.startswith(_QUOTE_OPEN) else "other"
    para.extra["rule"] = "starts with a quotation mark" if para.kind == "quote" else "no reference, marker or cue word: a plain line"


def _paren_reference(text: str, aliases) -> list[PassageReference]:
    """'(Romans 8:16)' or '(Romans 8:16 NKJV)': a whole line that is just a reference in brackets."""
    match = re.match(r"^\s*[(\[]\s*(?P<inner>[^()\[\]]+?)\s*[)\]]\s*$", text)
    if not match:
        return []
    inner = match.group("inner")
    parsed = parse_references(inner, aliases)
    if len(parsed.references) != 1 or parsed.issues:
        return []
    ref = parsed.references[0]
    return [ref] if ref.source_start <= 1 and len(inner) - ref.source_end <= 1 else []


def _is_reference_start(text: str, aliases) -> bool:
    leads, _ = _lead_references(text, aliases)
    return bool(leads)


def _build(paras: list[_Para], aliases, rules: _Rules, duplicates: str = "keep") -> NoteDocument:
    notes, points = rules.notes, rules.points
    rules.aliases = aliases
    doc = NoteDocument()
    seen_refs: set[str] = set()
    last_point = 0
    seen_content = False
    for index, para in enumerate(paras):
        _items_before = len(doc.items)
        _prior_text = doc.items[-1].text if doc.items and isinstance(doc.items[-1], PointItem) else None
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
            why = para.extra.get("why", "stage/production cue")
            if notes.stage_cues == "review":
                doc.items.append(ReviewItem(line=line, text=text, reason=f"{why}: your setting asks to check these"))
            elif notes.stage_cues == "bullet":
                doc.items.append(PointItem(line=line, role="slide_note", label="•", text=text, detail_end=line, warnings=[f"{why} made a slide because of your setting"]))
            else:
                doc.items.append(SkippedItem(line=line, category="stage_note", text=text, reason=why))
        elif kind == "header":
            if para.extra.get("why") == "recap heading" and notes.headings == "slide":
                doc.items.append(_heading(para, text))
            else:
                doc.items.append(SkippedItem(line=line, category="header", text=text, reason=para.extra.get("why", "heading")))
        elif kind == "marker":
            number, label = para.extra["number"], para.extra["label"]
            if number is None:
                role = "list_item"            # bullets and lettered items
            elif number == 1 or number == last_point + 1 or not points.require_sequence:
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
            before = len(doc.items)
            _emit_verses(doc, para, para.refs, paras[index - 1] if index else None)
            if duplicates == "drop":
                for item in list(doc.items[before:]):
                    key = item.reference.normalized_text.casefold() if isinstance(item, VerseItem) else ""
                    if key and key in seen_refs:
                        doc.items.remove(item)
                        doc.items.append(SkippedItem(line=item.line, category="stage_note", text=item.reference.normalized_text, reason="same verse already added above (your duplicates setting)"))
                    elif key:
                        seen_refs.add(key)
            if para.extra.get("numbered_verse"):
                doc.items[-1].warnings.append("numbered line holds a reference; treated as a verse, not a sermon point")
        elif kind == "badref":
            doc.items.append(ReviewItem(line=line, text=text, reason=f"looks like a verse but is not one ({para.extra['why']})"))
        elif kind == "midref":
            names = ", ".join(ref.normalized_text for ref in para.refs[:3])
            if notes.reference_in_text == "skip":
                doc.items.append(SkippedItem(line=line, category="stage_note", text=text, reason=f"mentions {names} inside text (your setting skips these)"))
            elif notes.reference_in_text == "verse":
                for ref in para.refs:
                    doc.items.append(VerseItem(line=line, reference=ref, warnings=[f"taken from the sentence: {text[:50]!r}"]))
            else:
                doc.items.append(ReviewItem(
                    line=line, text=text,
                    reason=f"mentions {names} inside text; put the reference on its own line to add it as a slide",
                ))
        elif kind == "quote":
            nxt = paras[index + 1] if index + 1 < len(paras) else None
            if nxt is not None and nxt.kind == "verse" and nxt.line - para.end <= 3 and notes.quote_above_reference == "skip":
                doc.items.append(SkippedItem(line=line, category="quoted_text", text=text,
                                             reason="verse text quoted above its reference; text is fetched from the Bible provider"))
            elif nxt is not None and nxt.kind == "verse" and nxt.line - para.end <= 3:
                doc.items.append(ReviewItem(line=line, text=text, reason="verse text quoted above its reference (your setting asks to check these)"))
            elif notes.quote_alone == "skip":
                doc.items.append(SkippedItem(line=line, category="quoted_text", text=text, reason="quoted text with no reference (your setting skips these)"))
            elif notes.quote_alone == "bullet":
                doc.items.append(PointItem(line=line, role="list_item", label="•", text=text, detail_end=line, warnings=["quoted text with no reference made a bullet slide because of your setting"]))
            else:
                doc.items.append(ReviewItem(line=line, text=text, reason="quoted text with no reference"))
        elif rules.labelled is not None and rules.labelled.match(text) and not para.first and notes.label_own_slide:
            # "Conclusion: Will You Let God Use Your Life?" is a different sentence from the point above it: its own slide
            doc.items.append(PointItem(line=line, role="slide_note", label="", text=text, detail_end=line))
        else:
            prior = doc.items[-1] if doc.items else None
            joined = False
            if (
                notes.line_breaks == "paragraph" and isinstance(prior, PointItem) and prior.role in {"point", "list_item"}
                and line - prior.detail_end <= 2 and text.rstrip().endswith((".", "?", "!", "”", '"'))
                and isinstance(_other(para, text, rules), (ReviewItem, PointItem)) and para.extra.get("seen_content")
            ):
                prior.text += "\n" + text
                prior.detail_end = line
                note = "joined unlabeled detail line(s) under this point"
                if note not in prior.warnings:
                    prior.warnings.append(note)
                joined = True
            if not joined:
                doc.items.append(_other(para, text, rules))
        _trace_outcome(doc, para, _items_before, _prior_text, rules)
    return doc


_SETTINGS_BY_LABEL = {
    "stage": ("stage_cues",), "midref": ("reference_in_text",), "quote": ("quote_above_reference", "quote_alone"),
    "other": ("line_breaks", "unlabeled_sentence", "unlabeled_short", "before_first_item", "header_max_words", "headings", "caps_lines", "wrapped_lines", "orphan_max_words"),
    "verse": ("duplicates",), "marker": (), "header": (),
}


def _trace_outcome(doc: NoteDocument, para: _Para, before: int, prior_text, rules: _Rules) -> None:
    """What this paragraph turned into, and which of your options decided it."""
    made = doc.items[before:]
    settings = {k: getattr(rules.notes, k) for k in _SETTINGS_BY_LABEL.get(para.kind, ())}
    if para.kind == "marker":
        settings = {"points.numbering": rules.points.numbering, "points.require_sequence": rules.points.require_sequence}
    outcomes = []
    for item in made:
        row: dict = {"kind": item.kind, "line": item.line}
        if isinstance(item, PointItem):
            row.update(role=item.role, label=item.label, text=item.text, warnings=item.warnings)
        elif isinstance(item, VerseItem):
            row.update(reference=item.reference.normalized_text, version_written=item.reference.version, warnings=item.warnings)
        elif isinstance(item, SkippedItem):
            row.update(category=item.category, reason=item.reason, text=item.text)
        elif isinstance(item, ReviewItem):
            row.update(reason=item.reason, text=item.text)
        outcomes.append(row)
    merged = not made and prior_text is not None
    if merged:
        last = doc.items[-1]
        outcomes.append({"kind": "joined", "into_line": last.line, "note": "added to the sermon point above as a detail line (line_breaks=paragraph)"})
    runlog.trace("item", line=para.line, label=para.kind, settings=settings, outcome=outcomes)


def _quote_of(para: _Para, prev: "_Para | None", ref: PassageReference) -> str:
    """The verse text the notes quote with this reference: the quotation before it on the same line ("…" (John 3:3)), or the quotation
    paragraph just above it."""
    text = para.text
    at = text.rfind(ref.source_text.strip()) if ref.source_text.strip() else -1
    if at > 0:
        before = text[:at].strip().rstrip("(—–-,; ").strip()
        if len(before.split()) >= 5:
            return before
    if prev is not None and prev.kind == "quote" and para.line - prev.end <= 3 and len(prev.text.split()) >= 5:
        return prev.text
    return ""


def _emit_verses(doc: NoteDocument, para: _Para, refs: list[PassageReference], prev: "_Para | None" = None) -> None:
    for ref in refs:
        if any("whole chapter" in warning for warning in ref.warnings):
            doc.items.append(ReviewItem(
                line=para.line, text=ref.source_text,
                reason="whole-chapter reference; add verse numbers to include it",
            ))
            continue
        doc.items.append(VerseItem(line=para.line, reference=ref, warnings=list(ref.warnings), quoted=_quote_of(para, prev, ref)))


def _is_book_name(text: str, aliases) -> bool:
    found = parse_references(f"{text} 1:1", aliases or {}).references
    return bool(found) and found[0].source_start <= 1


def _heading(para: _Para, text: str) -> "PointItem":
    """A title or heading line as its own slide, with no bullet (it is not a point, just words to have ready)."""
    return PointItem(line=para.line, role="slide_note", label="", text=text, detail_end=para.end)


def _other(para: _Para, text: str, rules: _Rules):
    """A plain line: heading, preamble, or one of the three ways the options say to treat unlabeled text."""
    notes = rules.notes
    line = para.line
    ends = text.endswith((".", "?", "!", "”", '"'))
    # a part of a reference ("2 Corinthians", "12:9 NKJV") is never a title
    as_heading = notes.headings == "slide" and not re.search(r"\d+\s*:\s*\d+", text) and not _is_book_name(text, rules.aliases)
    if re.match(r"^(part|week|session|series)\s*\d+\s*[:.-]", text, re.IGNORECASE) or text.endswith(":"):
        return _heading(para, text) if as_heading else SkippedItem(line=line, category="header", text=text, reason="title / heading line")
    words = text.split()
    if _EMAIL_HEADER.search(text):
        return SkippedItem(line=line, category="header", text=text, reason="email header line")
    if para.first and not ends and notes.before_first_item == "skip":
        return _heading(para, text) if as_heading else SkippedItem(line=line, category="header", text=text, reason="document title (first line)")
    if len(words) <= notes.header_max_words and _header_like(words) and not ends:
        if as_heading:
            return _heading(para, text)
        return SkippedItem(line=line, category="header", text=text, reason=f"{notes.header_max_words} or fewer capitalized words: looks like a heading")

    def handle(mode: str, review_reason: str, bullet_warning: list[str]):
        if mode == "bullet":
            return PointItem(line=line, role="list_item", label="•", text=text, detail_end=para.end, warnings=bullet_warning)
        if mode == "skip":
            return SkippedItem(line=line, category="header", text=text, reason="unlabeled line (your setting skips these)")
        return ReviewItem(line=line, text=text, reason=review_reason)

    if not para.extra.get("seen_content"):
        if len(text) <= 80 and not ends:
            if notes.before_first_item == "skip":
                return _heading(para, text) if as_heading else SkippedItem(line=line, category="header", text=text, reason="title/preamble before the first verse or point")
            return handle("bullet" if notes.before_first_item == "bullet" else "review", "line before the first verse or point (your setting asks to check these)", ["line before the first verse or point made a slide because of your setting"])
        if notes.before_first_item == "skip" and len(text) <= 80:
            return SkippedItem(line=line, category="header", text=text, reason="title/preamble before the first verse or point")
        return handle("review", "unlabeled line; not a reference, point, or known cue", []) if notes.before_first_item != "bullet" else handle("bullet", "", ["line before the first verse or point made a slide because of your setting"])
    if as_heading and notes.caps_lines == "heading" and not ends and _is_caps(text) and len(words) >= 2:
        return _heading(para, text)          # ALL CAPITALS, no full stop: a heading, so no bullet
    if len(text) <= 80 and not ends:
        return handle(notes.unlabeled_short, "short unlabeled line: a heading, or a point missing its number? add a number or bullet to make it a slide",
                      ["short line with no ending punctuation made a bullet point; it may be a heading instead"])
    return handle(notes.unlabeled_sentence, "unlabeled line; not a reference, point, or known cue", [])    # a full sentence: the line-per-slide rule, no guess
