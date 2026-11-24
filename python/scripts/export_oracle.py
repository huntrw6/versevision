#!/usr/bin/env python3
"""Export the VerseVision *oracle*: what the Python app does with a set of documents, written as plain files.

The Rust port (see docs/PLAN_CONTRACT.md and oracle/README.md) must reproduce every file below field by field.

    cd python && uv run python scripts/export_oracle.py            # rewrites ../oracle
    cd python && uv run python scripts/export_oracle.py --out DIR  # writes somewhere else (to compare two runs)

Written (all deterministic: sorted keys, no timestamps, run ids, uuids or absolute paths):

    oracle/cases/<name>/input.<ext>          the document (txt, md, html, docx or pdf)
    oracle/cases/<name>/options.toml         only what differs from the defaults ([run] holds the run parameters)
    oracle/cases/<name>/bible.json           the verse text the case needs, "<VERSION> <Book> <ch>:<v>" -> text (offline run)
    oracle/cases/<name>/expected/plan.json   the plan: slides, decisions, notices ...
    oracle/cases/<name>/expected/trace.jsonl the run trace (one JSON object per line)
    oracle/cases/<name>/expected/review.json the review report the web page is built from
    oracle/cases/<name>/expected/extracted.txt   the text the document reader returned (what the classifier receives)
    oracle/cases/<name>/expected/document.json   what the notes classifier made of it (items, issues, the rule of each paragraph)
    oracle/references.jsonl, fitting.jsonl, versions.jsonl   line-level oracles
    oracle/document_errors.jsonl                              documents (and options) for which the classifier raises, with the exception and message
    oracle/user_patterns.jsonl, user_patterns_subjects.json   Python `re` as a user writes it in notes.skip_patterns: search results and compile errors
    oracle/config_defaults.json, config_defaults.toml, config_rich.toml   the default config and a config using every kind of value (what `save_config` writes)
    oracle/config_options.jsonl                               `with_options` (profile + `--set` strings): the changed fields or the exact error

Only the standard library and the app itself are used. Nothing here changes how the app behaves.
"""
from __future__ import annotations

import argparse
import ast
import asyncio
import copy
import hashlib
import io
import json
import shutil
import sys
import tempfile
import tomllib
import zipfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import tomli_w

PY_ROOT = Path(__file__).resolve().parents[1]          # python/
REPO_ROOT = PY_ROOT.parent
TESTS = PY_ROOT / "tests"
sys.path.insert(0, str(PY_ROOT / "src"))

from versevision import runlog                                         # noqa: E402
from versevision.bibles.models import BibleVerse, ResolvedPassage       # noqa: E402
from versevision.config import AppConfig, with_options                   # noqa: E402
from versevision.errors import ConfigurationError                        # noqa: E402
from versevision.documents import extract_text                          # noqa: E402
from versevision.errors import BibleProviderError                       # noqa: E402
from versevision.notes import classify_document                          # noqa: E402
from versevision.references import parse_references                     # noqa: E402
from versevision.references.versification import VERSE_COUNTS           # noqa: E402
from versevision.slides import fitting                                  # noqa: E402
from versevision.versions.registry import canonical_version, correct_version, is_real_version  # noqa: E402
from versevision.workflow.notes_workflow import apply_review_choices, plan_notes, quote_score    # noqa: E402
from versevision.workflow.report import build_report                   # noqa: E402

DEFAULT_RUN = {"default_version": "NKJV", "no_fetch": False}


# ------------------------------------------------------------------------------------------------------ small helpers
def dumps(obj: Any, *, indent: int | None = 2) -> str:
    return json.dumps(obj, sort_keys=True, ensure_ascii=False, indent=indent, separators=None if indent else (",", ":")) + "\n"


def write_text(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(text.encode("utf-8"))               # LF only, whatever the platform


def sort_tree(value: Any) -> Any:
    if isinstance(value, dict):
        return {k: sort_tree(value[k]) for k in sorted(value)}
    if isinstance(value, list):
        return [sort_tree(v) for v in value]
    return value


# ------------------------------------------------------------------------------------------------- synthetic verse text
_WORDS = ("grace faith mercy kingdom light truth spirit servant shepherd covenant righteous promise glory wisdom peace "
          "heaven temple mountain river bread vine lamp seed harvest judge father brother stranger prayer song "
          "tender steadfast humble faithful ancient living gentle mighty hidden open ready").split()
_SEPARATORS = (" ", " ", " ", " ", ", ", " ", "; ", " ", ": ", " ", " — ", ". ")


def _stream(seed: str):
    counter = 0
    while True:
        for byte in hashlib.sha256(f"{seed}#{counter}".encode()).digest():
            yield byte
        counter += 1


def synthetic_verse(version: str, book: str, chapter: int, verse: int) -> str:
    """A made-up sentence of varying length for one verse (not Scripture): the same key always gives the same text."""
    rng = _stream(f"{version} {book} {chapter}:{verse}")
    length = 4 + next(rng) % 28 if next(rng) % 4 else 28 + next(rng) % 40        # mostly short, one in four long enough to be cut
    words: list[str] = []
    for i in range(length):
        word = _WORDS[next(rng) % len(_WORDS)]
        if i == 0:
            word = word.capitalize()
        words.append(word + (_SEPARATORS[next(rng) % len(_SEPARATORS)].rstrip() if i < length - 1 else ""))
    text = " ".join(words)
    if next(rng) % 7 == 0:
        text = "“" + text + "”"
    return text.rstrip(",;:—") + ("." if not text.rstrip().endswith((".", "”")) else "")


# --------------------------------------------------------------------------------------------- the offline Bible provider
def expand_span(book_id: str, span) -> list[tuple[int, int]]:
    start, end = span.start, span.end or span.start
    if end.chapter == start.chapter:
        return [(start.chapter, v) for v in range(start.verse, end.verse + 1)]
    counts = VERSE_COUNTS[book_id]
    out = [(start.chapter, v) for v in range(start.verse, counts[start.chapter - 1] + 1)]
    for chapter in range(start.chapter + 1, end.chapter):
        out += [(chapter, v) for v in range(1, counts[chapter - 1] + 1)]
    return out + [(end.chapter, v) for v in range(1, end.verse + 1)]


def verse_keys_of(reference) -> list[tuple[str, int, int]]:
    out: list[tuple[str, int, int]] = []
    for span in reference.spans:
        out += [(f"{reference.version} {reference.book_display_name} {c}:{v}", c, v) for c, v in expand_span(reference.book_id, span)]
    return out


class OracleBible:
    """Serves bible.json. The same provider contract is described in oracle/README.md."""

    def __init__(self, table: dict[str, str]):
        self.table = table

    async def fetch(self, reference):
        info = {"reference": reference.normalized_text, "version": reference.version}
        try:
            verses = []
            for key, chapter, number in verse_keys_of(reference):
                if key not in self.table:
                    raise BibleProviderError(f"no verse text for {key}")
                verses.append(BibleVerse(book_id=reference.book_id, chapter=chapter, verse=number, text=self.table[key], version=reference.version))
        except Exception as exc:
            runlog.trace("provider", error=str(exc), ms=0, **info)
            raise
        runlog.trace("provider", provider="oracle", source="bible.json", ms=0, **info)
        return ResolvedPassage(reference=reference, resolved_version=reference.version, verses=verses)

    async def aclose(self) -> None:
        return None


class Recorder:
    """First pass of a case: asks a text source for every verse the run needs and remembers them (becomes bible.json)."""

    def __init__(self, overrides: dict[str, str], missing_versions: set[str], source=None):
        self.overrides, self.missing, self.source = overrides, {v.casefold() for v in missing_versions}, source
        self.table: dict[str, str] = {}

    async def fetch(self, reference):
        if (reference.version or "").casefold() in self.missing:
            raise BibleProviderError(f"no verse text for {reference.version} {reference.book_display_name}")
        if self.source is not None:
            passage = await self.source.fetch(reference)
            for v in passage.verses:
                self.table[f"{reference.version} {reference.book_display_name} {v.chapter}:{v.verse}"] = v.text
            return passage
        verses = []
        for key, chapter, number in verse_keys_of(reference):
            text = self.overrides.get(key) or synthetic_verse(reference.version, reference.book_display_name, chapter, number)
            self.table[key] = text
            verses.append(BibleVerse(book_id=reference.book_id, chapter=chapter, verse=number, text=text, version=reference.version))
        return ResolvedPassage(reference=reference, resolved_version=reference.version, verses=verses)

    async def aclose(self) -> None:
        return None


# ------------------------------------------------------------------------------------------------ documents in each format
def build_html(text: str) -> bytes:
    from html import escape
    body = "\n".join(f"<p>{escape(line)}</p>" for line in text.split("\n"))
    return f"<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\"><title>notes</title></head>\n<body>\n{body}\n</body></html>\n".encode("utf-8")


def build_docx(text: str) -> bytes:
    """One Word paragraph per line; the zip and the properties are fixed so the bytes do not change between runs."""
    import datetime as dt
    from docx import Document
    document = Document()
    for line in text.split("\n"):
        document.add_paragraph(line)
    props = document.core_properties
    fixed = dt.datetime(2026, 1, 1, 0, 0, 0)
    props.author, props.last_modified_by, props.title, props.comments = "oracle", "oracle", "oracle", ""
    props.created, props.modified, props.last_printed, props.revision = fixed, fixed, fixed, 1
    buffer = io.BytesIO()
    document.save(buffer)
    out = io.BytesIO()
    with zipfile.ZipFile(io.BytesIO(buffer.getvalue())) as source, zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as target:
        for name in sorted(source.namelist()):
            info = zipfile.ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
            info.compress_type, info.external_attr = zipfile.ZIP_DEFLATED, 0o600 << 16
            target.writestr(info, source.read(name))
    return out.getvalue()


def build_pdf(lines: list[str]) -> bytes:
    """A small one-font PDF with a text layer, one drawn line per input line (ASCII only)."""
    def esc(s: str) -> str:
        return s.replace("\\", "\\\\").replace("(", "\\(").replace(")", "\\)")
    per_page = 48
    pages = [lines[i:i + per_page] for i in range(0, len(lines), per_page)] or [[]]
    objects: list[bytes] = []
    kids = " ".join(f"{3 + 2 * i} 0 R" for i in range(len(pages)))
    objects.append(b"<< /Type /Catalog /Pages 2 0 R >>")
    objects.append(f"<< /Type /Pages /Kids [{kids}] /Count {len(pages)} >>".encode())
    font_id = 3 + 2 * len(pages)
    for i, page in enumerate(pages):
        content_id = 4 + 2 * i
        objects.append(f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {content_id} 0 R /Resources << /Font << /F1 {font_id} 0 R >> >> >>".encode())
        stream = "BT /F1 11 Tf 14 TL 40 750 Td\n" + "".join(f"({esc(line)}) Tj T*\n" for line in page) + "ET"
        objects.append(f"<< /Length {len(stream)} >>\nstream\n{stream}\nendstream".encode())
    objects.append(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>")
    out = bytearray(b"%PDF-1.4\n")
    offsets = []
    for number, body in enumerate(objects, 1):
        offsets.append(len(out))
        out += f"{number} 0 obj\n".encode() + body + b"\nendobj\n"
    xref = len(out)
    out += f"xref\n0 {len(objects) + 1}\n0000000000 65535 f \n".encode()
    out += b"".join(f"{o:010d} 00000 n \n".encode() for o in offsets)
    out += f"trailer\n<< /Size {len(objects) + 1} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode()
    return bytes(out)


# ------------------------------------------------------------------------------------------------------------- the cases
@dataclass
class Case:
    name: str
    data: bytes
    ext: str = "txt"
    options: dict = field(default_factory=dict)             # config overrides, as nested tables
    run: dict = field(default_factory=dict)                 # default_version / no_fetch when they differ from DEFAULT_RUN
    overrides: dict = field(default_factory=dict)           # explicit verse texts ("<VERSION> <Book> <ch>:<v>" -> text)
    missing_versions: tuple = ()                            # versions the offline provider does not have
    synthetic_usfm: bool = False                            # take verse text from tests/fixtures/usfm/synthetic
    choices: dict = field(default_factory=dict)             # review choices applied afterwards: skip / include / reject_fixes (decision ids)
    group: str = ""


def text_case(name: str, text: str, group: str, **kw) -> Case:
    return Case(name, text.encode("utf-8"), "txt", group=group, **kw)


WHITEFIELD = (TESTS / "fixtures" / "whitefield_notes.txt").read_text(encoding="utf-8")

WHITEFIELD_OPTION_SETS = {
    "defaults": {},
    "max-lines-per-slide-2": {"points": {"max_lines_per_slide": 2}},
    "max-body-lines-4": {"slides": {"max_body_lines": 4}},
    "line-breaks-paragraph": {"notes": {"line_breaks": "paragraph"}},
    "keep-trailing-period": {"points": {"strip_trailing_period": False}},
    "no-label-fix": {"points": {"fix_label_typos": False}},
}

# One document with a bit of everything the classifier knows about; the option cases below change one thing each.
KITCHEN = """JPL 11/22/26 A Study in the Psalms
Show Photo of the shepherd
THREE THINGS TO KNOW
“The Lord is my shepherd; I shall not want.”
Psalm 23:1 NKJV
1) The Lord leads us beside still waters.
A full sentence that sits under the first point and keeps going for a good while so that it wraps.
Short heading
2) He restores my soul and guides me in the paths of righteousness for His name's sake.
Conclusion: trust Him.
Romans 8:28
John 3:16-18 NIV
Psalm 23
As Philippians 4:13 says, we can do all things.
Philippians 4:19
John 3:16-18 NIV
a) Pray
b) Read
• A bullet line
- another bullet
(pause here)
https://example.com/slides
3) KEY#3) mismatched label
Matthew 6:9-13 KJV
Acts 2:1-4 & 6-8
Genesis 1:1 – 2:3
Closing thought for the evening.
"""

OPTION_VARIANTS: dict[str, dict] = {
    "numbering-strip": {"points": {"numbering": "strip"}},
    "numbering-renumber": {"points": {"numbering": "renumber"}},
    "number-style": {"points": {"numbering": "renumber", "number_style": "#{n}:"}},
    "bullet-char-dash": {"points": {"bullet_char": "-"}},
    "bullet-char-empty": {"points": {"bullet_char": ""}},
    "case-upper": {"points": {"case": "upper"}},
    "case-lower": {"points": {"case": "lower"}},
    "case-title": {"points": {"case": "title"}},
    "points-no-balance": {"points": {"balance_lines": False}},
    "points-narrow": {"points": {"line_width_units": 20.0}},
    "points-max-lines-1": {"points": {"max_lines_per_slide": 1}},
    "points-no-bullets": {"points": {"bullets": False, "lettered": False}},
    "points-no-sequence-rule": {"points": {"require_sequence": False}},
    "notes-unlabeled-review": {"notes": {"unlabeled_sentence": "review", "unlabeled_short": "review"}},
    "notes-unlabeled-skip": {"notes": {"unlabeled_sentence": "skip", "unlabeled_short": "skip"}},
    "notes-stage-cues-bullet": {"notes": {"stage_cues": "bullet"}},
    "notes-stage-cues-review": {"notes": {"stage_cues": "review", "extra_cue_words": ["pause"]}},
    "notes-skip-patterns": {"notes": {"skip_patterns": ["^Closing"]}},
    "notes-headings-skip": {"notes": {"headings": "skip", "caps_lines": "line"}},
    "notes-before-first-item-bullet": {"notes": {"before_first_item": "bullet"}},
    "notes-whole-chapter-skip": {"notes": {"whole_chapter": "skip", "reference_in_text": "verse"}},
    "notes-duplicates-drop": {"notes": {"duplicates": "drop"}},
    "notes-label-own-slide-off": {"notes": {"label_own_slide": False}},
    "notes-paragraph": {"notes": {"line_breaks": "paragraph"}},
    "notes-wrapped-separate": {"notes": {"wrapped_lines": "separate"}},
    "verses-version-source-default": {"verses": {"version_source": "default"}},
    "verses-version-source-required": {"verses": {"version_source": "required"}},
    "verses-quote-check-off": {"verses": {"version_from_quote": "off"}},
    "slides-label-segment": {"slides": {"reference_label_mode": "segment"}},
    "slides-label-source": {"slides": {"reference_label_mode": "source"}},
    "slides-verse-numbers": {"slides": {"include_verse_numbers": True}},
    "slides-reference-once": {"slides": {"repeat_reference_on_continuation": False}},
    "slides-body-lines-2": {"slides": {"max_body_lines": 2, "line_width_units": 30.0}},
    "slides-no-punctuation-breaks": {"slides": {"prefer_punctuation_breaks": False, "max_body_lines": 3}},
    "slides-tight-tolerance": {"slides": {"even_split_tolerance": 0.0, "even_split_punctuation_tolerance": 0.0, "max_body_lines": 3}},
    "alias-override": {"bible": {"version_aliases": {"YLT": "NLT", "NKJ": "NKJV"}}},
}


def whitefield_cases() -> list[Case]:
    cases = [text_case(f"whitefield-{name}", WHITEFIELD, "whitefield", options=opts) for name, opts in WHITEFIELD_OPTION_SETS.items()]
    cases.append(text_case("whitefield-no-fetch", WHITEFIELD, "whitefield", run={"no_fetch": True}))
    cases.append(text_case("whitefield-default-version-niv", WHITEFIELD, "whitefield", run={"default_version": "NIV"}))
    return cases


def fixture_cases() -> list[Case]:
    cases = []
    for path in sorted((TESTS / "fixtures").rglob("*.txt")):
        if path.name == "whitefield_notes.txt":
            continue
        cases.append(text_case("fixture-" + path.stem.replace("_", "-"), path.read_text(encoding="utf-8"), "fixture-files"))
    return cases


FORMAT_TEXT = """Title 11/22/26 Grace
1) God saves by grace through faith.
Ephesians 2:8-9 NKJV
2) We were made for good works that He prepared for us beforehand so that we would walk in them every day of our lives.
Ephesians 2:10
3) Peace
Romans 5:1 ESV
"""


def format_cases() -> list[Case]:
    lines = FORMAT_TEXT.rstrip("\n").split("\n")
    return [
        text_case("format-txt", FORMAT_TEXT, "formats"),
        Case("format-md", FORMAT_TEXT.encode(), "md", group="formats"),
        Case("format-html", build_html(FORMAT_TEXT.rstrip("\n")), "html", group="formats"),
        Case("format-docx", build_docx(FORMAT_TEXT.rstrip("\n")), "docx", group="formats"),
        Case("format-pdf", build_pdf(lines), "pdf", group="formats"),
        # a PDF that cut a long line in two (wrapped lines are joined back by the classifier)
        Case("format-pdf-wrapped", build_pdf(["Title 11/22/26 Grace", "1) God saves by grace through faith in Jesus Christ and", "not by anything we have done.", "Ephesians 2:8-9 NKJV"]), "pdf", group="formats"),
    ]


def synthetic_cases() -> list[Case]:
    sys.path.insert(0, str(TESTS))
    source = ast.literal_eval(next(
        n.value for n in ast.parse((TESTS / "test_synthetic_acceptance.py").read_text(encoding="utf-8")).body
        if isinstance(n, ast.Assign) and getattr(n.targets[0], "id", "") == "SOURCE"))
    return [
        text_case("synthetic-acceptance-usfm", source, "synthetic", synthetic_usfm=True),
        text_case("synthetic-acceptance-generated", source, "synthetic"),
        text_case("synthetic-acceptance-narrow", source, "synthetic", options={"slides": {"max_body_lines": 2, "line_width_units": 30.0}}),
    ]


EPH_KJV = "For by grace are ye saved through faith; and that not of yourselves: it is the gift of God: Not of works, lest any man should boast."
EPH_NKJV = "For by grace you have been saved through faith, and that not of yourselves; it is the gift of God, not of works, lest anyone should boast."
JOHN_KJV = "Jesus answered and said unto him, Verily, verily, I say unto thee, Except a man be born again, he cannot see the kingdom of God."
JOHN_NKJV = "Jesus answered and said to him, Most assuredly, I say to you, unless one is born again, he cannot see the kingdom of God."
QUOTE_DOC = ('1) Faith\n"By grace are ye saved through faith; and that not of yourselves: it is the gift of God: Not of works, lest any man should boast" (Ephesians 2:8).\n'
             '"Jesus answered and said to him, Most assuredly, I say to you, unless one is born again, he cannot see the kingdom of God."\nJohn 3:3\n')
QUOTE_TEXTS = {"KJV Ephesians 2:8": EPH_KJV, "NKJV Ephesians 2:8": EPH_NKJV, "KJV John 3:3": JOHN_KJV, "NKJV John 3:3": JOHN_NKJV}


def curated_cases() -> list[Case]:
    cases = [
        text_case("kitchen-sink-defaults", KITCHEN, "kitchen-sink"),
        text_case("kitchen-sink-no-fetch", KITCHEN, "kitchen-sink", run={"no_fetch": True}),
        text_case("kitchen-sink-no-default-version", KITCHEN.replace("Psalm 23:1 NKJV", "Psalm 23:1"), "kitchen-sink", run={"default_version": ""}),
        text_case("quote-version-from-quote", QUOTE_DOC, "versions", overrides=QUOTE_TEXTS),
        text_case("quote-version-check-off", QUOTE_DOC, "versions", overrides=QUOTE_TEXTS, options={"verses": {"version_from_quote": "off"}}),
        text_case("quote-version-written", QUOTE_DOC.replace("(Ephesians 2:8).", "(Ephesians 2:8 NKJV)."), "versions", overrides=QUOTE_TEXTS),
        text_case("provider-missing-version", "Title 11/22/26\nJohn 3:16 NKJV\nRomans 8:28 ESV\nPsalm 23:1 NIV\n", "providers", missing_versions=("ESV",)),
        text_case("provider-unknown-version-label", "Title 11/22/26\nJohn 3:16 NKJV\nRomans 8:28 NIVV\nPsalm 23:1 HCSB\n", "providers"),
        text_case("version-comparison-pair", "Title 11/22/26\nJohn 3:16 NKJV\nJohn 3:16 NIV\nRomans 8:28\n", "versions"),
        text_case("version-source-default-cross-chapter", "Title 11/22/26\nGenesis 1:30 – 2:3 NKJV\nPsalm 23:1-6\n", "versions", options={"verses": {"version_source": "default"}}),
        text_case("label-typos-three-keys", "Title 11/8/26\n\nKEY #1) ONE\nKEY#2) TWO\nKEY #3) THREE\nKEY #4) FOUR\n", "label-fix"),
        text_case("long-verse-split", "Title 11/22/26\nEsther 8:9 NKJV\nPsalms 119:1-3 NIV\n", "fitting", options={"slides": {"max_body_lines": 2, "line_width_units": 36.0}}),
    ]
    review_doc = "Title 11/8/26\n\nKEY#1) ONE\nKEY #2) TWO\nKEY #3) THREE\nKEY #4) FOUR\n(pause)\nSome stray line of prose to maybe add.\nJohn 3:16 NIV\n"
    cases.append(text_case("review-choices-skip-include-reject", review_doc, "review-choices", choices={"skip": [4], "include": [5, 6], "reject_fixes": [1]}))
    for name, opts in OPTION_VARIANTS.items():
        cases.append(text_case(f"options-{name}", KITCHEN, "options", options=opts))
    return cases


# ---- classifier cases: one small document per rule branch of notes/classifier.py that the documents above never reach
# (found with scripts/classifier_coverage.py), plus a few per setting. They run with no_fetch (no Bible text needed): what is compared is the
# classifier's output (expected/document.json and the parse_start / line_join / paragraph / item trace records).
_FULL = "Grace is the free and undeserved favor of God toward people who could never earn it by their own effort."
_LEAD = "The shepherd leads us beside still waters and gives us rest in every season of life and"


def _page(*middle: str) -> str:
    """A page of notes whose long lines make the classifier treat the text as hard wrapped (like a PDF)."""
    return "1) Opening point\n\n" + "\n\n".join([_FULL] * 9 + list(middle) + [_FULL] * 3) + "\n"


def classifier_cases() -> list[Case]:
    def cls(name: str, text: str, **options) -> Case:
        return text_case(f"classifier-{name}", text, "classifier", run={"no_fetch": True}, options=options)

    cases = [
        # --- [slide] tags
        cls("slide-tag-empty", "Title 11/22/26\n[slide]\n[slide] -\n1) One\n[ slide ] : Photo of the cross\n[slide] Faith = pistis\n[SLIDE] Video clip of the baptism\n"),
        cls("slide-tag-long-media-word", "1) One\n[slide] " + "a very long description of the screen that goes on and on and on for a while " * 2 + "\n"),
        # --- a reference in the middle of a sentence
        cls("midref-skip", "1) Point\nAs Psalm 23:1 says, he leads us.\nSee also John 3:16 and Romans 8:28 and Acts 2:1 and Acts 2:2 here\n", notes={"reference_in_text": "skip"}),
        cls("midref-verse", "1) Point\nHe's told us in John 3:16 and Romans 8:28 that it is so, and more.\nAs Psalm 23:1 says\n", notes={"reference_in_text": "verse"}),
        cls("midref-review", "1) Point\nAs Psalm 23:1 says, he leads us.\nSee Genesis 1:1; Exodus 3:14; Acts 2:1; Acts 2:2 for more\n"),
        # --- quoted text
        cls("quote-above-reference-review", "Title 11/22/26\n“The Lord is my shepherd; I shall not want.”\nPsalm 23:1 NKJV\n“Another quote that is far away.”\n\n\n\n\nJohn 3:16 NIV\n", notes={"quote_above_reference": "review"}),
        cls("quote-above-reference-skip", "Title 11/22/26\n“The Lord is my shepherd; I shall not want.”\nPsalm 23:1 NKJV\n“Another quote that is far away.”\n\n\n\n\nJohn 3:16 NIV\n"),
        cls("quote-alone-skip", "1) Point\n“A quotation with no reference anywhere near it.”\n", notes={"quote_alone": "skip"}),
        cls("quote-alone-bullet", "1) Point\n“A quotation with no reference anywhere near it.”\n", notes={"quote_alone": "bullet"}),
        cls("quote-alone-review", "1) Point\n“A quotation with no reference anywhere near it.”\n'And one in single quotes'\n«Guillemets too»\n"),
        cls("quote-inline-and-above", "“For God so loved the world that He gave His only begotten Son” (John 3:16 NKJV)\n“For by grace you have been saved through faith, and that not of yourselves”\nEphesians 2:8-9\n“Four words only here”\nPsalm 23:1\n\nLong ago, in the days of Moses, “Trust in the Lord with all your heart, and lean not on your own understanding” — Proverbs 3:5\n"),
        # --- references: lead, trailing, in brackets, bad, whole chapter
        cls("lead-reference-chain", "John 3:16 and then Romans 8:28 NIV\nJohn 3:16; Romans 8:28; Acts 2:1 NIV\nJohn 3:16, 17, 19\nPsalm 23\nGenesis 1:1 – 2:3\n"),
        cls("trailing-reference-chain", "“For God so loved the world that He gave His only Son” John 3:16; Romans 8:28 NIV\n“Be still, and know that I am God” John 3:16 and Psalm 46:10\n"),
        cls("reference-in-brackets", "(Romans 8:16)\n[John 3:16 NIV]\n(Romans 8:16) and more words\n(see Romans 8:16)\n(John 3:16; Romans 8:28)\n( Psalm 23:1 )\n[Psalm 23]\n"),
        cls("issues-in-a-verse-line", "John 3:16; John 3:99\nRomans 8:28 NIV and Genesis 51:2\n1) John 3:16; Acts 99:99\n(Romans 8:28)\nPsalm 23:1; Psalm 23:999 NIV\n"),
        cls("bad-references", "John 3:99\nRevelation 23:1\nGenesis 51:1\nPsalm 151:1\n1) Not a real one: John 3:99\nAs John 3:99 says\n"),
        cls("references-and-aliases", "John 3:16 YLT\nRomans 8:28 (NKJ)\nPsalm 23:1 xyz\nActs 2:1 NIVV\n", bible={"version_aliases": {"YLT": "NLT", "NKJ": "NKJV"}}),
        # --- a document with nothing in it, or only a title
        cls("empty-document", ""),
        cls("blank-lines-only", "\n\n   \n\t\n"),
        cls("only-a-title", "Sunday 11/22/26"),
        cls("only-a-plain-sentence", "This is only one plain sentence that is not anything at all."),
        # --- the lines before the first verse or point, and what to do with them
        cls("before-first-item-skip", "Pastor Amy's notes\nThis was a good week.\nAn overlong opening sentence that rambles on and on without any end to it at all because it keeps going\nAn overlong opening sentence that rambles on and on past eighty characters and ends here.\n1) A point\n"),
        cls("before-first-item-review", "Pastor Amy's notes\nThis was a good week.\nAn overlong opening sentence that rambles on and on without any end to it at all because it keeps going\nAn overlong opening sentence that rambles on and on past eighty characters and ends here.\n1) A point\n", notes={"before_first_item": "review"}),
        cls("before-first-item-bullet", "Pastor Amy's notes\nThis was a good week.\nAn overlong opening sentence that rambles on and on without any end to it at all because it keeps going\nAn overlong opening sentence that rambles on and on past eighty characters and ends here.\n1) A point\n", notes={"before_first_item": "bullet"}),
        cls("before-first-item-headings-skip", "Pastor Amy's notes\nA Title Line\n1) A point\n", notes={"headings": "skip"}),
        cls("email-header-lines", "Subject: Easter plans\nFrom: Pastor Amy\n1) A point\nReply to <amy@example.com> please\nTo: Everyone\nCc : Someone\n[quoted text hidden]\nsent: Monday\n"),
        # --- headings, titles and ALL CAPS lines
        cls("headings-and-titles", "Part 2: The Shepherd\n1) A point\nWeek 3 - Rest\nSession 4.\nSeries 5:\n6 KEYS TO A NEW AWAKENING\nTwo Words\nA Longer Heading Line Here\nIntroduction\n2 Corinthians\n12:9 NKJV\nTHE LORD IS GOOD.\nTHE LORD IS GOOD\nGOD\nTHE LORD IS GOOD, SAYS THE PSALMIST\n"),
        cls("headings-skip-caps-line", "1) A point\nTHE LORD IS GOOD\nPart 2: The Shepherd\nTwo Words\nIntroduction\nRECAP: Keys\nRecap the week\n", notes={"headings": "skip", "caps_lines": "line"}),
        cls("headings-header-max-words", "1) A point\nOne Two Three Four\nOne Two Three\nOne Two\nOne\n12 Days\nlower case Words\n", notes={"header_max_words": 4}),
        cls("headings-header-max-words-zero", "1) A point\nOne Two\nOne\n", notes={"header_max_words": 0}),
        cls("unlabeled-sentence-review", "1) A point\nThis is a full sentence of ordinary prose.\nA short line without an end\n", notes={"unlabeled_sentence": "review", "unlabeled_short": "review"}),
        cls("unlabeled-sentence-skip", "1) A point\nThis is a full sentence of ordinary prose.\nA short line without an end\n", notes={"unlabeled_sentence": "skip", "unlabeled_short": "skip"}),
        cls("unlabeled-sentence-bullet", "1) A point\nThis is a full sentence of ordinary prose.\nA short line without an end\nthe lowest case of all\n", notes={"unlabeled_sentence": "bullet", "unlabeled_short": "bullet"}),
        # --- Recap, dates, web addresses, notes to self, stage cues
        cls("recap-and-dates", "Recap\nRecap: the week\n1) A point\nRecap of last week\nSunday 11/22/26 and more\nPsalm 12:9 on 1/2/26\nThe date is 3/4/2026.\n"),
        cls("stage-cues-skip", "1) A point\nShow the photo of the cross\nWhen I say Amen, roll the video\nafter this, stop the music\nhttps://example.com/x\nwww.example.com\n(ask Amy to cue the video)\n[todo: fix slide]\nNote: remember the offering\nTBD\nFYI the band is late\nplaying with fire\nShowing off\n"),
        cls("stage-cues-bullet", "1) A point\nShow the photo of the cross\nhttps://example.com/x\n(ask Amy to cue the video)\n", notes={"stage_cues": "bullet"}),
        cls("stage-cues-review", "1) A point\nShow the photo of the cross\nhttps://example.com/x\n(ask Amy to cue the video)\nSing it loud\n", notes={"stage_cues": "review", "skip_patterns": ["loud$"]}),
        cls("extra-cue-words", "1) A point\nPause for prayer\nsing the chorus\nLoud + noise now\nA|B test\npausing is not a cue\nSILENCE here\n", notes={"extra_cue_words": ["pause", "Sing", " loud + noise ", "a|b", "", "   "]}),
        cls("skip-patterns", "1) A point\nClosing thoughts for the week\n10 min\nIt's a trap\n[x marks the spot\naab and ab\nfoo foo here\nFOO bar\nplain line that stays\nTab\there\nkeep me\n", notes={"skip_patterns": ["^Closing", "\\d+\\s*min$", "it's", "[[]x", "a{,2}b", "(?P<w>foo)\\s+(?P=w)", "\\ttab", "tab\\t"]}),
        cls("skip-pattern-flags-and-boundaries", "1) A point\nAnother Word\nwordy\nword\nNoTE to self\n", notes={"skip_patterns": ["(?-i:^another)", "(?i:\\bWORD\\b)", "^\\Bote"]}),
        cls("skip-pattern-empty-hard-wrap", _page(_LEAD + "\nEvery Single Day"), notes={"skip_patterns": [""]}),
        cls("skip-pattern-hits-in-hard-wrap", _page(_LEAD + "\nEvery Single Day", _LEAD + "\nClosing remarks"), notes={"skip_patterns": ["^Closing"]}),
        # --- markers and keyed headers
        cls("markers", "1) First\n2. Second\n#3: Third\na) Letter\nB. Letter two\n3: Colon\n12: 9 NKJV\n4) 12 apples\n5)\n6) \n7 ) space before\n#8) hash\nc: not a letter marker\n1000) too long\n- dash bullet\n• dot bullet\n* star\n– en dash\n—x\n"),
        cls("markers-out-of-sequence", "1) One\n2) Two\n5) Five\n3) Three\n4) Four\n6) Six\n"),
        cls("markers-no-sequence-rule", "1) One\n2) Two\n5) Five\n3) Three\n", points={"require_sequence": False}),
        cls("markers-numbered-off", "1) One\na) Letter\n- bullet\nKEY #1) Header\n2. John 3:16 NIV\n", points={"numbered": False}),
        cls("markers-lettered-off", "1) One\na) Letter\nb. Letter\n- bullet\n", points={"lettered": False}),
        cls("markers-bullets-off", "1) One\na) Letter\n- bullet\n• bullet\n* star\n", points={"bullets": False}),
        cls("markers-with-references", "1) Read Romans 8:28 today\n2. John 3:16 NIV\na) Genesis 1:1; Exodus 3:14\n- Acts 2:1\n- see Acts 2:2 now\n#4: Romans 8:28 – 9:1 NIV\n"),
        cls("keyed-points", "KEY #1) First\nKEY#2) Second\nStep 3: Third\nPrinciple #4. Fourth\nreason 5- Fifth\nWAY 6) Sixth\nTip 7: Seventh\nLesson #8) Eighth\nTruth 9. Ninth\nKEY #10) Tenth\nKEY #1000) Too many\nKEY #11)\nPoint #12) John 3:16 and Romans 8:28 here\nkey #13) Psalm 23:1 NIV\n"),
        cls("keyed-points-out-of-order", "KEY #2) Second first\nKEY #1) First\nKEY #4) Four\n"),
        # --- labelled lines ("Conclusion: ...")
        cls("labelled-lines", "Conclusion: first line is not its own slide\n1) A point\nConclusion: Will You Let God Use Your Life?\nApplication - live it out\ninvitation — come forward\nTakeaway : one thing\nConclusions are not labels\nBig idea: God is near\nmain point:none\n"),
        cls("labelled-lines-options", "1) A point\nConclusion: Will You Let God Use Your Life?\nCustom word: this one counts\nThe end - this one too\n", notes={"label_words": ["custom word", "the end", "  ", "a.b"]}),
        cls("labelled-lines-own-slide-off", "1) A point\nConclusion: Will You Let God Use Your Life?\n", notes={"label_own_slide": False}),
        cls("labelled-lines-no-words", "1) A point\nConclusion: Will You Let God Use Your Life?\n", notes={"label_words": []}),
        # --- one line break = one slide, or a paragraph under the point
        cls("line-breaks-paragraph", "1) A point\nA detail sentence under it.\nAnother detail sentence.\nNo end punctuation here\n\n\n\nA far away sentence.\n2) Next\nA quote follows.\n“A quoted detail.”\nA question?\nAn exclamation!\nSOME HEADING.\nPsalm 23:1\nA detail after a verse.\n- bullet\nDetail under a bullet.\n", notes={"line_breaks": "paragraph"}),
        cls("line-breaks-paragraph-skip-detail", "1) A point\nA detail sentence under it.\n", notes={"line_breaks": "paragraph", "unlabeled_sentence": "skip"}),
        cls("line-breaks-paragraph-review", "1) A point\nA detail sentence under it.\n", notes={"line_breaks": "paragraph", "unlabeled_sentence": "review"}),
        cls("line-breaks-paragraph-slide-note", "1) A point\n[slide] A note slide\nA detail sentence under it.\nConclusion: done.\nA detail after the label.\n", notes={"line_breaks": "paragraph"}),
        # --- duplicates
        cls("duplicates-drop", "John 3:16 NIV\nJohn 3:16 NIV\njohn 3:16 niv\nJohn 3:16 KJV\nRomans 8:28; Romans 8:28 NIV\nAs John 3:16 says\n", notes={"duplicates": "drop"}),
        cls("duplicates-keep", "John 3:16 NIV\nJohn 3:16 NIV\n"),
        # --- wrapped lines (a PDF or an email broke the lines mid-sentence)
        cls("wrap-tail-joins", _page(_LEAD + "\nEvery Single Day")),
        cls("wrap-orphan-max-words-1", _page(_LEAD + "\nEvery Single Day"), notes={"orphan_max_words": 1}),
        cls("wrap-orphan-max-words-0", _page(_LEAD + "\nDay"), notes={"orphan_max_words": 0}),
        cls("wrap-separate", _page(_LEAD + "\nEvery Single Day"), notes={"wrapped_lines": "separate"}),
        cls("wrap-sentence-ended", _page(_LEAD.replace("life and", "life.") + "\nEvery Single Day")),
        cls("wrap-lower-case-continues", _page(_LEAD + "\nthen walks beside us on the road to the very end of it all")),
        cls("wrap-numbered-point-complete", _page("2) The Lord is my shepherd.\nThe shepherd leads us beside still waters and more")),
        cls("wrap-capitals-heading", _page("A SHORT CAPITALS HEADING THAT NEEDS MORE WORDS HERE AND THERE AND\nthe next sentence in lower case runs on for many words indeed ok")),
        cls("wrap-capitals-heading-tail", _page("A SHORT CAPITALS HEADING THAT NEEDS MORE WORDS HERE AND THERE AND\nTHE END")),
        cls("wrap-reference-after-full-line", _page("“" + _LEAD + " and it goes on and so on into the evening”\nJohn 3:16 NIV")),
        cls("wrap-reference-after-short-line", _page("“A short quote of about nine words in a row.”\nJohn 3:16 NIV")),
        cls("wrap-open-quote-note", _page("“Jesus said to him, Truly I say to you, today you will be with Me in paradise and he said\nremember me when You come into Your kingdom.”\nLuke 23:42-43 NKJV")),
        cls("wrap-new-item-starts", _page(_LEAD + "\nKEY #2) NEXT", _LEAD + "\n- a bullet", _LEAD + "\n3) a marker", _LEAD + "\nShow the photo", _LEAD + "\nhttps://example.com/x", _LEAD + "\n“A quote opens”", _LEAD + "\n(a note in brackets)", _LEAD + "\nRecap", _LEAD + "\n[slide] a tag")),
        cls("wrap-reference-closes-the-line", _page("“" + _LEAD + " and it goes on” John 3:16 NIV\nwith more words after it\nNIV is a version", "Psalm 23:1 NIV\nand more words follow")),
        cls("wrap-capital-letters-narrow", _page("ILLICIT ILLNESS IS ITIL FILLED LIT LIFTS JUST IT ITS ALL TILL FIT\nIT IS")),
        cls("wrap-email-style", "\n".join(["From: Pastor Amy <amy@example.com>", "Subject: Sunday notes", ""] + [
            "Dear friends, this week we will look at what it means to rest in the Lord who", "is our shepherd and leads us beside quiet waters, restoring our souls and", "guiding us in paths of righteousness for the sake of His name. When we walk",
            "through the darkest valley we fear no evil, for You are with me; Your rod and", "Your staff, they comfort me.", "", "1) Rest", "Psalm 23:1-4 NKJV"] + [
            "Dear friends, this week we will look at what it means to rest in the Lord who", "is our shepherd and leads us beside quiet waters, restoring our souls and"]) + "\n"),
        cls("wrap-no-join-when-narrow", "1) One\n" + "\n".join(["A short line of about forty chars here"] * 12) + "\ntail\n"),
        # --- text oddities
        cls("odd-whitespace", "1) One\u00a0with nbsp\n\u00a02) Two\n3)\tTab\n\x0b4) vertical tab\n\x0c5) form feed\n6) Six\x1c\n   7) Indented\n\u3000\u30008) Ideographic space\n"),
        cls("turkish-dotted-i", "Conclusıon: dotless i\n1) A point\nCONCLUSİON: dotted capital\nConclusion: plain\nSHOW the photo\nSHOW\u0307 mark\nKELVIN: \u212aEY #1) x\n\u212aEY #2) kelvin sign\nſtep 3) long s\n"),
        cls("unicode-case", "1) ÉCOLE ÉLÈVES\nÉCOLE ÉLÈVES\nÀ la carte\nΣΟΦΊΑ ΘΕΟΎ\nǅ titlecase\nA\u0301B\u0301C\u0301\n² superscript\n1: ² verse?\n1: ① circled\n"),
        cls("many-points-and-verses", "\n".join(f"{n}) Point number {n}\nJohn 3:{n} NIV" for n in range(1, 13)) + "\n"),
        cls("long-text-in-trace", "1) " + "word " * 500 + "\nA long line " + "x" * 2500 + "\n"),
        cls("crlf-and-cr-line-ends", "1) One\r\n2) Two\rJohn 3:16 NIV\r\n"),
    ]
    return cases


# ---- documents lifted mechanically from the test files
_SKIP_FILES = {"test_autostart.py", "test_bible_providers.py", "test_config_page.py", "test_fetched_workflow.py", "test_themes.py", "test_trace.py", "test_logging.py"}
_CALLS = {"plan_notes", "classify_document", "run", "plan", "_plan", "doc", "notes", "classify", "_classify"}


def _constant(node: ast.AST) -> str | None:
    try:
        value = ast.literal_eval(node)
    except Exception:
        value = None
    if isinstance(value, str):
        return value
    if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute) and node.func.attr == "join" and isinstance(node.func.value, ast.Constant) \
            and isinstance(node.func.value.value, str) and len(node.args) == 1 and isinstance(node.args[0], (ast.List, ast.Tuple)):
        parts = [_constant(item) for item in node.args[0].elts]
        if all(p is not None for p in parts):
            return node.func.value.value.join(parts)
    return None


def test_document_strings() -> list[tuple[str, str]]:
    """(file stem, document) for every multi-line string constant, joined literal list, or first argument of a plan/classify helper in tests/."""
    found: list[tuple[str, str]] = []
    for path in sorted(TESTS.rglob("test_*.py")):
        if path.name in _SKIP_FILES:
            continue
        tree = ast.parse(path.read_text(encoding="utf-8"))
        docstrings = set()
        for n in ast.walk(tree):
            if isinstance(n, (ast.Module, ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)) and n.body and isinstance(n.body[0], ast.Expr) \
                    and isinstance(n.body[0].value, ast.Constant):
                docstrings.add(id(n.body[0].value))
        for n in ast.walk(tree):
            if id(n) in docstrings:
                continue
            text = None
            if isinstance(n, ast.Constant) and isinstance(n.value, str) and "\n" in n.value:
                text = n.value
            elif isinstance(n, ast.Call):
                name = getattr(n.func, "id", getattr(n.func, "attr", ""))
                if name in _CALLS and n.args:
                    text = _constant(n.args[0])
                elif name == "join":
                    text = _constant(n)
            if text and len([l for l in text.split("\n") if l.strip()]) >= 1 and sum(c.isalpha() for c in text) >= 8 and "\\id " not in text and "=" not in text.split("\n")[0][:12]:
                found.append((path.stem.removeprefix("test_"), text))
    seen: set[str] = {WHITEFIELD, KITCHEN}
    out: list[tuple[str, str]] = []
    for stem, text in found:
        if text in seen or text.strip() in seen:
            continue
        seen.add(text)
        out.append((stem, text))
    return out


def test_derived_cases() -> list[Case]:
    cases = []
    for stem, text in test_document_strings():
        digest = hashlib.sha1(text.encode("utf-8")).hexdigest()[:8]
        cases.append(text_case(f"tests-{stem.replace('_', '-')}-{digest}", text, "tests"))
    return cases


def all_cases() -> list[Case]:
    cases = whitefield_cases() + fixture_cases() + format_cases() + synthetic_cases() + curated_cases() + test_derived_cases() + classifier_cases()
    names = [c.name for c in cases]
    assert len(names) == len(set(names)), "duplicate case names"
    return cases


# ------------------------------------------------------------------------------------------------------- running a case
def build_config(options: dict) -> AppConfig:
    return AppConfig.model_validate(options)


def plan_json(result) -> dict:
    data = result.model_dump(mode="json")
    data.pop("target", None)
    data.pop("files_changed", None)
    data["looks"] = None
    return data


def document_json(text: str, config: AppConfig, trace_rows: list[dict]) -> dict:
    """The NoteDocument the classifier made of the extracted text, plus the rule that decided each paragraph (from the run's own trace)."""
    document = classify_document(text, config.bible.version_aliases, config.notes, config.points)
    paragraphs = [{key: row[key] for key in ("line", "end_line", "label", "rule", "why", "first_in_document")} for row in trace_rows if row["stage"] == "paragraph"]
    return {**document.model_dump(mode="json"), "paragraphs": paragraphs}


def run_case(case: Case, folder: Path) -> dict[str, str | bytes]:
    """Run one case through the app's own functions and return the files to write (relative path -> content)."""
    run_params = {**DEFAULT_RUN, **case.run}
    options_toml = tomli_w.dumps(sort_tree({**case.options, **({"run": case.run} if case.run else {})}))
    parsed = tomllib.loads(options_toml)                      # what a reader of options.toml gets back
    run_params = {**DEFAULT_RUN, **parsed.get("run", {})}
    config = build_config({k: v for k, v in parsed.items() if k != "run"})
    input_name = f"input.{case.ext}"
    scratch = Path(tempfile.mkdtemp(prefix="oracle-"))
    try:
        path = scratch / input_name
        path.write_bytes(case.data)
        default_version = run_params["default_version"] or None
        no_fetch = bool(run_params["no_fetch"])

        table: dict[str, str] = {}
        if not no_fetch:                                       # pass 1: discover (and, for the USFM case, read) the verse text; no run is active
            source = None
            if case.synthetic_usfm:
                import os
                from versevision.bibles.cache import ScriptureCache
                from versevision.bibles.registry import ProviderRegistry
                from versevision.config import load_config
                cwd = os.getcwd()
                os.chdir(PY_ROOT)
                try:
                    provider_config = load_config(TESTS / "fixtures" / "synthetic-provider.toml")
                    source = ProviderRegistry(provider_config.bible, ScriptureCache(scratch / "cache.sqlite3"))
                    recorder = Recorder({}, set(), source)
                    asyncio.run(plan_notes(extract_text(path), config, default_version=default_version, provider_registry=recorder))
                    asyncio.run(source.aclose())
                finally:
                    os.chdir(cwd)
            else:
                recorder = Recorder(case.overrides, set(case.missing_versions))
                asyncio.run(plan_notes(extract_text(path), config, default_version=default_version, provider_registry=recorder))
            table = dict(sorted(recorder.table.items()))
        bible_json = dumps(table)
        table = json.loads(bible_json)                         # pass 2 sees exactly what the file says
        if not no_fetch and case.missing_versions:
            table = {k: v for k, v in table.items() if k.split(" ", 1)[0].casefold() not in {m.casefold() for m in case.missing_versions}}

        trace_folder = scratch / "run"
        trace_folder.mkdir()
        run = runlog.RunLog(trace_folder, "oracle", "oracle")
        token = runlog.activate(run)
        try:
            text = extract_text(path)
            result = asyncio.run(plan_notes(text, config, default_version=default_version, no_fetch=no_fetch,
                                            provider_registry=None if no_fetch else OracleBible(table)))
        finally:
            runlog.deactivate(token)
        trace_path = trace_folder / "trace.jsonl"
        trace_rows = [json.loads(line) for line in trace_path.read_text(encoding="utf-8").split("\n") if line] if trace_path.exists() else []   # not splitlines(): U+2028 and U+0085 can sit inside a JSON string
        files: dict[str, str | bytes] = {
            input_name: case.data,
            "options.toml": options_toml,
            "bible.json": bible_json,
            "expected/plan.json": dumps(plan_json(result)),
            "expected/trace.jsonl": "".join(dumps(row, indent=None) for row in trace_rows),
            "expected/review.json": dumps(build_report(result, source_name=input_name)),
            "expected/extracted.txt": text,
            "expected/document.json": dumps(document_json(text, config, trace_rows)),
        }
        if case.choices:
            after = apply_review_choices(result, skip=set(case.choices.get("skip", [])), include=set(case.choices.get("include", [])),
                                         reject_fixes=set(case.choices.get("reject_fixes", [])))
            files["choices.json"] = dumps(case.choices)
            files["expected/plan_after_choices.json"] = dumps(after.model_dump(mode="json"))
        return files
    finally:
        shutil.rmtree(scratch, ignore_errors=True)


def check_clean(name: str, files: dict[str, str | bytes]) -> None:
    for rel, content in files.items():
        if isinstance(content, str):
            for bad in ("/home/", "/tmp/", "oracle-", "\\Users\\"):
                assert bad not in content, f"{name}/{rel} contains {bad!r}"


# ------------------------------------------------------------------------------------------------------ line-level oracles
def _string_constants(paths: list[Path]) -> list[str]:
    out: list[str] = []
    for path in paths:
        tree = ast.parse(path.read_text(encoding="utf-8"))
        docstrings = {id(n.body[0].value) for n in ast.walk(tree) if isinstance(n, (ast.Module, ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef))
                      and n.body and isinstance(n.body[0], ast.Expr) and isinstance(n.body[0].value, ast.Constant)}
        for n in ast.walk(tree):
            if isinstance(n, ast.Constant) and isinstance(n.value, str) and id(n) not in docstrings and n.value.strip() and "\n" not in n.value:
                out.append(n.value)
    return out


EXTRA_REFERENCE_LINES = [
    "Gen 1:1", "Ex. 20:1-17", "Deut 6:4-9", "Ps 23", "Psalm 119:105", "Prov 3:5-6 ESV", "Isa. 53:5", "Matt 5:3-12 NIV", "Mk 1:15", "Lk 2:10-11",
    "Jn 3:16", "Rom 8:1", "1 Cor 13:4-7", "I Cor 13:13", "2 Tim 3:16-17 NASB", "Phil 4:6-7", "Heb 11:1", "Jas 1:2-4", "1 Pet 5:7", "Rev 21:4",
    "Acts 2:1-4 & 6-8", "John 3:16; Romans 5:8", "John 3:16, Romans 5:8", "Luke 1:1 through 4", "Genesis 1:1 – 2:3", "Genesis 1:31 - 2:1 KJV",
    "Psalm 23:1-6 (NKJV)", "see Matthew 5:3 and Luke 6:20", "Matthew 28:19-20.", "“Romans 8:28”", "(John 3:16)", "John 3:16 NIVV", "John 3:16 YLT",
    "John 3:16 HCSB", "Jude 25", "Jude 1:26", "Obadiah 21", "3 John 14", "3 John 15", "Philemon 25", "Song of Songs 2:1", "Revelation 22:21", "Revelation 23:1",
    "Genesis 51:1", "Genesis 50:27", "John 0:1", "John 1:0", "John 3:16-3:15", "Psalm 151:1", "Hezekiah 3:4", "Matthew", "Matthew 5", "John 3:16 & ",
]


ALIASES = {"YLT": "NLT", "NKJ": "NKJV"}


def references_oracle() -> list[dict]:
    lines = _string_constants([TESTS / "unit" / "test_parser.py", TESTS / "unit" / "test_parser_properties.py", TESTS / "unit" / "test_real_world_layout.py"])
    lines = [l for l in lines if any(c.isdigit() for c in l)]            # parameter names and other prose in the tests are not references
    lines += EXTRA_REFERENCE_LINES
    lines += [l for l in WHITEFIELD.split("\n") + KITCHEN.split("\n") if any(c.isdigit() for c in l)]
    rows: list[dict] = []
    seen: set[tuple[str, bool]] = set()

    def add(line: str, aliases: dict | None) -> None:
        if (line, bool(aliases)) in seen:
            return
        seen.add((line, bool(aliases)))
        try:
            result = parse_references(line, aliases) if aliases else parse_references(line)
        except Exception as exc:          # the Python parser crashes on a few inputs (e.g. chapter 0); that is recorded, not hidden
            rows.append({"aliases": aliases, "input": line, "issues": [], "raises": type(exc).__name__, "references": []})
            return
        rows.append({"aliases": aliases, "input": line, "issues": [i.model_dump(mode="json") for i in result.issues],
                     "references": [r.model_dump(mode="json") for r in result.references]})

    for line in lines:
        add(line, None)
    for line in ("John 3:16 YLT", "John 3:16 NKJ", "John 3:16 (NKJ)", "John 3:16, YLT", "John 3:16 NKJV", "Romans 8:28 NIVV"):
        add(line, ALIASES)
    return rows


FITTING_TEXTS = [
    "Short.",
    "By grace are ye saved through faith; and that not of yourselves: it is the gift of God: Not of works, lest any man should boast.",
    "For God so loved the world, that He gave His only begotten Son, that whoever believes in Him should not perish but have everlasting life.",
    "The Lord is my shepherd; I shall not want. He makes me to lie down in green pastures; He leads me beside the still waters.",
    "Trust in the Lord with all your heart, and lean not on your own understanding; in all your ways acknowledge Him, and He shall direct your paths.",
    "Therefore, if anyone is in Christ, he is a new creation; old things have passed away; behold, all things have become new.",
    "In the beginning God created the heavens and the earth.",
    "SPEAK THE BORN AGAIN MESSAGE",
    "KEY #2) PRAY WITHOUT CEASING AND GIVE THANKS IN EVERYTHING FOR THIS IS GOD'S WILL FOR YOU",
    "• We must preach Christ crucified, risen, and returning—not ourselves, our methods, or our moods.",
    "One two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen nineteen twenty",
    "Supercalifragilisticexpialidocious-and-antidisestablishmentarianism are-single-words-that-cannot-be-cut-anywhere-at-all",
    "First line\nSecond line which is a little longer than the first one is\nThird",
    "“Do not be conformed to this world, but be transformed by the renewing of your mind, that you may prove what is that good and acceptable and perfect will of God.”",
    "Rejoice always, pray without ceasing, in everything give thanks; for this is the will of God in Christ Jesus for you.",
    "He who has begun a good work in you will complete it until the day of Jesus Christ.",
    "Be anxious for nothing, but in everything by prayer and supplication, with thanksgiving, let your requests be made known to God.",
    "And we know that all things work together for good to those who love God, to those who are the called according to His purpose.",
    "Amen.",
    "",
    "a b c d e f g h i j k l m n o p q r s t u v w x y z a b c d e f g h i j k l m n o p q r s t u v w x y z",
    "Wait, what?! No—yes; maybe: perhaps. Okay, fine, sure, why not, let's go on and see what happens next time around here.",
    "iiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiii WWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWW",
    synthetic_verse("NKJV", "Esther", 8, 9),
    synthetic_verse("NIV", "Psalms", 119, 2),
    synthetic_verse("NKJV", "Genesis", 1, 3),
    synthetic_verse("ESV", "John", 3, 1),
]


def fitting_oracle() -> list[dict]:
    rows: list[dict] = []

    def add(fn: str, args: dict, out: Any) -> None:
        rows.append({"args": args, "fn": fn, "out": out})

    balance_sets = [
        {"line_width_units": 33.0, "upper": True, "glue_first": 0, "max_lines": 4, "tolerance": 0.15, "punctuation_tolerance": 0.25},
        {"line_width_units": 20.0, "upper": True, "glue_first": 2, "max_lines": 4, "tolerance": 0.15, "punctuation_tolerance": 0.25},
        {"line_width_units": 48.0, "upper": False, "glue_first": 0, "max_lines": 3, "tolerance": 0.0, "punctuation_tolerance": 0.0},
        {"line_width_units": 33.0, "upper": True, "glue_first": 0, "max_lines": 4, "tolerance": 0.15, "punctuation_tolerance": 0.5},
    ]
    for i, text in enumerate(FITTING_TEXTS):
        for j in (0, 1, 2, 3):
            if (i + j) % 4 == 3 and j == 3 and i % 2:
                continue
            args = {"text": text, **balance_sets[j]}
            add("balance_lines", args, fitting.balance_lines(**args))
    verse_sets = [
        {"max_lines": 5, "line_width_units": 48.0, "prefer_punctuation": True, "tolerance": 0.15, "punctuation_tolerance": 0.25},
        {"max_lines": 2, "line_width_units": 36.0, "prefer_punctuation": True, "tolerance": 0.15, "punctuation_tolerance": 0.25},
        {"max_lines": 3, "line_width_units": 30.0, "prefer_punctuation": False, "tolerance": 0.15, "punctuation_tolerance": 0.25},
        {"max_lines": 2, "line_width_units": 36.0, "prefer_punctuation": True, "tolerance": 0.0, "punctuation_tolerance": 0.0},
        {"max_lines": 2, "line_width_units": 36.0, "prefer_punctuation": True, "tolerance": 0.05, "punctuation_tolerance": 0.5},
    ]
    for i, text in enumerate(FITTING_TEXTS):
        for j in range(len(verse_sets)):
            if (i + j) % 2 and j > 2:
                continue
            args = {"text": text, **verse_sets[j]}
            add("split_verse_text", args, fitting.split_verse_text(**args))
    point_sets = [
        {"line_width_units": 33.0, "max_lines": 2, "upper": True, "glue_first": 0, "tolerance": 0.15, "punctuation_tolerance": 0.25},
        {"line_width_units": 33.0, "max_lines": 1, "upper": True, "glue_first": 2, "tolerance": 0.15, "punctuation_tolerance": 0.25},
        {"line_width_units": 24.0, "max_lines": 3, "upper": False, "glue_first": 0, "tolerance": 0.0, "punctuation_tolerance": 0.0},
        {"line_width_units": 33.0, "max_lines": 0, "upper": True, "glue_first": 0, "tolerance": 0.15, "punctuation_tolerance": 0.25},
        {"line_width_units": 33.0, "max_lines": 2, "upper": True, "glue_first": 0, "tolerance": 0.1, "punctuation_tolerance": 0.6},
    ]
    for i, text in enumerate(FITTING_TEXTS):
        for j in range(len(point_sets)):
            if (i + j) % 2 and j > 2:
                continue
            args = {"text": text, **point_sets[j]}
            add("split_point_text", args, fitting.split_point_text(**args))
    for text in FITTING_TEXTS:
        add("text_width_units", {"value": text}, fitting.text_width_units(text))
        for width in (20.0, 48.0):
            add("wrap_body_lines", {"text": text, "line_width_units": width}, fitting.wrap_body_lines(text, width))
    return rows


def versions_oracle() -> list[dict]:
    from versevision.versions.registry import _NAMES
    rows: list[dict] = []
    labels = list(_NAMES) + ["nkjv", "Niv", "esv", "NASB", "nasb", "NASB95", "NASB20", "NASB1995", "NASB2020", "NIV11", "niv2011", "NIV84", "NIV2011", "YLT",
                             "HCSB", "NCV", "GNT", "CEV", "MEV", "NABRE", "KJ21", "TANAKH", "Phillips", "NIVV", "NKJVV", "ESB", "NLTT", "NIB", "KJ", "NKJ", "NA", "X", "XYZ", "", "John", "AMPC", "NIRV"]
    for label in labels:
        rows.append({"input": label, "kind": "version", "out": {"canonical": canonical_version(label), "corrected": correct_version(label), "is_real": is_real_version(label)}})
    for label, available in (("NIVV", ["NIV", "NKJV"]), ("NKJ", ["NKJV", "KJV"]), ("ES", ["ESV", "ESB"]), ("NLTT", ["NLT"]), ("AMPC", ["AMP"]), ("CSBB", ["CSB", "CSV"])):
        rows.append({"available": sorted(available), "input": label, "kind": "version_available", "out": {"corrected": correct_version(label, set(available))}})

    from types import SimpleNamespace as NS
    quote_cases = [
        ("Except a man be born again, he cannot see the kingdom of God", [JOHN_KJV]), ("Except a man be born again, he cannot see the kingdom of God", [JOHN_NKJV]),
        ("Most assuredly, I say to you, unless one is born again", [JOHN_NKJV]), ("Most assuredly, I say to you, unless one is born again", [JOHN_KJV]),
        ("By grace are ye saved through faith; and that not of yourselves: it is the gift of God: Not of works, lest any man should boast", [EPH_KJV]),
        ("By grace are ye saved through faith; and that not of yourselves: it is the gift of God: Not of works, lest any man should boast", [EPH_NKJV]),
        ("FOR BY GRACE ARE YE SAVED", [EPH_KJV]), ("for by grace are ye saved", [EPH_NKJV]), ("lest anyone should boast", [EPH_NKJV]), ("lest anyone should boast", [EPH_KJV]),
        ("it’s the gift of God", ["It's the gift of God."]), ("‘Verily’ I say", ["Verily, verily, I say unto thee"]), ("", [EPH_KJV]), ("something", []), ("something", [""]),
        ("1 2 3 four", ["1. two 3 four"]), ("In the beginning God created", ["In the beginning God created the heavens and the earth.", "And God said, Let there be light."]),
        ("the earth the beginning", ["In the beginning God created the heavens and the earth."]),
    ]
    for quoted, texts in quote_cases:
        passage = NS(verses=[NS(text=t) for t in texts])
        rows.append({"kind": "quote_score", "out": quote_score(quoted, passage), "quoted": quoted, "verse_texts": texts})
    return rows


def write_jsonl(path: Path, rows: list[dict]) -> None:
    write_text(path, "".join(dumps(r, indent=None) for r in rows))


# ------------------------------------------------------------------------------------------------------------ config
def flatten_config(value: Any, prefix: str = "") -> dict[str, Any]:
    """Leaves of a dumped config: tables are flattened to dotted paths, a list, a scalar or an empty table is one leaf."""
    if isinstance(value, dict) and value:
        out: dict[str, Any] = {}
        for key, inner in value.items():
            out.update(flatten_config(inner, f"{prefix}.{key}" if prefix else key))
        return out
    return {prefix: value}


def dump_config(config: AppConfig) -> dict:
    return config.model_dump(mode="json")


# (base, profile, sets): `base` is validated into the starting config (usually {} or a few profiles).
PROFILES = {
    "profiles": {
        "sunday": {"notes": {"line_breaks": "paragraph", "extra_cue_words": ["pause", "sing"]}, "slides": {"max_body_lines": 4}},
        "narrow": {"slides": {"line_width_units": 30, "max_body_lines": 3}, "points": {"line_width_units": 20.5}},
        "bad-range": {"slides": {"max_body_lines": 99}},
        "bad-name": {"slides": {"no_such_option": 1}, "no_such_section": {"x": 1}},
        "replace": {"bible": {"version_aliases": {"YLT": "NIV", "XYZ": "ESV"}, "default_version": "NIV"}},
        "text-for-number": {"propresenter": {"port": "8080", "timeout_seconds": "7.5"}},
        "providers": {"bible": {"providers": {
            "esv": {"type": "esv"},
            "api": {"type": "api_bible", "bible_ids": {"NIV": "abc"}, "timeout_seconds": 30},
            "local": {"type": "usfm", "version": "NKJV", "path": "~/Bibles//nkjv/", "name": "Local"},
        }}},
        "bad-provider": {"bible": {"providers": {"x": {"type": "nope"}}}},
        "provider-no-type": {"bible": {"providers": {"x": {"timeout_seconds": 3}}}},
        "provider-esv-cache": {"bible": {"providers": {"x": {"type": "esv", "cache_policy": "allowed"}}}},
        "usfm-incomplete": {"bible": {"providers": {"x": {"type": "usfm"}}}},
        "template": {"presentation_template": {"presentation_uuid": "u", "presentation_name": "n", "cue_uuid": "c", "style_fingerprint": "f"}},
        "template-incomplete": {"presentation_template": {"presentation_uuid": "u"}},
        "template-not-table": {"presentation_template": 5},
        "section-not-table": {"slides": 5},
        "wrong-types": {"slides": {"include_verse_numbers": "yes", "max_characters": 200.0, "reference_format": 5}},
        "bool-words": {"slides": {"include_verse_numbers": "T", "break_on_new_verse": "off", "preserve_poetry_lines": 0}},
    },
}

CONFIG_OPTION_ROWS: list[tuple[dict, str | None, list[str]]] = [
    # valid: every kind of field
    ({}, None, []),
    ({}, None, ["slides.max_body_lines=3"]),
    ({}, None, ["slides.max_body_lines= 4 "]),
    ({}, None, ["slides.max_body_lines=1_0"]),
    ({}, None, ["slides.max_body_lines=1e1"]),
    ({}, None, ["slides.max_body_lines=7.0"]),
    ({}, None, ["slides.max_body_lines=true"]),
    ({}, None, ["slides.line_width_units=40"]),
    ({}, None, ["slides.line_width_units=40.5"]),
    ({}, None, ["slides.line_width_units=1e2"]),
    ({}, None, ["slides.line_width_units=5.0001"]),
    ({}, None, ["slides.line_width_units=200"]),
    ({}, None, ["slides.include_verse_numbers=true"]),
    ({}, None, ["slides.include_verse_numbers=YES"]),
    ({}, None, ["slides.include_verse_numbers=on"]),
    ({}, None, ["slides.include_verse_numbers=1"]),
    ({}, None, ["slides.include_verse_numbers=t"]),
    ({}, None, ["slides.include_verse_numbers=1.0"]),
    ({}, None, ["slides.break_on_new_verse=off"]),
    ({}, None, ["slides.break_on_new_verse= No "]),
    ({}, None, ["slides.break_on_new_verse=false"]),
    ({}, None, ["slides.break_on_new_verse=0"]),
    ({}, None, ["slides.break_on_new_verse=ye\u017f"]),
    ({}, None, ["slides.reference_format={book} {chapter}:{verses}"]),
    ({}, None, ["slides.reference_format=1"]),
    ({}, None, ["slides.reference_format=true"]),
    ({}, None, ["slides.reference_format= padded "]),
    ({}, None, ["slides.reference_label_mode=segment"]),
    ({}, None, ["slides.even_split_tolerance=0"]),
    ({}, None, ["slides.even_split_tolerance=0.5"]),
    ({}, None, ["slides.even_split_tolerance=1"]),
    ({}, None, ["slides.max_characters=500", "slides.max_verses_per_slide=2"]),
    ({}, None, ["notes.line_breaks=paragraph"]),
    ({}, None, ["notes.unlabeled_sentence=review", "notes.stage_cues=bullet"]),
    ({}, None, ["notes.extra_cue_words=[\"pause\", \"sing\"]"]),
    ({}, None, ["notes.extra_cue_words=[]"]),
    ({}, None, ["notes.label_words= [\"conclusion\"] "]),
    ({}, None, ["notes.skip_patterns=[\"^\\\\d+$\", \"x{2}\"]"]),
    ({}, None, ["notes.header_max_words=0"]),
    ({}, None, ["notes.header_max_words=6"]),
    ({}, None, ["notes.orphan_max_words=8"]),
    ({}, None, ["notes.label_own_slide=no"]),
    ({}, None, ["points.numbering=renumber", "points.number_style=#{n}:"]),
    ({}, None, ["points.bullet_char="]),
    ({}, None, ["points.case=title"]),
    ({}, None, ["points.theme=Group/Name", "points.theme_slide=1"]),
    ({}, None, ["points.theme_slide=2"]),
    ({}, None, ["points.max_lines_per_slide=0"]),
    ({}, None, ["points.max_lines_per_slide=12"]),
    ({}, None, ["points.line_tolerance=0.2", "points.line_tolerance_at_punctuation=0.3"]),
    ({}, None, ["points.line_width_units=24"]),
    ({}, None, ["verses.version_source=required"]),
    ({}, None, ["verses.quote_versions=[\"NIV\"]"]),
    ({}, None, ["verses.quote_match=0.5"]),
    ({}, None, ["verses.quote_match=1"]),
    ({}, None, ["propresenter.host=10.0.0.5", "propresenter.port=50002", "propresenter.timeout_seconds=2.5"]),
    ({}, None, ["propresenter.port=65535"]),
    ({}, None, ["propresenter.timeout_seconds=120"]),
    ({}, None, ["node.type=hermes-remote", "node.name=imac"]),
    ({}, None, ["node.name="]),
    ({}, None, ["node.retries=0", "node.retry_delay_seconds=0", "node.quit_wait_seconds=1", "node.launch_wait_seconds=600"]),
    ({}, None, ["node.library_root=$HOME/Pro Presenter/Libraries"]),
    ({}, None, ["presentation.backup_count=100"]),
    ({}, None, ["output.library=Sermons", "output.swap_keeps_id=yes"]),
    ({}, None, ["bible.default_version=NIV"]),
    ({}, None, ["bible.default_version="]),
    ({}, None, ["bible.version_aliases.YLT=KJV"]),
    ({}, None, ["bible.version_aliases.YLT=5"]),
    ({}, None, ["slides.max_body_lines=2", "slides.max_body_lines=6"]),
    # unknown names and bad shapes
    ({}, None, ["slides.nope=1"]),
    ({}, None, ["nope.max_body_lines=1"]),
    ({}, None, ["slides=1"]),
    ({}, None, ["slides.max_body_lines"]),
    ({}, None, [""]),
    ({}, None, ["=3"]),
    ({}, None, [".max_body_lines=3"]),
    ({}, None, ["slides..max_body_lines=3"]),
    ({}, None, [" slides.max_body_lines =3"]),
    ({}, None, ["Slides.max_body_lines=3"]),
    ({}, None, ["slides.max_body_lines.x=3"]),
    ({}, None, ["bible.version_aliases.Q=NIV"]),
    ({}, None, ["bible.providers.x.type=esv"]),
    ({}, None, ["bible.providers.x=esv"]),
    ({}, None, ["presentation_template.cue_uuid=x"]),
    ({}, None, ["presentation_template=1"]),
    ({}, None, ["profiles.sunday.notes.line_breaks=slide"]),
    ({}, None, ["slides.max_body_lines=3", "nope.x=1"]),
    # out of range and wrong type
    ({}, None, ["slides.max_body_lines=0"]),
    ({}, None, ["slides.max_body_lines=21"]),
    ({}, None, ["slides.max_body_lines=-1"]),
    ({}, None, ["slides.max_body_lines=2.5"]),
    ({}, None, ["slides.max_body_lines=abc"]),
    ({}, None, ["slides.max_body_lines="]),
    ({}, None, ["slides.max_body_lines=inf"]),
    ({}, None, ["slides.max_body_lines=nan"]),
    ({}, None, ["slides.max_body_lines=1e400"]),
    ({}, None, ["slides.max_body_lines=\uff11\uff12"]),
    ({}, None, ["slides.line_width_units=\uff11\uff12.\uff15"]),
    ({}, None, ["slides.include_verse_numbers=5.0"]),
    ({}, None, ["slides.include_verse_numbers=1e1"]),
    ({}, None, ["slides.include_verse_numbers=-1"]),
    ({}, None, ["slides.max_body_lines=[3]"]),
    ({}, None, ["slides.max_body_lines=0x10"]),
    ({}, None, ["slides.max_body_lines=1__0"]),
    ({}, None, ["slides.max_characters=0"]),
    ({}, None, ["slides.max_verses_per_slide=0"]),
    ({}, None, ["slides.max_reference_lines=4"]),
    ({}, None, ["slides.max_reference_lines=0"]),
    ({}, None, ["slides.line_width_units=5"]),
    ({}, None, ["slides.line_width_units=4.99"]),
    ({}, None, ["slides.line_width_units=200.5"]),
    ({}, None, ["slides.line_width_units=nan"]),
    ({}, None, ["slides.line_width_units=inf"]),
    ({}, None, ["slides.line_width_units=-inf"]),
    ({}, None, ["slides.line_width_units=wide"]),
    ({}, None, ["slides.line_width_units="]),
    ({}, None, ["slides.reference_width_units=5"]),
    ({}, None, ["slides.reference_width_units=201"]),
    ({}, None, ["slides.even_split_tolerance=-0.1"]),
    ({}, None, ["slides.even_split_tolerance=1.1"]),
    ({}, None, ["slides.even_split_tolerance=nan"]),
    ({}, None, ["slides.even_split_punctuation_tolerance=2"]),
    ({}, None, ["slides.include_verse_numbers=maybe"]),
    ({}, None, ["slides.include_verse_numbers=2"]),
    ({}, None, ["slides.include_verse_numbers=1.5"]),
    ({}, None, ["slides.include_verse_numbers=[1]"]),
    ({}, None, ["slides.include_verse_numbers=ſ"]),
    ({}, None, ["slides.include_verse_numbers="]),
    ({}, None, ["slides.reference_label_mode=Segment"]),
    ({}, None, ["slides.reference_label_mode=words"]),
    ({}, None, ["slides.reference_label_mode=1"]),
    ({}, None, ["notes.line_breaks=line"]),
    ({}, None, ["notes.before_first_item=bullets"]),
    ({}, None, ["notes.quote_alone=verse"]),
    ({}, None, ["notes.reference_in_text=bullet"]),
    ({}, None, ["notes.whole_chapter=verse"]),
    ({}, None, ["notes.duplicates=true"]),
    ({}, None, ["notes.extra_cue_words=pause"]),
    ({}, None, ["notes.extra_cue_words=pause,sing"]),
    ({}, None, ["notes.extra_cue_words=[1]"]),
    ({}, None, ["notes.extra_cue_words=[\"a\", 2]"]),
    ({}, None, ["notes.extra_cue_words=[\"a\", null]"]),
    ({}, None, ["notes.extra_cue_words=[[\"a\"]]"]),
    ({}, None, ["notes.extra_cue_words=[\"a\""]),
    ({}, None, ["notes.extra_cue_words=5"]),
    ({}, None, ["notes.extra_cue_words=true"]),
    ({}, None, ["notes.extra_cue_words="]),
    ({}, None, ["notes.header_max_words=7"]),
    ({}, None, ["notes.header_max_words=-1"]),
    ({}, None, ["notes.orphan_max_words=9"]),
    ({}, None, ["points.numbering=roman"]),
    ({}, None, ["points.case=Upper"]),
    ({}, None, ["points.max_lines_per_slide=13"]),
    ({}, None, ["points.max_lines_per_slide=-1"]),
    ({}, None, ["points.max_lines=0"]),
    ({}, None, ["points.max_lines=13"]),
    ({}, None, ["points.line_width_units=5"]),
    ({}, None, ["points.line_tolerance=1.5"]),
    ({}, None, ["points.line_tolerance_at_punctuation=-0.5"]),
    ({}, None, ["verses.version_source=always"]),
    ({}, None, ["verses.version_from_quote=on"]),
    ({}, None, ["verses.quote_match=0.49"]),
    ({}, None, ["verses.quote_match=1.01"]),
    ({}, None, ["verses.quote_match=0"]),
    ({}, None, ["propresenter.port=0"]),
    ({}, None, ["propresenter.port=65536"]),
    ({}, None, ["propresenter.port=80.5"]),
    ({}, None, ["propresenter.port=http"]),
    ({}, None, ["propresenter.port=true"]),
    ({}, None, ["propresenter.host=5"]),
    ({}, None, ["propresenter.timeout_seconds=0"]),
    ({}, None, ["propresenter.timeout_seconds=-1"]),
    ({}, None, ["propresenter.timeout_seconds=120.5"]),
    ({}, None, ["propresenter.timeout_seconds=nan"]),
    ({}, None, ["node.type=remote"]),
    ({}, None, ["node.name=5"]),
    ({}, None, ["node.name=true"]),
    ({}, None, ["node.name=[\"a\"]"]),
    ({}, None, ["node.retries=6"]),
    ({}, None, ["node.retries=-1"]),
    ({}, None, ["node.retry_delay_seconds=-1"]),
    ({}, None, ["node.retry_delay_seconds=121"]),
    ({}, None, ["node.quit_wait_seconds=0.5"]),
    ({}, None, ["node.launch_wait_seconds=4"]),
    ({}, None, ["node.launch_wait_seconds=601"]),
    ({}, None, ["node.confirm_quit=perhaps"]),
    ({}, None, ["presentation.backup_count=0"]),
    ({}, None, ["presentation.backup_count=101"]),
    ({}, None, ["output.swap_keeps_id=2"]),
    ({}, None, ["bible.default_version=5"]),
    ({}, None, ["bible.default_version=1.5"]),
    ({}, None, ["bible.providers=foo"]),
    ({}, None, ["bible.version_aliases=[1]"]),
    ({}, None, ["bible.source=stagepilot"]),
    ({}, None, ["bible.source=own_keys"]),
    ({}, None, ["bible.source=auto"]),
    ({}, None, ["bible.source=sometimes"]),
    ({}, None, ["bible.source=OWN_KEYS"]),
    ({}, None, ["bible.source=5"]),
    ({}, None, ["bible.source=true"]),
    (PROFILES, "providers", ["bible.source=stagepilot"]),
    # the first error is the first in the model's field order, not the first --set
    ({}, None, ["slides.max_body_lines=0", "propresenter.port=0"]),
    ({}, None, ["points.max_lines=0", "notes.header_max_words=9", "slides.max_characters=0"]),
    ({}, None, ["bible.default_version=7", "node.name=7", "notes.line_breaks=x"]),
    ({}, None, ["slides.even_split_tolerance=2", "slides.line_width_units=1", "slides.max_body_lines=0"]),
    # profiles
    (PROFILES, "sunday", []),
    (PROFILES, "sunday", ["notes.line_breaks=slide"]),
    (PROFILES, "sunday", ["slides.max_body_lines=2"]),
    (PROFILES, "narrow", []),
    (PROFILES, "narrow", ["points.line_width_units=33"]),
    (PROFILES, "replace", []),
    (PROFILES, "text-for-number", []),
    (PROFILES, "providers", []),
    (PROFILES, "providers", ["bible.providers.api.timeout_seconds=45"]),
    (PROFILES, "providers", ["bible.providers.api.bible_ids.NIV=xyz"]),
    (PROFILES, "providers", ["bible.providers.api.bible_ids.KJV=xyz"]),
    (PROFILES, "providers", ["bible.providers.api.timeout_seconds=0"]),
    (PROFILES, "providers", ["bible.providers.esv.cache_policy=allowed"]),
    (PROFILES, "providers", ["bible.providers.local.copyright=(c) someone"]),
    (PROFILES, "providers", ["bible.providers.local.name="]),
    (PROFILES, "providers", ["bible.providers.local.path=a/./b/"]),
    (PROFILES, "providers", ["bible.providers.local.path=5"]),
    (PROFILES, "providers", ["bible.providers.local.type=esv"]),
    (PROFILES, "providers", ["bible.providers.nothing.type=esv"]),
    (PROFILES, "template", []),
    (PROFILES, "template", ["presentation_template.reference_object=Ref"]),
    (PROFILES, "template", ["presentation_template.cue_uuid=zz"]),
    (PROFILES, "template", ["presentation_template.cue_uuid=5"]),
    (PROFILES, "providers", ["bible.providers.api.type=esv"]),
    (PROFILES, "providers", ["bible.providers.api.type=nope"]),
    (PROFILES, "providers", ["bible.providers.api.timeout_seconds=abc"]),
    (PROFILES, "providers", ["bible.providers.local.version=KJV"]),
    (PROFILES, "providers", ["bible.providers.local.path=true"]),
    (PROFILES, "template", ["presentation_template.point_cue_uuid=p1"]),
    (PROFILES, "template", ["presentation_template.library_name=5"]),
    (PROFILES, "bool-words", []),
    (PROFILES, "bad-range", []),
    (PROFILES, "bad-range", ["slides.max_body_lines=3"]),
    (PROFILES, "bad-name", []),
    (PROFILES, "bad-provider", []),
    (PROFILES, "provider-no-type", []),
    (PROFILES, "provider-esv-cache", []),
    (PROFILES, "usfm-incomplete", []),
    (PROFILES, "template-incomplete", []),
    (PROFILES, "template-not-table", []),
    (PROFILES, "section-not-table", []),
    (PROFILES, "wrong-types", []),
    (PROFILES, "nobody", []),
    (PROFILES, "", ["slides.max_body_lines=3"]),
    ({}, "sunday", []),
    ({"profiles": {"b": {}, "a": {}, "C": {}}}, "zzz", []),
    (PROFILES, "sunday", ["notes.extra_cue_words=[\"x\"]"]),
]


def config_options_oracle() -> list[dict]:
    rows = []
    for base, profile, sets in CONFIG_OPTION_ROWS:
        start = AppConfig.model_validate(copy.deepcopy(base))          # with_options hands pieces of the profile to the data it edits: never share
        row: dict[str, Any] = {"base": base, "profile": profile, "sets": sets}
        before = flatten_config(dump_config(start))          # before the call: it may edit the profile tables it merges
        try:
            result = with_options(start, profile=profile, sets=sets)
        except ConfigurationError as exc:
            row["out"] = {"error": str(exc)}
        else:
            after = flatten_config(dump_config(result))
            changed = {path: value for path, value in after.items() if before.get(path, object()) != value}
            removed = [path for path in before if path not in after]
            row["out"] = {"changed": changed, **({"removed": removed} if removed else {})}
        rows.append(row)
    return rows


def write_config_oracle(out: Path) -> None:
    import tomli_w
    defaults = AppConfig()
    write_text(out / "config_defaults.json", dumps(dump_config(defaults)))
    write_text(out / "config_defaults.toml", tomli_w.dumps(defaults.model_dump(mode="json", exclude_none=True)))
    # a config that uses every kind of value (lists, providers, a template, profiles with odd names, text that needs escaping)
    rich = AppConfig.model_validate({
        "node": {"type": "hermes-remote", "name": "iMac \"main\"", "backup_dir": "$HOME/x y", "retry_delay_seconds": 7.5},
        "notes": {"extra_cue_words": ["pause", "sing"], "skip_patterns": ["^\\d+$", "a\"b", "tab\there"], "line_breaks": "paragraph"},
        "points": {"bullet_char": "\u2022", "theme": "Café – “Group”/Name", "line_width_units": 24.5},
        "verses": {"quote_versions": ["NIV", "ESV"], "quote_match": 0.9},
        "presentation_template": {"presentation_uuid": "u-1", "presentation_name": "Verses", "cue_uuid": "c-1", "style_fingerprint": "f", "library_name": "Lib"},
        "bible": {"default_version": "NIV", "version_routes": {"ESV": "esv"}, "providers": {
            "esv": {"type": "esv"},
            "api": {"type": "api_bible", "bible_ids": {"NIV": "abc", "KJV": "def"}, "timeout_seconds": 30},
            "local": {"type": "usfm", "version": "NKJV", "path": "~/Bibles//nkjv/", "name": "Local", "copyright": "(c)"},
        }},
        "profiles": {
            "Sunday AM": {"notes": {"line_breaks": "paragraph", "extra_cue_words": ["pause"]}, "slides": {"max_body_lines": 4, "include_verse_numbers": True}},
            "narrow": {"slides": {"line_width_units": 30.5}, "bible": {"providers": {"x": {"type": "esv"}}}},
        },
    })
    write_text(out / "config_rich.toml", tomli_w.dumps(rich.model_dump(mode="json", exclude_none=True)))
    rows = config_options_oracle()
    write_jsonl(out / "config_options.jsonl", rows)
    ok = sum(1 for r in rows if "changed" in r["out"])
    print(f"config_defaults.json, config_defaults.toml, config_rich.toml, config_options.jsonl: {len(rows)} lines ({ok} valid, {len(rows) - ok} errors)")


# ------------------------------------------------------------------------------------------------------------------- main
# ------------------------------------------------------------------------------------------------ what the classifier raises
def document_errors_oracle() -> list[dict]:
    """Documents (and options) for which `classify_document` raises, with the exception and its message. The Rust port must fail the same way.

    Three kinds: the reference parser raising (chapter or verse 0), a pattern in `notes.skip_patterns` that `re.compile` rejects (the exact
    `re.error` text, position included), and two documents that crash the Python importer with an `AttributeError` (a numbered line that holds a
    whole-chapter reference, or a verse the duplicates setting dropped: `doc.items[-1].warnings` is not there on a review or skipped item).
    """
    import warnings
    warnings.simplefilter("ignore")                  # `re` warns about "possible nested set" in some patterns
    rows: list[dict] = []

    def add(text: str, notes: dict | None = None, points: dict | None = None, aliases: dict | None = None, must_raise: bool = True) -> None:
        config = AppConfig.model_validate({"notes": notes or {}, "points": points or {}})
        try:
            classify_document(text, aliases, config.notes, config.points)
        except Exception as exc:                      # noqa: BLE001  (the oracle records whatever the Python app raises)
            rows.append({"aliases": aliases, "input": text, "message": str(exc), "notes": notes or {}, "points": points or {}, "raises": type(exc).__name__})
        else:
            if must_raise:
                raise AssertionError(f"classify_document did not raise for {text!r} {notes!r}")

    add("John 0:1\n")
    add("1) A point\nJohn 1:0\n")
    add("1. Psalm 23\n")
    add("2) Genesis 1\n", notes={"whole_chapter": "skip"})
    add("John 3:16\n1. John 3:16\n", notes={"duplicates": "drop"})
    add("Romans 8:28 NIV\n2) Romans 8:28 NIV\n", notes={"duplicates": "drop"})
    for pattern in ["(", ")", "a**", "[a", "a{2,1}", "\\", "(?P<1>a)", "(?<a)", "[z-a]", "\\y", "a(?i)b", "(?<=a+)b", "\\1", "(a)\\2", "[\\d-z]",
                    "x{99999999999}", "(?P<a>x)(?P<a>y)", "*a", "a\n(b", "(?P<n", "(?P=n)", "(?", "(?P", "(?#abc", "(?<", "(?i", "(?i-", "(?-i)", "(?ia-:x)",
                    "\\x4", "\\u12", "\\8", "[\\8]", "\\777", "(?(1)a|b|c)", "(?(2)a)", "(?(x)a)", "^*", "$+", "\\b?", "(?P>x)", "(?z)", "(?i-i:x)", "(?-a:x)",
                    "(?L)", "(?a)(?u)", "[", "[^", "[]", "[a-", "[a-\\d]", "(|", "a|*", "(?=a)(?<=b+)", "(a)(?<=\\1b)", "\\Z{", "x{1,2", "x{2}{3}",
                    "(?P<a>(?P=a))", "(?:", "(?>", "(?<!", "(?<=)", "\\k<a>", "\\p{L}", "\\z", "\\G", "(?x) a (", "a\r\n\\"]:
        add("x\n", notes={"skip_patterns": ["ok", pattern]}, must_raise=False)       # a pattern that compiles raises nothing: no row
    add("x\n", notes={"skip_patterns": ["fine"], "extra_cue_words": ["a(b", "c)"], "label_words": ["[x", "y]"]}, must_raise=False)   # these are escaped: no error
    return rows


# ------------------------------------------------------------------------------------------------ user patterns (Python `re`)
_PATTERN_SUBJECTS = [
    "", " ", "x", "Show the photo", "  SHOW the photo", "showing", "show", "Show\u0301 mark", "When I say Amen", "Conclusion: be kind", "Conclusıon: dotless",
    "CONCLUSİON: dotted", "1) One", "a) Letter", "#2: two", "12: 9 NKJV", "KEY #3) three", "ſtep 3) long s", "\u212aEY #1) kelvin", "10 min", "It's a trap",
    "[x marks", "aab and ab", "foo foo here", "FOO bar", "a\nb", "a\n", "\nabc", "tab\there", "\x1cfoo", "foo\x1c", "foo\u00a0bar", "ééé", "e\u0301", "École",
    "٣٤٥ digits", "² sup", "①", "snake_case", "snake case", "(note)", "[slide] tag", "http://example.com", "www.example.com", "line one.  ", "Psalm 23:1",
    "İstanbul", "ı", "i", "ǅ", "ß", "Σίσυφος", "日本語 text", "😀 emoji", "a" * 40,
]

_PATTERNS = [
    # literals, escapes, special characters
    "show", "^show", "show$", "^$", "", "a|b", "a||b", "(a|b)+", "\\.", "a.b", "\\$", "\\^", "\\\\", "\\/", "\\-", "\\ ", "\\#", "\\n", "\\t", "\\a", "\\f", "\\v", "\\0", "\\x41", "\\x4a", "\\u00e9",
    "\\U0001F600", "\\101", "\\060", "\\1000" if False else "\\400"[:0] + "a", "}", "]", "{", "{}", "{x", "x{", "x{1", "x{,", "x{1,2", "a{,2}", "a{2,}", "a{0}", "a{1}", "a{2,3}", "a{,}",
    "{1}", "x{2}{", "\\{", "a\\{2\\}",
    # classes
    "[abc]", "[^abc]", "[a-c]", "[a-cx-z]", "[]a]", "[^]a]", "[a-]", "[-a]", "[a\\-c]", "[\\d]", "[\\D]", "[\\w-]", "[\\s]", "[\\S]", "[\\W]", "[[]", "[[a]", "[a[]", "[&&]", "[--]",
    "[a&&b]", "[~~]", "[||]", "[\\]]", "[\\[]", "[\\\\]", "[\\n]", "[\\b]", "[\\x41-\\x43]", "[\\u0041-\\u0043]", "[\\101]", "[.]", "[$^]", "[*+?]", "[(){}]", "[é-ü]", "[ß]", "[İ]", "[ıi]",
    "[A-Za-z]", "[0-9]", "[^\\W\\d_]", "[\\w\\s]", "[^\\s\\S]", "[•▪◦●‣*\\-–—]", "[:\\-–—]", "[).:]", "[)\\].]", "[(\\[]",
    # categories and boundaries
    "\\d", "\\D", "\\d+", "\\s", "\\S", "\\s+", "\\w", "\\W", "\\w+", "\\b", "\\B", "\\bshow\\b", "\\Bhow", "show\\B", "\\Ashow", "show\\Z", "^\\s*\\w+\\s*[:\\-]", "\\bé", "\\b\\d+\\b",
    # quantifiers: greedy, lazy, possessive
    "a*", "a+", "a?", "a*?", "a+?", "a??", "a{2,3}?", "a*+", "a++", "a?+", "a{1,2}+", "(ab)*", "(?:ab)+", "(?:ab){2}", "(a|b)*c", "x*y*z*", ".*", ".+?", ".*?e", "o{1,}", "o{,1}",
    # groups
    "(a)", "(a)(b)", "(?:a)", "(?P<n>a)", "(?P<n>a)(?P<m>b)", "(?P<n>a)(?P=n)", "(a)\\1", "(a)(b)\\2\\1", "(?P<n_1>x)", "()", "(?:)", "(a|)", "(|a)", "((a)(b))", "(a(b(c)))", "(?#comment)a",
    "a(?#comment)b", "(?>a+)b", "(?>a|ab)c", "(?:(?:a))", "((?:a)|b)",
    # look-around and conditionals
    "(?=a)", "(?!a)", "(?<=a)b", "(?<!a)b", "a(?=b)", "a(?!b)", "(?<=ab|cd)e", "(?<=\\d{2})x", "(?<![A-Za-z])foo", "(?=.*\\d)\\w+", "(?!)", "(?=)", "(?<=(a))b", "(a)?(?(1)b|c)", "(a)?(?(1)b)",
    "(?P<n>a)?(?(n)b|c)", "(a)?b(?(1)c|d)",
    # flags
    "(?i)show", "(?i)SHOW", "(?s).", "(?m)^a", "(?m)a$", "(?x) a b # comment\n c", "(?x)a\\ b", "(?x)[ ]a", "(?i:a)b", "a(?i:b)c", "(?-i:a)b", "(?s:.)", "(?m:^a)", "(?x: a b )c", "(?im)^a", "(?a)\\w+", "(?a)é",
    "(?u)é", "(?i)é", "(?i)É", "(?i)ß", "(?i)ǅ", "(?i)σ", "(?i)ς", "(?i)\u212a", "(?i)k", "(?i)s", "(?i)ſ", "(?i)i", "(?i)ı", "(?i)İ", "(?i)[i]", "(?i)[ı]", "(?i)[a-z]", "(?i)[^a]",
    # the classifier's own patterns
    "^\\s*(show|play|put|start|cue|fade|transition|bring up|display|run|roll|when i (?:mention(?:ed)?|sa(?:y|id)|get|got|talk(?:ed)?|read|come|came)|after (?:this|that)|at the end|end with|stop)\\b",
    "^\\s*(\\(.*\\)\\s*$|\\[(?!\\s*slide)[^\\]]*\\]\\s*$|(note|notes|todo|to do|reminder|remember|ask|tell|check|fix|tbd|fyi)\\b\\s*[:\\-]?)",
    "^\\s*(from|to|cc|bcc|subject|date|sent)\\s*:|<[^>\\s]+@[^>\\s]+>|^\\[quoted text hidden\\]$",
    "^(?P<label>\\s*(?P<word>key|point|step|principle|reason|way|tip|lesson|truth)\\s*#?\\s*(?P<n>\\d{1,3})\\s*[).:\\-])\\s*(?P<rest>\\S.*)$",
    "^\\s*(?P<hash>#\\s*)?(?P<label>\\d{1,3}|[A-Za-z](?=[).]\\s))\\s*(?P<punct>[).:])\\s*(?P<rest>.*)$",
    "^\\s*[•▪◦●‣*\\-–—]\\s+(?P<rest>\\S.*)$", "^\\s*\\[\\s*slide\\s*\\]\\s*[-–—:]*\\s*(?P<rest>.*)$", "https?://|www\\.", "\\b(slide|photo|picture|image|video|clip|screen|portrait|graphic|manuscript)\\b",
    "^\\s*recap\\b", "\\d{1,2}/\\d{1,2}/\\d{2,4}", "\\d:\\d", "\\d+\\s*:\\s*\\d+", "^(part|week|session|series)\\s*\\d+\\s*[:.-]", "^\\s*(conclusion|application|invitation)\\s*[:\\-–—]\\s*\\S",
    "^\\s*[(\\[]\\s*(?P<inner>[^()\\[\\]]+?)\\s*[)\\]]\\s*$", "[\\[(]?([A-Za-z0-9]+)", "[\\\\/:*?\"<>|\\x00-\\x1f]", "\\s+-\\s+-\\s+", "\\s+\\d+$", "^\\s*(key|point)\\s*#?\\s*\\d+\\s*[).:\\-]",
    # what a user might write
    "^Closing", "\\d+\\s*min$", "it's", "^\\(.*\\)$", "^\\[.*\\]$", "(?i)^music", "^-{3,}$", "^\\*\\*\\*", "\\bTODO\\b", "^=+$", "^\\s*$", "^.{0,3}$", "^[A-Z ]+$", "\\.{3}", "\\?$", "^(Amen|Pray)\\b", "slide \\d+",
    "(?P<w>foo)\\s+(?P=w)", "(\\w+) \\1", "([a-z])\\1", "^(?!.*\\d)", "colou?r", "gr[ae]y", "^\\W+$", "[^\\x00-\\x7f]", "\\p", "é|e\u0301", "(?:^|\\s)x(?:$|\\s)", "\\S+@\\S+",
]


def user_patterns_oracle() -> list[dict]:
    """`re.compile(pattern, re.IGNORECASE or 0)` then `search` on a fixed list of texts: the span, the groups, or the exact error. The Rust port of
    Python's `re` syntax (what the user's `skip_patterns` need) must give the same answer for every pair."""
    import re
    import warnings
    warnings.simplefilter("ignore")
    rows = []
    for pattern in dict.fromkeys(_PATTERNS):
        for ignore_case in (True, False):
            if re.search(r"\(\?[a-zA-Z]*a", pattern) and (ignore_case or "(?i" in pattern):
                continue                              # the ASCII flag together with IGNORECASE: not supported by the Rust port (see notes/pyre.rs)
            row: dict = {"ignorecase": ignore_case, "pattern": pattern}
            try:
                compiled = re.compile(pattern, re.IGNORECASE if ignore_case else 0)
            except Exception as exc:                  # noqa: BLE001
                row["error"] = {"message": str(exc), "type": type(exc).__name__}
            else:
                outcomes = []
                for subject in _PATTERN_SUBJECTS:
                    match = compiled.search(subject)
                    outcomes.append(None if match is None else {"groups": [match.group(i) for i in range(compiled.groups + 1)], "span": list(match.span())})
                row["matches"] = outcomes
            rows.append(row)
    return rows


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--out", type=Path, default=REPO_ROOT / "oracle", help="where to write (default: <repo>/oracle)")
    parser.add_argument("--lines-only", action="store_true", help="only write the line-level files and the config files (quick check)")
    parser.add_argument("--only", help="only cases whose name contains this text (does not clean the output)")
    args = parser.parse_args()
    out: Path = args.out
    cases = [] if args.lines_only else all_cases()
    if args.only:
        cases = [c for c in cases if args.only in c.name]
    elif not args.lines_only and (out / "cases").exists():
        shutil.rmtree(out / "cases")
    groups: dict[str, int] = {}
    for case in cases:
        files = run_case(case, out)
        check_clean(case.name, files)
        for rel, content in files.items():
            target = out / "cases" / case.name / rel
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(content if isinstance(content, bytes) else content.encode("utf-8"))
        groups[case.group] = groups.get(case.group, 0) + 1
    if not args.only:
        for filename, rows in (("references.jsonl", references_oracle()), ("fitting.jsonl", fitting_oracle()), ("versions.jsonl", versions_oracle()),
                               ("document_errors.jsonl", document_errors_oracle()), ("user_patterns.jsonl", user_patterns_oracle())):
            write_jsonl(out / filename, rows)
            print(f"{filename}: {len(rows)} lines")
            if filename == "fitting.jsonl":
                by_fn: dict[str, int] = {}
                for r in rows:
                    by_fn[r["fn"]] = by_fn.get(r["fn"], 0) + 1
                print("   ", dict(sorted(by_fn.items())))
    if not args.only:
        write_text(out / "user_patterns_subjects.json", dumps(_PATTERN_SUBJECTS))
        write_config_oracle(out)
        from oracle_providers import providers_oracle          # the Bible providers (S5): oracle/providers/
        for filename, rows in providers_oracle(out, REPO_ROOT).items():
            write_jsonl(out / "providers" / filename, rows)
            print(f"providers/{filename}: {len(rows)} lines")
    print(f"cases: {len(cases)}")
    for group, count in sorted(groups.items()):
        print(f"   {group}: {count}")


if __name__ == "__main__":
    main()
